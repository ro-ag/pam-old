//! GUI-owned Settings v1: visibility into where PAM keeps things, plus the
//! one persisted preference so far — a custom models download directory.
//!
//! Every function here takes its directories as parameters rather than
//! calling `pam_platform::user_data_dir()` itself, so tests point at a
//! scratch directory instead of the real user profile. [`crate::desktop`]
//! resolves the real data directory and home directory once, at the command
//! boundary, mirroring how [`crate::model_download`] resolves `home`.

use std::{
    fs,
    path::{Component, Path, PathBuf},
};

use serde::{Deserialize, Serialize};

const SETTINGS_FILE_NAME: &str = "settings.json";
const LOGS_DIR_NAME: &str = "logs";
// Every on-disk file PAM writes under `<data_dir>/logs`: the daemon's own
// rotating diagnostic log (`pam_daemon::logging::DaemonLog`, active file plus
// at most one size-rotated predecessor) and the GUI's best-effort capture of
// a GUI-launched daemon child's stderr (`desktop::daemon_stderr_capture`).
const DAEMON_LOG_FILES: [&str; 3] = ["daemon.log", "daemon.log.1", "daemon-stderr.log"];

/// A bounded, user-facing settings failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SettingsFailure {
    pub(crate) detail: String,
    pub(crate) recovery: Option<String>,
}

impl SettingsFailure {
    fn new(detail: impl Into<String>, recovery: impl Into<String>) -> Self {
        Self {
            detail: detail.into(),
            recovery: Some(recovery.into()),
        }
    }
}

/// The one preference Settings v1 persists, read and written as small JSON
/// next to the daemon's durable state.
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
struct PersistedSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    models_dir: Option<String>,
}

/// Today's complete Settings snapshot: every location the GUI shows.
///
/// There is no `flowsDir`: this branch has no global (project-independent)
/// flows helper to report a location for — `.pam/flows` lives per-project.
/// Add the field if a global flows home is introduced later.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AppSettingsSnapshot {
    pub(crate) models_dir: PathBuf,
    pub(crate) models_dir_is_default: bool,
    pub(crate) data_dir: PathBuf,
    pub(crate) logs_dir: PathBuf,
    pub(crate) logs_size_bytes: u64,
}

/// Resolves the user's home directory: the same probe
/// [`crate::model_download::ModelDownloadManager::start`] uses for the
/// default `<home>/llm` model root.
pub(crate) fn resolve_home() -> Result<PathBuf, SettingsFailure> {
    directories::BaseDirs::new()
        .map(|dirs| dirs.home_dir().to_path_buf())
        .ok_or_else(|| {
            SettingsFailure::new(
                "PAM could not resolve the user home directory.",
                "Verify the operating system user profile, then retry.",
            )
        })
}

fn settings_path(data_dir: &Path) -> PathBuf {
    data_dir.join(SETTINGS_FILE_NAME)
}

fn logs_dir_for(data_dir: &Path) -> PathBuf {
    data_dir.join(LOGS_DIR_NAME)
}

fn default_models_dir(home: &Path) -> PathBuf {
    home.join("llm")
}

