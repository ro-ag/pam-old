//! GUI-owned local model import.
//!
//! The desktop shell is the whole surface: the user points PAM at a GGUF and
//! accepts its license in the panel, and PAM computes the file digest, the
//! notice digest, and the size itself before running the shared verification
//! and registration path. No terminal round-trip, ever.

use std::{
    fs::File,
    future::Future,
    io::Read,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use pam_core::ContentDigest;
use pam_model::{
    ImportRequest, LicenseConsent, LicenseSnapshot, ModelDescriptor, ModelError, ModelKey,
    RegisteredModel, import_existing, inspect_model_file,
};
use pam_platform::user_data_dir;
use pam_store::Store;
use sha2::{Digest, Sha256};

const HASH_BUFFER_BYTES: usize = 1024 * 1024;
const METADATA_RECOVERY: &str = "Check the GGUF file and license metadata, then import again.";
/// `model_import` hashes a multi-GB file twice (once in `hash_file`, once
/// inside `import_existing`'s re-verification), so the bound is generous.
const MODEL_IMPORT_TIMEOUT: Duration = Duration::from_mins(15);
/// `model_inspect` only reads the GGUF header, never the tensor data.
const MODEL_INSPECT_TIMEOUT: Duration = Duration::from_secs(30);
/// Qwen3-Coder-30B-A3B-Instruct at `Q4_K_S` (~17.5 GB) is the smallest model
/// PAM's flows were validated on; anything under this floor needs the
/// explicit Advanced override to register.
pub(crate) const MIN_RECOMMENDED_MODEL_BYTES: u64 = 17_000_000_000;

/// One complete GUI-owned import request, with the exact license notice text
/// the user accepted on screen.
pub struct ModelImportParams {
    /// Stable model identity in `vendor/name` form.
    pub model: String,
    /// Absolute path to the user-owned GGUF.
    pub path: PathBuf,
    pub license_id: String,
    pub license_url: String,
    pub license_notice_text: String,
    /// Accepts a model under the recommended minimum size anyway.
    pub allow_small: bool,
}

/// A bounded, user-facing import failure.
#[derive(Clone, Debug)]
pub(crate) struct ModelImportFailure {
    pub detail: String,
    pub recovery: Option<String>,
}

impl ModelImportFailure {
    fn new(detail: impl Into<String>, recovery: &str) -> Self {
        Self {
            detail: detail.into(),
            recovery: Some(recovery.to_owned()),
        }
    }
}

impl From<ModelError> for ModelImportFailure {
    fn from(error: ModelError) -> Self {
        Self::new(error.to_string(), METADATA_RECOVERY)
    }
}

/// Live progress for one import run, shared between the blocking hash thread
/// and the polled snapshot.
#[derive(Debug, Default)]
pub(crate) struct ImportProgress {
    hashed_bytes: AtomicU64,
    registering: AtomicBool,
}

impl ImportProgress {
    /// Test-only hook: production hashing reports through the raw counter
    /// `hash_file` receives.
    #[cfg(test)]
    pub(crate) fn add_hashed(&self, bytes: u64) {
        self.hashed_bytes.fetch_add(bytes, Ordering::Relaxed);
    }

    /// Marks the hand-off into `import_existing`, whose internal re-hash
    /// reports no progress: the run becomes indeterminate from here.
    pub(crate) fn begin_registering(&self) {
        self.registering.store(true, Ordering::Relaxed);
    }

    pub(crate) fn hashed_bytes(&self) -> u64 {
        self.hashed_bytes.load(Ordering::Relaxed)
    }
}

/// The import manager's current, polled state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ModelImportStatusKind {
    Idle,
    Running,
    Complete,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ModelImportStage {
    /// The GUI-owned first hash, with a live byte counter.
    Hashing,
    /// `pam_model::import_existing`'s own re-verification: indeterminate.
    Registering,
}

#[derive(Clone, Debug)]
pub(crate) struct ModelImportSnapshot {
    pub(crate) status: ModelImportStatusKind,
    pub(crate) model: Option<String>,
    pub(crate) stage: Option<ModelImportStage>,
    pub(crate) hashed_bytes: u64,
    pub(crate) total_bytes: u64,
    pub(crate) failure: Option<ModelImportFailure>,
}

impl ModelImportSnapshot {
    fn idle() -> Self {
        Self {
            status: ModelImportStatusKind::Idle,
            model: None,
            stage: None,
            hashed_bytes: 0,
            total_bytes: 0,
            failure: None,
        }
    }
}

struct ImportManagerState {
    snapshot: ModelImportSnapshot,
    progress: Arc<ImportProgress>,
}

/// Single-flight background import runner, polled for progress — the same
/// shape as `ModelDownloadManager`, so a multi-GB hash never runs under
/// `DesktopCore::command_gate`.
pub(crate) struct ModelImportManager {
    state: Mutex<ImportManagerState>,
}

impl ModelImportManager {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(ImportManagerState {
                snapshot: ModelImportSnapshot::idle(),
                progress: Arc::new(ImportProgress::default()),
            }),
        })
    }

    /// The current status. While an import is running, `hashed_bytes` and
    /// `stage` reflect the live counters rather than the values at start.
    pub(crate) fn snapshot(&self) -> ModelImportSnapshot {
        let state = self.state.lock().unwrap();
        let mut snapshot = state.snapshot.clone();
        if snapshot.status == ModelImportStatusKind::Running {
            snapshot.hashed_bytes = state.progress.hashed_bytes();
            snapshot.stage = Some(if state.progress.registering.load(Ordering::Relaxed) {
                ModelImportStage::Registering
            } else {
                ModelImportStage::Hashing
            });
        }
        snapshot
    }

    /// Starts one real background import.
    ///
    /// Takes an owned `Arc` (rather than `&self`) because `self: &Arc<Self>`
    /// is not a valid receiver type in stable Rust; the caller clones the
    /// `Arc` it already holds, which is a cheap refcount bump.
    ///
    /// # Errors
    ///
    /// Returns [`ModelImportFailure`] when another import is already running.
    pub(crate) fn start(
        self: Arc<Self>,
        params: ModelImportParams,
    ) -> Result<(), ModelImportFailure> {
        self.start_with(params, run_model_import)
    }

    /// Starts an import using the given runner. Kept generic so tests can
    /// observe a run mid-flight and finish it without touching the real
    /// user data store.
    pub(crate) fn start_with<F, Fut>(
        self: Arc<Self>,
        params: ModelImportParams,
        run: F,
    ) -> Result<(), ModelImportFailure>
    where
        F: FnOnce(ModelImportParams, Arc<ImportProgress>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<RegisteredModel, ModelImportFailure>> + Send + 'static,
    {
        // Advisory denominator for the progress bar; the authoritative size
        // check still happens inside the run. A stat failure here becomes an
        // indeterminate bar, not a refused import.
        let total_bytes = std::fs::metadata(&params.path).map_or(0, |metadata| metadata.len());
        let progress = {
            let mut state = self.state.lock().unwrap();
            if state.snapshot.status == ModelImportStatusKind::Running {
                return Err(ModelImportFailure::new(
                    "A model import is already running.",
                    "Wait for the current import to finish, then retry.",
                ));
            }
            let progress = Arc::new(ImportProgress::default());
            state.progress = Arc::clone(&progress);
            state.snapshot = ModelImportSnapshot {
                status: ModelImportStatusKind::Running,
                model: Some(params.model.clone()),
                stage: Some(ModelImportStage::Hashing),
                hashed_bytes: 0,
                total_bytes,
                failure: None,
            };
            progress
        };
        let model = params.model.clone();
        tokio::spawn(async move {
            let outcome = run(params, Arc::clone(&progress)).await;
            self.finish(model, outcome);
        });
        Ok(())
    }

    fn finish(&self, model: String, outcome: Result<RegisteredModel, ModelImportFailure>) {
        let mut state = self.state.lock().unwrap();
        state.snapshot = match outcome {
            Ok(registered) => ModelImportSnapshot {
                status: ModelImportStatusKind::Complete,
                model: Some(registered.key.id()),
                stage: None,
                hashed_bytes: registered.size_bytes,
                total_bytes: registered.size_bytes,
                failure: None,
            },
            Err(failure) => ModelImportSnapshot {
                status: ModelImportStatusKind::Failed,
                model: Some(model),
                stage: None,
                hashed_bytes: state.progress.hashed_bytes(),
                total_bytes: state.snapshot.total_bytes,
                failure: Some(failure),
            },
        };
    }
}

