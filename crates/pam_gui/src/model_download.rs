//! GUI-owned guided model download.
//!
//! A curated preset becomes a resumable, verified HTTPS acquisition
//! (`pam_model::download_https`) that runs in a single-flight background
//! task. This codebase has no event bus, so progress is polled through
//! [`ModelDownloadManager::snapshot`] instead of pushed.

use std::{
    future::Future,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use directories::BaseDirs;
use pam_model::{
    DownloadRequest, DownloadResponse, DownloadTransport, LicenseConsent, ModelDescriptor,
    ModelError, RegisteredModel, ReqwestDownloadTransport, TransferRequest, download_https,
    model_path_under,
};
use pam_platform::user_data_dir;

use crate::{
    model_import::parse_model_key,
    model_presets::ModelPreset,
    settings,
    store_writes::{StoreWriteFailure, register_model},
};

/// ponytail: Hugging Face's download redirect lands on a region-sharded CDN
/// host (observed `us.aws.cdn.hf.co` from this network today; HF documents
/// `eu.`/`ap.` shards and older `cdn-lfs*` names elsewhere), never a single
/// fixed name. This is a bounded allowlist of Hugging-Face-controlled CDN
/// hosts derived from their own infrastructure naming, not attacker input —
/// widen it if a new shard ever fails a redirect here.
const HUGGING_FACE_REDIRECT_HOSTS: &[&str] = &[
    "us.aws.cdn.hf.co",
    "eu.aws.cdn.hf.co",
    "ap.aws.cdn.hf.co",
    "cdn.hf.co",
    "cdn-lfs.hf.co",
    "cdn-lfs.huggingface.co",
    "cdn-lfs-us-1.huggingface.co",
    "cdn-lfs-eu-1.huggingface.co",
];

/// A bounded, user-facing download failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ModelDownloadFailure {
    pub(crate) detail: String,
    pub(crate) recovery: Option<String>,
}

impl ModelDownloadFailure {
    fn new(detail: impl Into<String>, recovery: impl Into<String>) -> Self {
        Self {
            detail: detail.into(),
            recovery: Some(recovery.into()),
        }
    }
}

impl From<StoreWriteFailure> for ModelDownloadFailure {
    fn from(failure: StoreWriteFailure) -> Self {
        Self {
            detail: failure.detail,
            recovery: failure.recovery,
        }
    }
}

impl From<ModelError> for ModelDownloadFailure {
    fn from(error: ModelError) -> Self {
        // Integrity and redirect refusals are the checks that protect a
        // pasted source, so each one says what to do about it rather than
        // "check your network connection", which would be a lie about a
        // digest mismatch.
        let recovery = match error {
            ModelError::DigestMismatch => {
                "The bytes Pam received do not match the digest you gave. Nothing was registered \
                 and the partial file was discarded. Re-check the digest against the publisher, \
                 or stop trusting this source."
            }
            ModelError::SizeMismatch { .. } => {
                "Re-check the expected size in bytes against the publisher's own listing, then \
                 retry."
            }
            ModelError::RedirectNotAllowed => {
                "Pam follows a pasted download only within the host you pasted. Open the link in \
                 a browser, then paste the final URL it lands on."
            }
            ModelError::TooManyRedirects => {
                "The source redirected too many times. Paste the final download URL instead."
            }
            ModelError::InsecureSource | ModelError::InvalidSource => {
                "Paste a plain HTTPS URL with no credentials, query string, or fragment."
            }
            _ => "Retry the download; if it keeps failing, check your network connection.",
        };
        Self::new(error.to_string(), recovery)
    }
}

/// The download manager's current, polled state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ModelDownloadStatusKind {
    Idle,
    Running,
    Complete,
    Failed,
    /// Stopped on user request; the partial file stays on disk, so starting
    /// the same preset again resumes where the transfer left off.
    Cancelled,
}

