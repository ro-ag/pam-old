use std::{
    fs,
    path::{Component, Path, PathBuf},
};

use serde::Deserialize;

use crate::{ModelError, ModelKey, model::valid_segment};

/// Validates one GGUF filename without accepting path components.
///
/// # Errors
///
/// Returns [`ModelError::InvalidFilename`] for a path, reserved segment, or a
/// filename without a `.gguf` extension.
pub fn validate_model_filename(filename: &str) -> Result<(), ModelError> {
    let path = Path::new(filename);
    if !valid_segment(filename)
        || path.components().count() != 1
        || !filename.rsplit_once('.').is_some_and(|(stem, extension)| {
            !stem.is_empty() && extension.eq_ignore_ascii_case("gguf")
        })
    {
        return Err(ModelError::InvalidFilename);
    }
    Ok(())
}

/// Returns `<root>/<vendor>/<filename>` without creating it. `root` is
/// caller-chosen: [`default_model_path`] passes `<home>/llm`, and PAM's
/// Settings-configured custom download directory passes its own root
/// directly.
///
/// # Errors
///
/// Returns an error when `root` is not an absolute Unicode path or the
/// filename is unsafe.
pub fn model_path_under(
    root: &Path,
    key: &ModelKey,
    filename: &str,
) -> Result<PathBuf, ModelError> {
    validate_absolute_unicode_path(root)?;
    validate_model_filename(filename)?;
    Ok(root.join(key.vendor()).join(filename))
}

/// PAM's default models root, `<home>/llm`, before any Settings override.
#[must_use]
pub fn default_models_dir(home: &Path) -> PathBuf {
    home.join("llm")
}

/// The one persisted preference that can move the models root. Only the field
/// this crate needs is read; the GUI owns writing the file.
#[derive(Debug, Default, Deserialize)]
struct PersistedModelsDir {
    #[serde(default)]
    models_dir: Option<String>,
}

/// The effective models download directory: the Settings-persisted override
/// when one is set, otherwise [`default_models_dir`].
///
/// This lives here, beside the path rules it feeds, because it has two
/// readers that must never disagree: the GUI, which shows and edits the
/// directory, and the daemon, which sweeps it and gates weights deletion on
/// containment in it. Infallible by design — a missing or corrupt preference
/// file falls back to the default rather than failing an unrelated read.
#[must_use]
pub fn effective_models_dir(data_dir: &Path, home: &Path) -> PathBuf {
    fs::read_to_string(data_dir.join("settings.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<PersistedModelsDir>(&text).ok())
        .and_then(|persisted| persisted.models_dir)
        .map_or_else(|| default_models_dir(home), PathBuf::from)
}

/// Returns `<home>/llm/<vendor>/<filename>` without creating it.
///
/// # Errors
///
/// Returns an error when the home path is not absolute Unicode or the filename
/// is unsafe.
pub fn default_model_path(
    home: &Path,
    key: &ModelKey,
    filename: &str,
) -> Result<PathBuf, ModelError> {
    model_path_under(&default_models_dir(home), key, filename)
}

/// Validates an absolute, non-parent-traversing Unicode path. Shared by the
/// default `<home>/llm` model root and PAM's Settings-configured custom
/// models directory.
///
/// # Errors
///
/// Returns [`ModelError::InvalidPath`] when `path` is relative, not valid
/// Unicode, or contains a `..` component.
pub fn validate_absolute_unicode_path(path: &Path) -> Result<(), ModelError> {
    if !path.is_absolute()
        || path.to_str().is_none()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(ModelError::InvalidPath);
    }
    Ok(())
}