pub(crate) fn parse_model_key(value: &str) -> Result<ModelKey, ModelImportFailure> {
    let (vendor, name) = value.split_once('/').ok_or_else(|| {
        ModelImportFailure::new(
            "model identity must use the vendor/name form",
            "Name the model as vendor/name, e.g. qwen/qwen3-4b-instruct-q4.",
        )
    })?;
    ModelKey::new(vendor, name).map_err(Into::into)
}

/// Hashes the exact notice text the user accepted in the GUI.
pub(crate) fn notice_digest(notice: &str) -> ContentDigest {
    ContentDigest::from_sha256(Sha256::digest(notice.as_bytes()).into())
}

/// Rejects a path that is not a plain regular file (FIFO, socket, device, or
/// symlink) before the caller ever reaches `open(2)`. `open()` on a
/// writer-less FIFO or a character device like `/dev/zero` blocks or loops
/// forever, and `model_inspect` runs this while the caller holds
/// `DesktopCore::command_gate` (`desktop.rs`), which serializes every other
/// desktop command — so this check has to run first, not after a
/// failed/hanging open. Imports run off the gate, but a hung open would
/// still wedge the single-flight import slot for its full timeout.
///
/// `symlink_metadata` never follows the final path component, so a symlink
/// (to anything) is caught here as "not a regular file" without ever
/// resolving it. A missing path, or any other `symlink_metadata` error, is
/// deliberately *not* turned into a failure here — that's left for the
/// subsequent `open()` to report with its own familiar not-found/permission
/// message, so this guard only adds the one check `open()` can't make for
/// itself.
///
/// Residual TOCTOU: the file can still be swapped between this check and the
/// `open()` that follows. That gap is intentionally left open — the
/// authoritative gate is `pam_model::import_existing`'s CapFile-hardened
/// re-verification, which re-checks the actual bytes after this path ever
/// reaches the store.
fn reject_non_regular_file(path: &Path, recovery: &str) -> Result<(), ModelImportFailure> {
    if let Ok(metadata) = std::fs::symlink_metadata(path)
        && !metadata.is_file()
    {
        return Err(ModelImportFailure::new(
            "the model path must be a regular file, not a symlink, socket, FIFO, or device",
            recovery,
        ));
    }
    Ok(())
}