/// Where the running download came from: Pam's own hand-checked catalog, or
/// a URL the owner pasted and vouched for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ModelDownloadKind {
    Preset,
    Url,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ModelDownloadSnapshot {
    pub(crate) status: ModelDownloadStatusKind,
    /// Identifies the download the status belongs to: the preset id for a
    /// catalog entry, the `vendor/name` model key for a pasted URL.
    pub(crate) download_id: Option<String>,
    pub(crate) download_kind: Option<ModelDownloadKind>,
    pub(crate) received_bytes: u64,
    pub(crate) total_bytes: u64,
    pub(crate) failure: Option<ModelDownloadFailure>,
}

impl ModelDownloadSnapshot {
    fn idle() -> Self {
        Self {
            status: ModelDownloadStatusKind::Idle,
            download_id: None,
            download_kind: None,
            received_bytes: 0,
            total_bytes: 0,
            failure: None,
        }
    }
}

/// One fully validated acquisition, whatever it came from: the descriptor
/// Pam will verify the bytes against, the source, and the hosts a redirect
/// may land on beyond the source host itself.
///
/// Building one is where every check happens, so nothing reaches
/// [`download_https`] that has not already been parsed and refused on the
/// caller's thread with a message the user can act on.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ModelAcquisition {
    pub(crate) id: String,
    pub(crate) kind: ModelDownloadKind,
    pub(crate) descriptor: ModelDescriptor,
    pub(crate) url: String,
    /// Redirect hosts allowed *beyond* the source's own host, which
    /// `pam_model` always appends. Empty means same-host redirects only.
    pub(crate) allowed_redirect_hosts: Vec<String>,
}

impl ModelAcquisition {
    /// Builds the acquisition for one hand-checked catalog preset. Its URL is
    /// a checked-in constant, so it keeps the Hugging Face CDN redirect
    /// allowlist a pasted URL never gets.
    ///
    /// # Errors
    ///
    /// Returns [`ModelDownloadFailure`] when the preset's own metadata is
    /// malformed, which the catalog's tests already rule out.
    pub(crate) fn from_preset(preset: &ModelPreset) -> Result<Self, ModelDownloadFailure> {
        let key = parse_model_key(preset.model).map_err(|failure| ModelDownloadFailure {
            detail: failure.detail,
            recovery: failure.recovery,
        })?;
        let descriptor = ModelDescriptor::new(
            key,
            preset.file_name,
            preset.expected_digest(),
            preset.expected_size_bytes,
            preset.license()?,
        )?;
        Ok(Self {
            id: preset.id.to_owned(),
            kind: ModelDownloadKind::Preset,
            descriptor,
            url: preset.url.to_owned(),
            allowed_redirect_hosts: HUGGING_FACE_REDIRECT_HOSTS
                .iter()
                .map(|host| (*host).to_owned())
                .collect(),
        })
    }
}

struct ManagerState {
    snapshot: ModelDownloadSnapshot,
    received: Arc<AtomicU64>,
    cancel: Arc<AtomicBool>,
}

/// Single-flight background download runner, polled for progress.
pub(crate) struct ModelDownloadManager {
    state: Mutex<ManagerState>,
}

