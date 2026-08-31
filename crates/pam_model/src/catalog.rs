//! Reconciliation between the durable model registry and the weights on
//! disk, plus the provenance gate that decides whether PAM may delete a
//! registered artifact.
//!
//! Registration and verification both answer "is this one artifact still what
//! the registry says it is". This module answers the two questions that only
//! make sense across the whole catalog: what the registry names that disk no
//! longer has, and what disk holds that the registry never named. It also owns
//! the one rule that keeps PAM from destroying a user's file — PAM deletes
//! only what PAM downloaded, and only where PAM downloaded it to.

use std::path::{Component, Path, PathBuf};

use cap_std::fs::Dir;

use crate::{
    ModelError, ModelKey, ModelSource, RegisteredModel,
    acquisition::{open_absolute_directory, open_child_directory},
};

/// How deep beneath the models directory the sweep looks. Downloads land at
/// `<root>/<vendor>/<file>.gguf`, so two levels covers the layout PAM writes;
/// the extra levels catch a user's own nesting without ever becoming an
/// unbounded walk of an arbitrary directory the user pointed Settings at.
const MAX_SWEEP_DEPTH: usize = 4;

/// The most entries the sweep will visit before it stops descending. A
/// misconfigured models directory (a home directory, say) must not turn a
/// bounded report into an unbounded traversal.
const MAX_SWEEP_ENTRIES: usize = 20_000;

/// A registry row whose recorded path no longer resolves to a regular file.
///
/// The size is the one the registration recorded, not a size on disk: there
/// are no bytes left to measure, and reporting what the registry believes is
/// what makes the row explainable.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DanglingRegistration {
    pub key: ModelKey,
    pub path: PathBuf,
    pub size_bytes: u64,
}

/// A `.gguf` file under the models directory that no registry row points at.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OrphanWeights {
    pub path: PathBuf,
    pub size_bytes: u64,
}

/// One reconciliation of the registry against the models directory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelsDirectorySweep {
    /// The directory the sweep actually looked at, resolved.
    pub models_dir: PathBuf,
    pub dangling: Vec<DanglingRegistration>,
    pub orphans: Vec<OrphanWeights>,
    /// Every regular file under the models directory, summed. This is the
    /// directory's honest cost, so it counts in-flight download siblings and
    /// anything else the user keeps there, not only registered weights.
    pub total_bytes: u64,
}

/// Reconciles the registry against the models directory in both directions.
///
/// Dangling rows are found by resolution alone: a row is dangling when its
/// recorded path is no longer a regular, non-symlink file. That is the same
/// first step [`crate::revalidate_registered_model`] takes, without the hash,
/// so a sweep stays cheap over a catalog of very large artifacts.
///
/// Orphans are `.gguf` files under the models directory that no row names.
/// In-flight download siblings — `.<name>.pam-model.part`, its `.json`
/// checkpoint and its `.lock` — are never orphans: none of them ends in
/// `.gguf`, so the extension test excludes them by construction.
#[must_use]
pub fn sweep_models_directory(
    models_dir: &Path,
    registered: &[RegisteredModel],
) -> ModelsDirectorySweep {
    let models_dir = models_dir
        .canonicalize()
        .unwrap_or_else(|_| models_dir.to_path_buf());
    let dangling = registered
        .iter()
        .filter(|model| !resolves_to_regular_file(&model.path))
        .map(|model| DanglingRegistration {
            key: model.key.clone(),
            path: model.path.clone(),
            size_bytes: model.size_bytes,
        })
        .collect();
    let mut walk = DirectoryWalk::default();
    if let Ok(root) = open_absolute_directory(&models_dir, false) {
        walk.descend(&root, &models_dir, 0);
    }
    let mut orphans = walk
        .weights
        .into_iter()
        .filter(|(path, _)| !registered.iter().any(|model| &model.path == path))
        .map(|(path, size_bytes)| OrphanWeights { path, size_bytes })
        .collect::<Vec<_>>();
    orphans.sort_by(|left, right| left.path.cmp(&right.path));
    ModelsDirectorySweep {
        models_dir,
        dangling,
        orphans,
        total_bytes: walk.total_bytes,
    }
}

