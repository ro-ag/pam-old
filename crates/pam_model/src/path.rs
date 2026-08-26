use std::path::{Component, Path, PathBuf};

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
    model_path_under(&home.join("llm"), key, filename)
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