impl ModelDownloadManager {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(ManagerState {
                snapshot: ModelDownloadSnapshot::idle(),
                received: Arc::new(AtomicU64::new(0)),
                cancel: Arc::new(AtomicBool::new(false)),
            }),
        })
    }

    /// The current status. While a download is running, `received_bytes`
    /// reflects the live counter rather than the value recorded at start.
    pub(crate) fn snapshot(&self) -> ModelDownloadSnapshot {
        let state = self.state.lock().unwrap();
        let mut snapshot = state.snapshot.clone();
        if snapshot.status == ModelDownloadStatusKind::Running {
            snapshot.received_bytes = state.received.load(Ordering::Relaxed);
        }
        snapshot
    }

    /// Starts a real HTTPS download for one preset.
    ///
    /// Takes an owned `Arc` (rather than `&self`) because `self: &Arc<Self>`
    /// is not a valid receiver type in stable Rust; the caller clones the
    /// `Arc` it already holds, which is a cheap refcount bump.
    ///
    /// # Errors
    ///
    /// Returns [`ModelDownloadFailure`] when another download is already
    /// running.
    pub(crate) fn start(
        self: Arc<Self>,
        acquisition: ModelAcquisition,
    ) -> Result<(), ModelDownloadFailure> {
        let home = BaseDirs::new()
            .map(|directories| directories.home_dir().to_path_buf())
            .ok_or_else(|| {
                ModelDownloadFailure::new(
                    "Pam could not resolve the user home directory.",
                    "Verify the operating system user profile, then retry.",
                )
            })?;
        // Settings' persisted custom models directory, when one is set,
        // otherwise the default `<home>/llm`. A Settings read glitch falls
        // back to the default rather than failing the download.
        let models_root = user_data_dir().map_or_else(
            |_| home.join("llm"),
            |data_dir| settings::effective_models_dir(&data_dir, &home),
        );
        self.start_with(acquisition, models_root, |received, cancel| {
            ReqwestDownloadTransport::secure().map(|transport| CountingTransport {
                inner: transport,
                received,
                cancel,
            })
        })
    }

    /// Requests cancellation of the running download. The transfer stops at
    /// the next chunk boundary with its partial file synced to disk, so a
    /// later start of the same preset resumes instead of restarting.
    ///
    /// # Errors
    ///
    /// Returns [`ModelDownloadFailure`] when no download is running.
    pub(crate) fn cancel(&self) -> Result<(), ModelDownloadFailure> {
        let state = self.state.lock().unwrap();
        if state.snapshot.status != ModelDownloadStatusKind::Running {
            return Err(ModelDownloadFailure::new(
                "No model download is running.",
                "Start a download before cancelling one.",
            ));
        }
        state.cancel.store(true, Ordering::Relaxed);
        Ok(())
    }

    /// Starts a download into a caller-supplied models root (destination
    /// becomes `<root>/<vendor>/<filename>`) using the given transport
    /// factory. Kept generic so tests can point at a scratch directory and
    /// inject a fake transport instead of the real Settings-resolved root
    /// and HTTPS.
    pub(crate) fn start_with<T, F>(
        self: Arc<Self>,
        acquisition: ModelAcquisition,
        models_root: PathBuf,
        make_transport: F,
    ) -> Result<(), ModelDownloadFailure>
    where
        T: DownloadTransport + 'static,
        F: FnOnce(Arc<AtomicU64>, Arc<AtomicBool>) -> Result<T, ModelError> + Send + 'static,
    {
        let (received, cancel) = {
            let mut state = self.state.lock().unwrap();
            if state.snapshot.status == ModelDownloadStatusKind::Running {
                return Err(ModelDownloadFailure::new(
                    "A model download is already running.",
                    "Wait for the current download to finish, then retry.",
                ));
            }
            let received = Arc::new(AtomicU64::new(0));
            let cancel = Arc::new(AtomicBool::new(false));
            state.received = Arc::clone(&received);
            state.cancel = Arc::clone(&cancel);
            state.snapshot = ModelDownloadSnapshot {
                status: ModelDownloadStatusKind::Running,
                download_id: Some(acquisition.id.clone()),
                download_kind: Some(acquisition.kind),
                received_bytes: 0,
                total_bytes: acquisition.descriptor.expected_size_bytes,
                failure: None,
            };
            (received, cancel)
        };
        tokio::spawn(async move {
            let seed_received = Arc::clone(&received);
            let outcome = match make_transport(received, cancel) {
                Ok(transport) => {
                    run_download(&transport, &acquisition, &models_root, seed_received).await
                }
                Err(error) => Err(error.into()),
            };
            self.finish(&acquisition, outcome);
        });
        Ok(())
    }

    fn finish(
        &self,
        acquisition: &ModelAcquisition,
        outcome: Result<RegisteredModel, ModelDownloadFailure>,
    ) {
        let mut state = self.state.lock().unwrap();
        let download_id = Some(acquisition.id.clone());
        let download_kind = Some(acquisition.kind);
        let total_bytes = acquisition.descriptor.expected_size_bytes;
        state.snapshot = match outcome {
            Ok(registered) => ModelDownloadSnapshot {
                status: ModelDownloadStatusKind::Complete,
                download_id,
                download_kind,
                received_bytes: registered.size_bytes,
                total_bytes,
                failure: None,
            },
            // A requested cancel surfaces as the transfer error it forced;
            // report it as Cancelled, not Failed. A download that completed
            // before the flag was seen stays Complete above — truthfully.
            Err(_) if state.cancel.load(Ordering::Relaxed) => ModelDownloadSnapshot {
                status: ModelDownloadStatusKind::Cancelled,
                download_id,
                download_kind,
                received_bytes: state.received.load(Ordering::Relaxed),
                total_bytes,
                failure: None,
            },
            Err(failure) => ModelDownloadSnapshot {
                status: ModelDownloadStatusKind::Failed,
                download_id,
                download_kind,
                received_bytes: state.received.load(Ordering::Relaxed),
                total_bytes,
                failure: Some(failure),
            },
        };
    }
}