/// Reads the persisted preferences, tolerating a missing or unreadable file:
/// Settings v1 has exactly one preference, and losing it is recoverable by
/// re-entering it — never worth failing an unrelated read for.
fn load_persisted(data_dir: &Path) -> PersistedSettings {
    fs::read_to_string(settings_path(data_dir))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn write_persisted(data_dir: &Path, persisted: &PersistedSettings) -> Result<(), SettingsFailure> {
    let recovery = "Verify PAM's local data directory is writable, then retry.";
    fs::create_dir_all(data_dir).map_err(|error| {
        SettingsFailure::new(
            format!("PAM could not reach its settings file: {error}"),
            recovery,
        )
    })?;
    let text = serde_json::to_string_pretty(persisted).map_err(|error| {
        SettingsFailure::new(
            format!("PAM could not encode its settings: {error}"),
            recovery,
        )
    })?;
    fs::write(settings_path(data_dir), text).map_err(|error| {
        SettingsFailure::new(
            format!("PAM could not write its settings file: {error}"),
            recovery,
        )
    })
}

/// The effective models download directory: the persisted override when one
/// is set, otherwise `<home>/llm`. Infallible by design (a missing or
/// corrupt preference file silently falls back) so a Settings read glitch
/// never blocks a model download.
pub(crate) fn effective_models_dir(data_dir: &Path, home: &Path) -> PathBuf {
    load_persisted(data_dir)
        .models_dir
        .map_or_else(|| default_models_dir(home), PathBuf::from)
}

fn logs_size_bytes(logs_dir: &Path) -> u64 {
    DAEMON_LOG_FILES
        .iter()
        .filter_map(|name| fs::metadata(logs_dir.join(name)).ok())
        .map(|metadata| metadata.len())
        .sum()
}

/// Builds today's settings snapshot without changing anything on disk.
pub(crate) fn snapshot(data_dir: &Path, home: &Path) -> AppSettingsSnapshot {
    let default_models_dir = default_models_dir(home);
    let models_dir = effective_models_dir(data_dir, home);
    let logs_dir = logs_dir_for(data_dir);
    AppSettingsSnapshot {
        models_dir_is_default: models_dir == default_models_dir,
        models_dir,
        data_dir: data_dir.to_path_buf(),
        logs_size_bytes: logs_size_bytes(&logs_dir),
        logs_dir,
    }
}

fn validate_custom_dir(raw: &str) -> Result<PathBuf, SettingsFailure> {
    let recovery = "Choose an absolute path PAM can create or already owns, then retry.";
    let trimmed = raw.trim();
    let path = PathBuf::from(trimmed);
    if trimmed.is_empty()
        || !path.is_absolute()
        || path.to_str().is_none()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(SettingsFailure::new(
            "The models directory must be an absolute path with no `..` segments.",
            recovery,
        ));
    }
    fs::create_dir_all(&path).map_err(|error| {
        SettingsFailure::new(
            format!("PAM could not create or reach that directory: {error}"),
            recovery,
        )
    })?;
    Ok(path)
}

/// Validates and persists a new custom models directory, or clears the
/// override back to the default when `new_dir` is `None`.
///
/// # Errors
///
/// Returns [`SettingsFailure`] when `new_dir` is relative, escapes with
/// `..`, or names a directory PAM cannot create.
pub(crate) fn update_models_dir(
    data_dir: &Path,
    home: &Path,
    new_dir: Option<String>,
) -> Result<AppSettingsSnapshot, SettingsFailure> {
    let mut persisted = load_persisted(data_dir);
    persisted.models_dir = new_dir
        .map(|raw| validate_custom_dir(&raw))
        .transpose()?
        .map(|path| path.to_string_lossy().into_owned());
    write_persisted(data_dir, &persisted)?;
    Ok(snapshot(data_dir, home))
}

/// Deletes the on-disk daemon log files, if any exist. Never touches the
/// durable state store; the daemon's in-memory ring buffer keeps serving the
/// debug console regardless of what is on disk.
///
/// # Errors
///
/// Returns [`SettingsFailure`] when a log file exists but cannot be removed.
pub(crate) fn delete_logs(
    data_dir: &Path,
    home: &Path,
) -> Result<AppSettingsSnapshot, SettingsFailure> {
    let logs_dir = logs_dir_for(data_dir);
    for name in DAEMON_LOG_FILES {
        let path = logs_dir.join(name);
        if let Err(error) = fs::remove_file(&path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            return Err(SettingsFailure::new(
                format!("PAM could not delete {name}: {error}"),
                "Close any program viewing the log file, then retry.",
            ));
        }
    }
    Ok(snapshot(data_dir, home))
}

/// True when `path` is exactly one of today's known Settings locations — the
/// only paths PAM will open in the system file manager for `reveal_path`.
pub(crate) fn is_known_location(snapshot: &AppSettingsSnapshot, path: &Path) -> bool {
    path == snapshot.models_dir || path == snapshot.data_dir || path == snapshot.logs_dir
}
