//! Per-user data-directory resolution, including the one-time move off the
//! legacy `dev.PAM.PAM` location.
//!
//! Pam's per-user directories used to be derived from
//! `ProjectDirs::from("dev", "Pam", "Pam")`. The `directories` crate composes
//! that triple differently per platform, so the lowercase rename is
//! asymmetric: Linux already lowercased the application name, Windows composes
//! `pam\pam` on a case-insensitive volume, and macOS composes `dev.pam.pam`
//! where `dev.PAM.PAM` used to sit. Only a case-sensitive macOS volume turns
//! the rename into a genuine move that would otherwise orphan
//! `state.sqlite3`, `evidence/blobs`, `callers/`, `logs/`, `runtime/` and
//! `.pam/flows` — and that is exactly the case a developer on a default
//! case-insensitive volume never observes.
//!
//! Only the data directory is migrated. `data_local_dir` diverges from
//! `data_dir` on Windows alone, where the legacy and current paths resolve to
//! the same case-insensitive directory and there is nothing to move; on macOS
//! and Linux the two are the same path.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::OnceLock,
};

use directories::ProjectDirs;

const QUALIFIER: &str = "dev";
const ORGANIZATION: &str = "pam";
const APPLICATION: &str = "pam";
const LEGACY_ORGANIZATION: &str = "Pam";
const LEGACY_APPLICATION: &str = "Pam";

/// Outcome of the single legacy data-directory migration attempted per
/// process.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DataDirMigration {
    /// Nothing to do: no legacy directory exists, or both names resolve to the
    /// same directory on a case-insensitive volume.
    NotNeeded,
    /// The legacy directory was renamed onto the current path.
    Moved { from: PathBuf, to: PathBuf },
    /// Both directories exist and are genuinely distinct. Nothing was moved,
    /// merged, or overwritten; which one wins is an operator decision.
    Conflict { legacy: PathBuf, current: PathBuf },
    /// The rename was attempted and failed. The legacy directory is intact.
    Failed {
        legacy: PathBuf,
        current: PathBuf,
        reason: String,
    },
}

impl DataDirMigration {
    /// Renders the audit line for this outcome, or `None` when the startup
    /// had nothing to report.
    #[must_use]
    pub fn audit_line(&self) -> Option<String> {
        match self {
            Self::NotNeeded => None,
            Self::Moved { from, to } => Some(format!(
                "moved the legacy per-user data directory {} to {}",
                from.display(),
                to.display()
            )),
            Self::Conflict { legacy, current } => Some(format!(
                "legacy per-user data directory {} and current directory {} both exist and are \
                 distinct; nothing was moved or merged, and the legacy directory is being ignored",
                legacy.display(),
                current.display()
            )),
            Self::Failed {
                legacy,
                current,
                reason,
            } => Some(format!(
                "failed to move the legacy per-user data directory {} to {}: {reason}",
                legacy.display(),
                current.display()
            )),
        }
    }
}

static MIGRATION: OnceLock<DataDirMigration> = OnceLock::new();

/// Completes the legacy per-user data-directory move, at most once per
/// process, and reports what happened.
///
/// The move is atomic (a rename within one parent directory), never merges
/// into an existing directory, and never overwrites, so a repeated call or a
/// second process observes [`DataDirMigration::NotNeeded`].
pub fn migrate_user_data_dir() -> &'static DataDirMigration {
    MIGRATION.get_or_init(|| {
        let Some(current) = current_project_dirs() else {
            return DataDirMigration::NotNeeded;
        };
        let Some(legacy) = legacy_project_dirs() else {
            return DataDirMigration::NotNeeded;
        };
        migrate(legacy.data_dir(), current.data_dir())
    })
}

/// Returns the current project directories with the legacy migration already
/// completed.
///
/// Every per-user path Pam opens resolves through here, so no caller can open
/// the durable store, the endpoint, or the ownership lock ahead of the move.
pub(crate) fn project_dirs() -> Option<ProjectDirs> {
    migrate_user_data_dir();
    current_project_dirs()
}

fn current_project_dirs() -> Option<ProjectDirs> {
    ProjectDirs::from(QUALIFIER, ORGANIZATION, APPLICATION)
}

fn legacy_project_dirs() -> Option<ProjectDirs> {
    ProjectDirs::from(QUALIFIER, LEGACY_ORGANIZATION, LEGACY_APPLICATION)
}

pub(crate) fn migrate(legacy: &Path, current: &Path) -> DataDirMigration {
    if !legacy.is_dir() {
        return DataDirMigration::NotNeeded;
    }
    // A case-insensitive volume resolves both names to one directory: the
    // rename is not just unnecessary there, it would be a self-rename. Decide
    // this by filesystem identity, never by comparing the strings.
    if same_directory(legacy, current) {
        return DataDirMigration::NotNeeded;
    }
    if current.exists() {
        return DataDirMigration::Conflict {
            legacy: legacy.to_path_buf(),
            current: current.to_path_buf(),
        };
    }
    if let Some(parent) = current.parent()
        && let Err(error) = fs::create_dir_all(parent)
    {
        return DataDirMigration::Failed {
            legacy: legacy.to_path_buf(),
            current: current.to_path_buf(),
            reason: error.to_string(),
        };
    }
    match fs::rename(legacy, current) {
        Ok(()) => DataDirMigration::Moved {
            from: legacy.to_path_buf(),
            to: current.to_path_buf(),
        },
        Err(error) => DataDirMigration::Failed {
            legacy: legacy.to_path_buf(),
            current: current.to_path_buf(),
            reason: error.to_string(),
        },
    }
}

fn same_directory(left: &Path, right: &Path) -> bool {
    let (Ok(left_real), Ok(right_real)) = (left.canonicalize(), right.canonicalize()) else {
        return false;
    };
    left_real == right_real || same_identity(&left_real, &right_real)
}

#[cfg(unix)]
fn same_identity(left: &Path, right: &Path) -> bool {
    use std::os::unix::fs::MetadataExt as _;

    match (fs::metadata(left), fs::metadata(right)) {
        (Ok(left), Ok(right)) => left.dev() == right.dev() && left.ino() == right.ino(),
        _ => false,
    }
}

#[cfg(not(unix))]
fn same_identity(_left: &Path, _right: &Path) -> bool {
    false
}