async fn run_download<T: DownloadTransport>(
    transport: &T,
    acquisition: &ModelAcquisition,
    models_root: &Path,
    received: Arc<AtomicU64>,
) -> Result<RegisteredModel, ModelDownloadFailure> {
    let descriptor = acquisition.descriptor.clone();
    let consent = LicenseConsent::accept(&descriptor);
    let destination = model_path_under(models_root, &descriptor.key, &descriptor.filename)?;
    seed_resume_offset(&destination, &received);
    let registered_at_ms = now_ms()?;
    let request = DownloadRequest {
        descriptor,
        consent,
        source: acquisition.url.clone(),
        allowed_redirect_hosts: acquisition.allowed_redirect_hosts.clone(),
        destination,
        registered_at_ms,
    };
    let registered = download_https(transport, request).await?;
    persist(registered).await
}

/// Seeds the progress counter with bytes already on disk from an earlier,
/// interrupted attempt, so a resumed download reports its true completion
/// rather than restarting the visible percentage at zero.
///
/// Mirrors `pam_model::acquisition::AcquisitionPaths`' own `.part` naming
/// convention rather than exposing it from that crate, since the resume
/// offset `download_https` computes internally never crosses its public API.
/// ponytail: re-stats a file `download_https` will also inspect; if a future
/// resume ever discards this partial (stale/corrupt checkpoint), the bar can
/// briefly overshoot 100% until the download finishes and `finish()`
/// overwrites it with the real size.
fn seed_resume_offset(destination: &Path, received: &AtomicU64) {
    let Some(filename) = destination.file_name().and_then(|name| name.to_str()) else {
        return;
    };
    let partial = destination.with_file_name(format!(".{filename}.pam-model.part"));
    if let Ok(metadata) = std::fs::metadata(partial) {
        received.store(metadata.len(), Ordering::Relaxed);
    }
}

fn now_ms() -> Result<u64, ModelDownloadFailure> {
    let clock_recovery = "Correct the system clock, then retry the download.";
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| {
            ModelDownloadFailure::new(
                "The system clock cannot timestamp the download.",
                clock_recovery,
            )
        })?
        .as_millis();
    u64::try_from(millis).map_err(|_| {
        ModelDownloadFailure::new(
            "The system clock cannot timestamp the download.",
            clock_recovery,
        )
    })
}

/// Persists one verified download through the same routed registration the
/// GUI's local import uses: the daemon writes the registry while it owns it.
async fn persist(registered: RegisteredModel) -> Result<RegisteredModel, ModelDownloadFailure> {
    register_model(registered).await.map_err(Into::into)
}

/// Delegates to an inner transport, adding each response chunk's length into
/// a shared counter so a caller can poll download progress.
pub(crate) struct CountingTransport<T> {
    pub(crate) inner: T,
    pub(crate) received: Arc<AtomicU64>,
    pub(crate) cancel: Arc<AtomicBool>,
}

impl<T: DownloadTransport> DownloadTransport for CountingTransport<T> {
    type Response = CountingResponse<T::Response>;

