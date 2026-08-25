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
        atomic::{AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use directories::BaseDirs;
use pam_model::{
    DownloadRequest, DownloadResponse, DownloadTransport, LicenseConsent, ModelDescriptor,
    ModelError, RegisteredModel, ReqwestDownloadTransport, TransferRequest, default_model_path,
    download_https,
};
use pam_platform::user_data_dir;
use pam_store::Store;

use crate::{model_import::parse_model_key, model_presets::ModelPreset};

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

impl From<ModelError> for ModelDownloadFailure {
    fn from(error: ModelError) -> Self {
        Self::new(
            error.to_string(),
            "Retry the download; if it keeps failing, check your network connection.",
        )
    }
}

/// The download manager's current, polled state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ModelDownloadStatusKind {
    Idle,
    Running,
    Complete,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ModelDownloadSnapshot {
    pub(crate) status: ModelDownloadStatusKind,
    pub(crate) preset_id: Option<String>,
    pub(crate) received_bytes: u64,
    pub(crate) total_bytes: u64,
    pub(crate) failure: Option<ModelDownloadFailure>,
}

impl ModelDownloadSnapshot {
    fn idle() -> Self {
        Self {
            status: ModelDownloadStatusKind::Idle,
            preset_id: None,
            received_bytes: 0,
            total_bytes: 0,
            failure: None,
        }
    }
}

struct ManagerState {
    snapshot: ModelDownloadSnapshot,
    received: Arc<AtomicU64>,
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
    pub(crate) fn start(self: Arc<Self>, preset: ModelPreset) -> Result<(), ModelDownloadFailure> {
        let home = BaseDirs::new()
            .map(|directories| directories.home_dir().to_path_buf())
            .ok_or_else(|| {
                ModelDownloadFailure::new(
                    "PAM could not resolve the user home directory.",
                    "Verify the operating system user profile, then retry.",
                )
            })?;
        self.start_with(preset, home, |received| {
            ReqwestDownloadTransport::secure().map(|transport| CountingTransport {
                inner: transport,
                received,
            })
        })
    }

    /// Starts a download using a caller-supplied home directory and transport
    /// factory. Kept generic so tests can point at a scratch directory and
    /// inject a fake transport instead of the real home and HTTPS.
    pub(crate) fn start_with<T, F>(
        self: Arc<Self>,
        preset: ModelPreset,
        home: PathBuf,
        make_transport: F,
    ) -> Result<(), ModelDownloadFailure>
    where
        T: DownloadTransport + 'static,
        F: FnOnce(Arc<AtomicU64>) -> Result<T, ModelError> + Send + 'static,
    {
        let received = {
            let mut state = self.state.lock().unwrap();
            if state.snapshot.status == ModelDownloadStatusKind::Running {
                return Err(ModelDownloadFailure::new(
                    "A model download is already running.",
                    "Wait for the current download to finish, then retry.",
                ));
            }
            let received = Arc::new(AtomicU64::new(0));
            state.received = Arc::clone(&received);
            state.snapshot = ModelDownloadSnapshot {
                status: ModelDownloadStatusKind::Running,
                preset_id: Some(preset.id.to_owned()),
                received_bytes: 0,
                total_bytes: preset.expected_size_bytes,
                failure: None,
            };
            received
        };
        tokio::spawn(async move {
            let outcome = match make_transport(received) {
                Ok(transport) => run_download(&transport, preset, &home).await,
                Err(error) => Err(error.into()),
            };
            self.finish(preset, outcome);
        });
        Ok(())
    }

    fn finish(&self, preset: ModelPreset, outcome: Result<RegisteredModel, ModelDownloadFailure>) {
        let mut state = self.state.lock().unwrap();
        state.snapshot = match outcome {
            Ok(registered) => ModelDownloadSnapshot {
                status: ModelDownloadStatusKind::Complete,
                preset_id: Some(preset.id.to_owned()),
                received_bytes: registered.size_bytes,
                total_bytes: preset.expected_size_bytes,
                failure: None,
            },
            Err(failure) => ModelDownloadSnapshot {
                status: ModelDownloadStatusKind::Failed,
                preset_id: Some(preset.id.to_owned()),
                received_bytes: state.received.load(Ordering::Relaxed),
                total_bytes: preset.expected_size_bytes,
                failure: Some(failure),
            },
        };
    }
}

async fn run_download<T: DownloadTransport>(
    transport: &T,
    preset: ModelPreset,
    home: &Path,
) -> Result<RegisteredModel, ModelDownloadFailure> {
    let key = parse_model_key(preset.model).map_err(|failure| ModelDownloadFailure {
        detail: failure.detail,
        recovery: failure.recovery,
    })?;
    let license = preset.license()?;
    let descriptor = ModelDescriptor::new(
        key,
        preset.file_name,
        preset.expected_digest(),
        preset.expected_size_bytes,
        license,
    )?;
    let consent = LicenseConsent::accept(&descriptor);
    let destination = default_model_path(home, &descriptor.key, &descriptor.filename)?;
    let registered_at_ms = now_ms()?;
    let request = DownloadRequest {
        descriptor,
        consent,
        source: preset.url.to_owned(),
        allowed_redirect_hosts: HUGGING_FACE_REDIRECT_HOSTS
            .iter()
            .map(|host| (*host).to_owned())
            .collect(),
        destination,
        registered_at_ms,
    };
    let registered = download_https(transport, request).await?;
    persist(registered).await
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

async fn persist(registered: RegisteredModel) -> Result<RegisteredModel, ModelDownloadFailure> {
    let recovery = "Verify the local PAM data store, then retry the download.";
    let state_path = user_data_dir()
        .map_err(|_| {
            ModelDownloadFailure::new(
                "PAM could not resolve its local data store.",
                "Verify the operating system user data directory, then retry.",
            )
        })?
        .join("state.sqlite3");
    let store = Store::open(state_path)
        .map_err(|error| ModelDownloadFailure::new(error.to_string(), recovery))?;
    let result = store.put_model(registered).await;
    let shutdown = store.shutdown().await;
    let registered =
        result.map_err(|error| ModelDownloadFailure::new(error.to_string(), recovery))?;
    shutdown.map_err(|error| ModelDownloadFailure::new(error.to_string(), recovery))?;
    Ok(registered)
}

/// Delegates to an inner transport, adding each response chunk's length into
/// a shared counter so a caller can poll download progress.
pub(crate) struct CountingTransport<T> {
    inner: T,
    received: Arc<AtomicU64>,
}

impl<T: DownloadTransport> DownloadTransport for CountingTransport<T> {
    type Response = CountingResponse<T::Response>;

    fn send(
        &self,
        request: TransferRequest,
    ) -> impl Future<Output = Result<Self::Response, ModelError>> + Send {
        let received = Arc::clone(&self.received);
        let sent = self.inner.send(request);
        async move {
            Ok(CountingResponse {
                response: sent.await?,
                received,
            })
        }
    }
}

pub(crate) struct CountingResponse<R> {
    response: R,
    received: Arc<AtomicU64>,
}

impl<R> CountingResponse<R> {
    #[cfg(test)]
    pub(crate) fn new(response: R, received: Arc<AtomicU64>) -> Self {
        Self { response, received }
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
        let next = self.response.next_chunk();
        async move {
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
                format!("PAM could not probe host memory: {error}"),
                recovery,
            )
        })?;
    if !output.status.success() {
        return Err(ModelDownloadFailure::new(
            "PAM's host memory probe exited with an error.",
            recovery,
        ));
    }
    std::str::from_utf8(&output.stdout)
        .ok()
        .and_then(|text| text.trim().parse::<u64>().ok())
        .filter(|bytes| *bytes > 0)
        .ok_or_else(|| {
            ModelDownloadFailure::new(
                "PAM could not parse the host memory probe output.",
                recovery,
            )
        })
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn host_memory_total_bytes() -> Result<u64, ModelDownloadFailure> {
    Err(ModelDownloadFailure::new(
        "Host memory probing is only implemented on macOS.",
        "Run PAM on macOS to see host memory.",
    ))
}
