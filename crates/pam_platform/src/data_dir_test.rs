use std::ffi::OsStr;

use super::data_dir::project_dirs;

// Nothing reconciles a renamed per-user data directory any more, so the name
// is pinned by assertion instead. The literal is spelled out here rather than
// derived from the module's own constants: a blind rename or case sweep over
// `pam` has to fail in this test, not in a user's home directory, where it
// would silently orphan `state.sqlite3`, `callers/` and `evidence/blobs`.
#[test]
fn the_per_user_data_directory_name_is_pinned() {
    let project_dirs = project_dirs().expect("the test host must expose a per-user data directory");
    // macOS composes the whole reverse-DNS triple into one directory name;
    // Linux and Windows both end in the lowercased application name.
    let expected = if cfg!(target_os = "macos") {
        "dev.pam.pam"
    } else {
        "pam"
    };

    assert_eq!(
        project_dirs.data_dir().file_name().and_then(OsStr::to_str),
        Some(expected),
        "renaming the per-user data directory orphans every existing install's durable state"
    );
}