    fn send(
        &self,
        request: TransferRequest,
    ) -> impl Future<Output = Result<Self::Response, ModelError>> + Send {
        let received = Arc::clone(&self.received);
        let cancel = Arc::clone(&self.cancel);
        let sent = self.inner.send(request);
        async move {
            Ok(CountingResponse {
                response: sent.await?,
                received,
                cancel,
            })
        }
    }
}

pub(crate) struct CountingResponse<R> {
    response: R,
    received: Arc<AtomicU64>,
    cancel: Arc<AtomicBool>,
}

impl<R> CountingResponse<R> {
    #[cfg(test)]
    pub(crate) fn new(response: R, received: Arc<AtomicU64>) -> Self {
        Self {
            response,
            received,
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl<R: DownloadResponse> DownloadResponse for CountingResponse<R> {
    fn status(&self) -> u16 {
        self.response.status()
    }

    fn content_length(&self) -> Option<&str> {
        self.response.content_length()
    }

    fn content_range(&self) -> Option<&str> {
        self.response.content_range()
    }

    fn content_encoding(&self) -> Option<&str> {
        self.response.content_encoding()
    }

    fn etag(&self) -> Option<&str> {
        self.response.etag()
    }

    fn location(&self) -> Option<&str> {
        self.response.location()
    }

    fn next_chunk(&mut self) -> impl Future<Output = Result<Option<Vec<u8>>, ModelError>> + Send {
        let received = Arc::clone(&self.received);
        let cancel = Arc::clone(&self.cancel);
        let next = self.response.next_chunk();
        async move {
            // A requested cancel forces the transfer-error path, which syncs
            // and keeps the partial file for resume; `finish` reports it as
            // Cancelled rather than Failed.
            if cancel.load(Ordering::Relaxed) {
                return Err(ModelError::TransferInterrupted);
            }
            let chunk = next.await?;
            if let Some(chunk) = &chunk {
                let chunk_bytes = u64::try_from(chunk.len()).unwrap_or(u64::MAX);
                received.fetch_add(chunk_bytes, Ordering::Relaxed);
            }
            Ok(chunk)
        }
    }
}

/// Coarse host memory probe: total physical RAM in bytes.
///
/// Pam's supported system minimum: local AI is the product premise, and the
/// validated models need a 32 GiB machine or larger.
pub(crate) const MIN_SUPPORTED_HOST_MEMORY_BYTES: u64 = 32 * (1 << 30);

/// ponytail: shells out to `sysctl -n hw.memsize` instead of binding libc.
/// This is an advisory "will this preset fit" hint for the picker, not the
/// daemon's authoritative llama.cpp admission check at load time.
///
/// # Errors
///
/// Returns [`ModelDownloadFailure`] when the probe is unsupported, fails, or
/// its output does not parse as a positive byte count.
#[cfg(target_os = "macos")]
pub(crate) fn host_memory_total_bytes() -> Result<u64, ModelDownloadFailure> {
    let recovery = "Retry the host memory probe.";
    let output = std::process::Command::new("sysctl")
        .args(["-n", "hw.memsize"])
        .output()
        .map_err(|error| {
            ModelDownloadFailure::new(
                format!("Pam could not probe host memory: {error}"),
                recovery,
            )
        })?;
    if !output.status.success() {
        return Err(ModelDownloadFailure::new(
            "Pam's host memory probe exited with an error.",
            recovery,
        ));
    }
    std::str::from_utf8(&output.stdout)
        .ok()
        .and_then(|text| text.trim().parse::<u64>().ok())
        .filter(|bytes| *bytes > 0)
        .ok_or_else(|| {
            ModelDownloadFailure::new(
                "Pam could not parse the host memory probe output.",
                recovery,
            )
        })
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn host_memory_total_bytes() -> Result<u64, ModelDownloadFailure> {
    Err(ModelDownloadFailure::new(
        "Host memory probing is only implemented on macOS.",
        "Run Pam on macOS to see host memory.",
    ))
}
