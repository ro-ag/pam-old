//! GUI-owned local model import.
//!
//! The desktop shell is the whole surface: the user points PAM at a GGUF and
//! accepts its license in the panel, and PAM computes the file digest, the
//! notice digest, and the size itself before running the shared verification
//! and registration path. No terminal round-trip, ever.

use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
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
#[derive(Debug)]
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

fn hash_file(path: &Path) -> Result<(ContentDigest, u64), ModelImportFailure> {
    let open_recovery = "Point PAM at the downloaded .gguf file, then import again.";
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
    let (digest, size_bytes) = hash_file(&params.path)?;
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
) -> Result<RegisteredModel, ModelImportFailure> {
    let registered_at_ms = now_ms()?;
    let imported =
        tokio::task::spawn_blocking(move || verify_and_register(params, registered_at_ms))
            .await
            .map_err(|_| {
                ModelImportFailure::new(
                    "PAM could not complete model verification.",
                    "Retry the exact import.",
                )
            })??;
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
    let report = tokio::task::spawn_blocking(move || inspect_model_file(&path))
        .await
        .map_err(|_| ModelImportFailure::new("PAM could not complete model inspection.", recovery))?
        .map_err(|error| ModelImportFailure::new(error.to_string(), recovery))?;
    Ok(ModelInspectReport {
        file_name,
        size_bytes: report.size_bytes,
        architecture: report.metadata.architecture,
        model_name: report.metadata.model_name,
        below_floor: report.size_bytes < MIN_RECOMMENDED_MODEL_BYTES,
    })
}
