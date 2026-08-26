use std::{fs, path::PathBuf};

use uuid::Uuid;

use crate::settings::{
    delete_logs, effective_models_dir, is_known_location, snapshot, update_models_dir,
};

struct TestDirs {
    data_dir: PathBuf,
    home: PathBuf,
}

impl TestDirs {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("pam-gui-settings-{name}-{}", Uuid::new_v4()));
        let data_dir = root.join("data");
        let home = root.join("home");
        fs::create_dir_all(&data_dir).unwrap();
        fs::create_dir_all(&home).unwrap();
        Self { data_dir, home }
    }
}

impl Drop for TestDirs {
    fn drop(&mut self) {
        if let Some(root) = self.data_dir.parent() {
            let _ = fs::remove_dir_all(root);
        }
    }
}

#[test]
fn snapshot_defaults_to_home_llm_with_no_persisted_override() {
    let dirs = TestDirs::new("defaults");
    let snap = snapshot(&dirs.data_dir, &dirs.home);

    assert_eq!(snap.models_dir, dirs.home.join("llm"));
    assert!(snap.models_dir_is_default);
    assert_eq!(snap.data_dir, dirs.data_dir);
    assert_eq!(snap.flows_dir, dirs.data_dir.join(".pam/flows"));
    assert_eq!(snap.logs_dir, dirs.data_dir.join("logs"));
    assert_eq!(snap.logs_size_bytes, 0);
}

#[test]
fn update_models_dir_persists_and_reports_a_non_default_snapshot() {
    let dirs = TestDirs::new("update");
    let custom = dirs.home.join("custom-models");

    let snap = update_models_dir(
        &dirs.data_dir,
        &dirs.home,
        Some(custom.to_str().unwrap().to_owned()),
    )
    .unwrap();

    assert_eq!(snap.models_dir, custom);
    assert!(!snap.models_dir_is_default);
    assert!(custom.is_dir(), "the custom directory must be created");
    assert_eq!(effective_models_dir(&dirs.data_dir, &dirs.home), custom);
}

#[test]
fn update_models_dir_rejects_a_relative_path() {
    let dirs = TestDirs::new("relative");
    let failure = update_models_dir(
        &dirs.data_dir,
        &dirs.home,
        Some("relative/models".to_owned()),
    );
    assert!(failure.is_err());
    // The rejected update must not have persisted: reads still fall back to
    // the default, and no config file was written.
    assert!(
        effective_models_dir(&dirs.data_dir, &dirs.home).is_absolute(),
        "a rejected update must never leave a relative path in effect"
    );
}

#[test]
fn update_models_dir_rejects_parent_traversal() {
    let dirs = TestDirs::new("traversal");
    let failure = update_models_dir(&dirs.data_dir, &dirs.home, Some("/tmp/../etc".to_owned()));
    assert!(failure.is_err());
}

#[test]
fn update_models_dir_none_clears_a_previously_persisted_override() {
    let dirs = TestDirs::new("clear");
    let custom = dirs.home.join("custom-models");
    update_models_dir(
        &dirs.data_dir,
        &dirs.home,
        Some(custom.to_str().unwrap().to_owned()),
    )
    .unwrap();

    let cleared = update_models_dir(&dirs.data_dir, &dirs.home, None).unwrap();

    assert!(cleared.models_dir_is_default);
    assert_eq!(cleared.models_dir, dirs.home.join("llm"));
}

#[test]
fn delete_logs_removes_present_files_and_tolerates_absence() {
    let dirs = TestDirs::new("delete-logs");
    let logs_dir = dirs.data_dir.join("logs");
    fs::create_dir_all(&logs_dir).unwrap();
    fs::write(logs_dir.join("daemon.log"), b"one entry\n").unwrap();
    fs::write(logs_dir.join("daemon.log.1"), b"older entry\n").unwrap();

    let before = snapshot(&dirs.data_dir, &dirs.home);
    assert!(before.logs_size_bytes > 0);

    let after = delete_logs(&dirs.data_dir, &dirs.home).unwrap();
    assert_eq!(after.logs_size_bytes, 0);
    assert!(!logs_dir.join("daemon.log").exists());
    assert!(!logs_dir.join("daemon.log.1").exists());

    // Deleting again with nothing present must succeed, not error.
    assert!(delete_logs(&dirs.data_dir, &dirs.home).is_ok());
}

#[test]
fn is_known_location_matches_exactly_the_four_reported_directories() {
    let dirs = TestDirs::new("known-location");
    let snap = snapshot(&dirs.data_dir, &dirs.home);

    assert!(is_known_location(&snap, &snap.models_dir));
    assert!(is_known_location(&snap, &snap.data_dir));
    assert!(is_known_location(&snap, &snap.flows_dir));
    assert!(is_known_location(&snap, &snap.logs_dir));
    assert!(!is_known_location(&snap, &dirs.home));
}