fn hash_file(
    path: &Path,
    progress: &AtomicU64,
) -> Result<(ContentDigest, u64), ModelImportFailure> {
    let open_recovery = "Point PAM at the downloaded .gguf file, then import again.";
    reject_non_regular_file(path, open_recovery)?;
    let mut file = File::open(path).map_err(|error| {
        ModelImportFailure::new(
            format!("PAM could not open the model file: {error}"),
            open_recovery,
        )
    })?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; HASH_BUFFER_BYTES];
    let mut size: u64 = 0;
    loop {
        let read = file.read(&mut buffer).map_err(|error| {
            ModelImportFailure::new(
                format!("PAM could not read the model file: {error}"),
                open_recovery,
            )
        })?;
        if read == 0 {
            break;
        }
        size += read as u64;
        progress.fetch_add(read as u64, Ordering::Relaxed);
        hasher.update(&buffer[..read]);
    }
    Ok((ContentDigest::from_sha256(hasher.finalize().into()), size))
}

/// Verifies and registers the GGUF in place through the shared import path.
///
/// Blocking: the model file is read twice — once to compute the descriptor
/// digest and size, and once inside `import_existing`, which re-verifies the
/// exact bytes against that descriptor.
pub(crate) fn verify_and_register(
    params: ModelImportParams,
    registered_at_ms: u64,
    progress: &ImportProgress,
) -> Result<RegisteredModel, ModelImportFailure> {
    let key = parse_model_key(&params.model)?;
    let filename = params
        .path
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .ok_or_else(|| {
            ModelImportFailure::new(
                "model path must end in a Unicode GGUF filename",
                METADATA_RECOVERY,
            )
        })?
        .to_owned();
    let (digest, size_bytes) = hash_file(&params.path, &progress.hashed_bytes)?;
    if size_bytes < MIN_RECOMMENDED_MODEL_BYTES && !params.allow_small {
        let size_gb = size_bytes / 100_000_000;
        let floor_gb = MIN_RECOMMENDED_MODEL_BYTES / 100_000_000;
        return Err(ModelImportFailure::new(
            format!(
                "This model is {}.{} GB — below PAM's recommended minimum of {}.{} GB, so results will fall short in real flows.",
                size_gb / 10,
                size_gb % 10,
                floor_gb / 10,
                floor_gb % 10
            ),
            "Pick a curated preset instead, or allow smaller models under Advanced and import again.",
        ));
    }
    let license = LicenseSnapshot::new(
        params.license_id,
        params.license_url,
        notice_digest(&params.license_notice_text),
    )?;
    let descriptor = ModelDescriptor::new(key, filename, digest, size_bytes, license)?;
    let consent = LicenseConsent::accept(&descriptor);
    progress.begin_registering();
    import_existing(ImportRequest {
        descriptor,
        consent,
        path: params.path,
        registered_at_ms,
    })
    .map_err(Into::into)
}