/// Why PAM will not delete a registered model's weights.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WeightsRefusal {
    /// The registration recorded [`ModelSource::Local`]: `pam model import`
    /// verified this GGUF where its owner already kept it, so PAM never
    /// downloaded the file and will not remove it.
    NotDownloadedByPam,
    /// The artifact is PAM-downloaded but no longer sits under the models
    /// directory in effect right now, so deleting it would reach outside the
    /// only root PAM owns.
    OutsideModelsDirectory,
    /// The path stopped being a plain regular file PAM can safely remove — a
    /// symlink, a directory, or an entry that vanished.
    Unsafe,
}

/// Decides whether PAM may delete one registered model's weights.
///
/// The gate has exactly two conditions and both must hold: the registration
/// says PAM downloaded the artifact (`https` provenance), and the artifact is
/// still inside the models directory in effect right now. An imported-in-place
/// file fails the first; a downloaded file the user has since moved elsewhere
/// fails the second.
///
/// # Errors
///
/// Returns the [`WeightsRefusal`] that explains which condition failed.
pub fn weights_deletion_allowed(
    models_dir: &Path,
    model: &RegisteredModel,
) -> Result<(), WeightsRefusal> {
    if matches!(model.source, ModelSource::Local) {
        return Err(WeightsRefusal::NotDownloadedByPam);
    }
    relative_to_models_dir(models_dir, &model.path).map(|_| ())
}

/// Deletes one registered model's weights and reports the bytes reclaimed.
///
/// Every step is confined to the models directory: the root is opened as a
/// capability directory, each path segment beneath it is opened without
/// following symlinks, and the final entry must be a regular, non-symlink
/// file. A symlink anywhere along the way is refused rather than followed, so
/// no removal can ever land outside the root the gate approved.
///
/// # Errors
///
/// Returns the [`WeightsRefusal`] that explains why nothing was deleted.
pub fn delete_registered_weights(
    models_dir: &Path,
    model: &RegisteredModel,
) -> Result<u64, WeightsRefusal> {
    if matches!(model.source, ModelSource::Local) {
        return Err(WeightsRefusal::NotDownloadedByPam);
    }
    let relative = relative_to_models_dir(models_dir, &model.path)?;
    let (parent, name) = open_confined(models_dir, &relative)?;
    let metadata = parent
        .symlink_metadata(&name)
        .map_err(|_| WeightsRefusal::Unsafe)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(WeightsRefusal::Unsafe);
    }
    let size_bytes = metadata.len();
    parent
        .remove_file(&name)
        .map_err(|_| WeightsRefusal::Unsafe)?;
    Ok(size_bytes)
}

/// The path's segments beneath the models directory, or the refusal that says
/// it is not beneath it at all.
///
/// Containment is decided on components of canonicalized paths, never on a
/// string prefix: `/models-old/x.gguf` must not read as inside `/models`.
fn relative_to_models_dir(models_dir: &Path, path: &Path) -> Result<PathBuf, WeightsRefusal> {
    let root = models_dir
        .canonicalize()
        .unwrap_or_else(|_| models_dir.to_path_buf());
    let candidate = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let relative = candidate
        .strip_prefix(&root)
        .map_err(|_| WeightsRefusal::OutsideModelsDirectory)?;
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(WeightsRefusal::OutsideModelsDirectory);
    }
    Ok(relative.to_path_buf())
}

