use std::{
    fs,
    path::{Path, PathBuf},
};

use super::data_dir::{DataDirMigration, migrate};

// The real pair (`dev.PAM.PAM` and `dev.pam.pam`) differs only by case, so on
// a case-insensitive volume it would collapse into one directory and the
// fixtures would stop testing anything. These names are distinct on every
// volume; the identity case is covered explicitly by the symlink test below.
struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "pam-data-dir-{label}-{}",
            uuid::Uuid::new_v4().simple()
        ));
        fs::create_dir_all(&path).expect("the test host must provide a writable temporary tree");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        drop(fs::remove_dir_all(&self.0));
    }
}

fn populate(directory: &Path) {
    fs::create_dir_all(directory.join("evidence/blobs")).unwrap();
    fs::create_dir_all(directory.join("callers")).unwrap();
    fs::write(directory.join("state.sqlite3"), b"durable").unwrap();
    fs::write(directory.join("evidence/blobs/one"), b"blob").unwrap();
}

#[test]
fn migration_moves_a_populated_legacy_directory_and_is_idempotent() {
    let scratch = Scratch::new("move");
    let legacy = scratch.path().join("legacy-data");
    let current = scratch.path().join("current-data");
    populate(&legacy);

    assert_eq!(
        migrate(&legacy, &current),
        DataDirMigration::Moved {
            from: legacy.clone(),
            to: current.clone(),
        }
    );
    assert!(!legacy.exists(), "the legacy directory must be gone");
    assert_eq!(fs::read(current.join("state.sqlite3")).unwrap(), b"durable");
    assert_eq!(
        fs::read(current.join("evidence/blobs/one")).unwrap(),
        b"blob"
    );
    assert!(current.join("callers").is_dir());

    assert_eq!(migrate(&legacy, &current), DataDirMigration::NotNeeded);
}

#[test]
fn migration_reports_the_move_in_its_audit_line() {
    let scratch = Scratch::new("audit");
    let legacy = scratch.path().join("legacy-data");
    let current = scratch.path().join("current-data");
    populate(&legacy);

    let line = migrate(&legacy, &current)
        .audit_line()
        .expect("a completed move must be recorded");

    assert!(line.contains(&legacy.display().to_string()));
    assert!(line.contains(&current.display().to_string()));
}

#[cfg(unix)]
#[test]
fn migration_is_a_no_op_when_both_paths_name_the_same_directory() {
    let scratch = Scratch::new("identity");
    let legacy = scratch.path().join("legacy-data");
    let current = scratch.path().join("current-data");
    populate(&legacy);
    // Stands in for a case-insensitive volume, where both names reach one
    // directory: string comparison sees two paths, identity sees one.
    std::os::unix::fs::symlink(&legacy, &current).unwrap();

    let outcome = migrate(&legacy, &current);

    assert_eq!(outcome, DataDirMigration::NotNeeded);
    assert!(outcome.audit_line().is_none(), "a no-op must stay silent");
    assert!(legacy.join("state.sqlite3").exists());
}

#[test]
fn migration_refuses_when_both_directories_exist_and_are_distinct() {
    let scratch = Scratch::new("conflict");
    let legacy = scratch.path().join("legacy-data");
    let current = scratch.path().join("current-data");
    populate(&legacy);
    fs::create_dir_all(&current).unwrap();
    fs::write(current.join("state.sqlite3"), b"current").unwrap();

    let outcome = migrate(&legacy, &current);

    assert_eq!(
        outcome,
        DataDirMigration::Conflict {
            legacy: legacy.clone(),
            current: current.clone(),
        }
    );
    let line = outcome
        .audit_line()
        .expect("a refused migration must be reported");
    assert!(line.contains(&legacy.display().to_string()));
    assert!(line.contains(&current.display().to_string()));
    // Neither side is merged into the other, and neither is overwritten.
    assert_eq!(fs::read(legacy.join("state.sqlite3")).unwrap(), b"durable");
    assert_eq!(fs::read(current.join("state.sqlite3")).unwrap(), b"current");
    assert!(!current.join("evidence").exists());
}

#[test]
fn migration_does_nothing_when_neither_directory_exists() {
    let scratch = Scratch::new("absent");
    let legacy = scratch.path().join("legacy-data");
    let current = scratch.path().join("current-data");

    assert_eq!(migrate(&legacy, &current), DataDirMigration::NotNeeded);
    assert!(
        !current.exists(),
        "no directory may be created speculatively"
    );
}