fn now_ms() -> Result<u64, ModelImportFailure> {
    let clock_recovery = "Correct the system clock, then import again.";
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| {
            ModelImportFailure::new(
                "The system clock cannot timestamp the model import.",
                clock_recovery,
            )
        })?
        .as_millis();
    u64::try_from(millis).map_err(|_| {
        ModelImportFailure::new(
            "The system clock cannot timestamp the model import.",
            clock_recovery,
        )
    })
}

/// Runs one complete import and persists the registration durably.
pub(crate) async fn run_model_import(
    params: ModelImportParams,
    progress: Arc<ImportProgress>,
) -> Result<RegisteredModel, ModelImportFailure> {
    let registered_at_ms = now_ms()?;
    let blocking = tokio::task::spawn_blocking(move || {
        verify_and_register(params, registered_at_ms, &progress)
    });
    let imported = match tokio::time::timeout(MODEL_IMPORT_TIMEOUT, blocking).await {
        Ok(Ok(result)) => result?,
        Ok(Err(_)) => {
            return Err(ModelImportFailure::new(
                "PAM could not complete model verification.",
                "Retry the exact import.",
            ));
        }
        // ponytail: tokio::time::timeout only stops *waiting* — it cannot
        // cancel a spawn_blocking thread mid-syscall. On expiry the blocking
        // thread leaks, still parked in open()/read() on the offending path,
        // until that call eventually returns on its own (or never, for a
        // writer-less FIFO). Acceptable ceiling: real cancellation of
        // blocking IO would need the file opened with a cancellable/async
        // API, which hash_file's plain std::fs doesn't have. This timeout's
        // job is only to release the single-flight import slot so a new
        // import can start.
        Err(_) => {
            return Err(ModelImportFailure::new(
                "PAM could not complete model verification in time.",
                "Retry the exact import.",
            ));
        }
    };
    let store_recovery = "Verify the local PAM data store, then import again.";
    let state_path = user_data_dir()
        .map_err(|_| {
            ModelImportFailure::new(
                "PAM could not resolve its local data store.",
                "Verify the operating system user data directory, then import again.",
            )
        })?
        .join("state.sqlite3");
    let store = Store::open(state_path)
        .map_err(|error| ModelImportFailure::new(error.to_string(), store_recovery))?;
    let result = store.put_model(imported).await;
    let shutdown = store.shutdown().await;
    let registered =
        result.map_err(|error| ModelImportFailure::new(error.to_string(), store_recovery))?;
    shutdown.map_err(|error| ModelImportFailure::new(error.to_string(), store_recovery))?;
    Ok(registered)
}

/// One pre-import preview of a candidate GGUF: its identity metadata and
/// whether it clears PAM's recommended size floor.
pub(crate) struct ModelInspectReport {
    pub file_name: String,
    pub size_bytes: u64,
    pub architecture: Option<String>,
    pub model_name: Option<String>,
    pub license: Option<String>,
    pub below_floor: bool,
}

/// Reads a candidate GGUF's bounded header and identity metadata without
/// hashing it, so the Control Center can preview a model before importing.
pub(crate) async fn run_model_inspect(
    path: PathBuf,
) -> Result<ModelInspectReport, ModelImportFailure> {
    let recovery = "Point PAM at a downloaded .gguf file.";
    let file_name = path
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .map(str::to_owned)
        .ok_or_else(|| {
            ModelImportFailure::new("model path must end in a Unicode filename", recovery)
        })?;
    reject_non_regular_file(&path, recovery)?;
    let blocking = tokio::task::spawn_blocking(move || inspect_model_file(&path));
    let report = match tokio::time::timeout(MODEL_INSPECT_TIMEOUT, blocking).await {
        Ok(Ok(result)) => {
            result.map_err(|error| ModelImportFailure::new(error.to_string(), recovery))?
        }
        Ok(Err(_)) => {
            return Err(ModelImportFailure::new(
                "PAM could not complete model inspection.",
                recovery,
            ));
        }
        // ponytail: see the matching comment in `run_model_import` — the
        // blocking thread can still leak past this timeout; it only bounds
        // how long `command_gate` stays held.
        Err(_) => {
            return Err(ModelImportFailure::new(
                "PAM could not complete model inspection in time.",
                recovery,
            ));
        }
    };
    Ok(ModelInspectReport {
        file_name,
        size_bytes: report.size_bytes,
        architecture: report.metadata.architecture,
        model_name: report.metadata.model_name,
        license: report.metadata.license,
        below_floor: report.size_bytes < MIN_RECOMMENDED_MODEL_BYTES,
    })
}