/// Opens the directory holding `relative` beneath `root`, refusing to follow a
/// symlink at any level, and returns it with the final entry's name.
fn open_confined(root: &Path, relative: &Path) -> Result<(Dir, PathBuf), WeightsRefusal> {
    let mut segments = relative
        .components()
        .map(|component| PathBuf::from(component.as_os_str()))
        .collect::<Vec<_>>();
    let name = segments.pop().ok_or(WeightsRefusal::Unsafe)?;
    let mut directory =
        open_absolute_directory(root, false).map_err(|_| WeightsRefusal::OutsideModelsDirectory)?;
    for segment in segments {
        directory = open_child_directory(&directory, &segment, false)
            .map_err(|_| WeightsRefusal::Unsafe)?;
    }
    Ok((directory, name))
}

/// True when the path is a regular file that is not a symlink. A dangling row
/// is exactly the negation: the registry names something disk cannot serve.
fn resolves_to_regular_file(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|metadata| {
        metadata.is_file() && !metadata.file_type().is_symlink() && path.canonicalize().is_ok()
    })
}

/// Running totals for one bounded walk of the models directory.
#[derive(Default)]
struct DirectoryWalk {
    total_bytes: u64,
    weights: Vec<(PathBuf, u64)>,
    visited: usize,
}

impl DirectoryWalk {
    /// Adds one directory level to the totals. Symlinks are counted by
    /// nothing and never descended, so the walk cannot leave the root or
    /// double-count a file it already saw.
    fn descend(&mut self, directory: &Dir, path: &Path, depth: usize) {
        if depth >= MAX_SWEEP_DEPTH || self.visited >= MAX_SWEEP_ENTRIES {
            return;
        }
        let Ok(entries) = directory.entries() else {
            return;
        };
        for entry in entries.flatten() {
            if self.visited >= MAX_SWEEP_ENTRIES {
                return;
            }
            self.visited += 1;
            let name = PathBuf::from(entry.file_name());
            let Ok(metadata) = directory.symlink_metadata(&name) else {
                continue;
            };
            if metadata.file_type().is_symlink() {
                continue;
            }
            let child = path.join(&name);
            if metadata.is_dir() {
                if let Ok(nested) = open_child_directory(directory, &name, false) {
                    self.descend(&nested, &child, depth + 1);
                }
                continue;
            }
            if !metadata.is_file() {
                continue;
            }
            self.total_bytes = self.total_bytes.saturating_add(metadata.len());
            if is_weights_file(&name) {
                self.weights.push((child, metadata.len()));
            }
        }
    }
}

/// True for a `.gguf` artifact. The in-flight download siblings PAM writes
/// beside a destination — `.<name>.pam-model.part`, `.pam-model.json` and
/// `.pam-model.lock` — all end in something else, so they are never weights.
fn is_weights_file(name: &Path) -> bool {
    name.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("gguf"))
}

/// The user-facing sentence for one refusal. Kept beside the gate so the
/// daemon, the CLI and the GUI all refuse in exactly the same words.
#[must_use]
pub const fn weights_refusal_message(refusal: WeightsRefusal) -> &'static str {
    match refusal {
        WeightsRefusal::NotDownloadedByPam => {
            "PAM did not download this model, so it will not delete the file"
        }
        WeightsRefusal::OutsideModelsDirectory => {
            "this model's weights are no longer inside PAM's models directory"
        }
        WeightsRefusal::Unsafe => "this model's path is not a regular file PAM can remove",
    }
}

/// Classifies one [`ModelError`] from revalidation into a stable, reportable
/// health label. Verification never answers a bare boolean: every failure says
/// which specific thing stopped matching the registration.
#[must_use]
pub fn health_label(error: &ModelError) -> &'static str {
    match error {
        ModelError::SizeMismatch { .. } => "size_mismatch",
        ModelError::DigestMismatch => "digest_mismatch",
        ModelError::InvalidGguf | ModelError::UnsupportedGgufVersion(_) => "metadata_mismatch",
        ModelError::UnsafePath
        | ModelError::InvalidPath
        | ModelError::InvalidFilename
        | ModelError::NotRegularFile => "unsafe_path",
        ModelError::Io(error) if error.kind() == std::io::ErrorKind::NotFound => "path_missing",
        _ => "unreadable",
    }
}
