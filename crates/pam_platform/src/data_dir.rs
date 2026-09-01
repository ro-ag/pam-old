//! Per-user data-directory resolution.
//!
//! Every durable per-user path Pam opens — `state.sqlite3`, `evidence/blobs`,
//! `callers/`, `logs/`, `runtime/` and `.pam/flows` — hangs off a single
//! `ProjectDirs::from("dev", "pam", "pam")`, which the `directories` crate
//! composes per platform: `dev.pam.pam` under `~/Library/Application Support`
//! on macOS, `pam` under the XDG data home on Linux, and `pam\pam` under the
//! app-data roots on Windows. The triple is constructed here and nowhere else,
//! so `identity` and `endpoint` cannot disagree about where a user's state
//! lives.
//!
//! `data_local_dir` diverges from `data_dir` on Windows alone; on macOS and
//! Linux the two name the same path.

use directories::ProjectDirs;

const QUALIFIER: &str = "dev";
const ORGANIZATION: &str = "pam";
const APPLICATION: &str = "pam";

/// Returns Pam's per-user project directories, or `None` when the operating
/// system exposes no home directory for the current process.
///
/// Resolution is pure: it composes paths and never creates, moves, or renames
/// anything on disk.
pub(crate) fn project_dirs() -> Option<ProjectDirs> {
    ProjectDirs::from(QUALIFIER, ORGANIZATION, APPLICATION)
}
