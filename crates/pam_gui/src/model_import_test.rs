use std::{fs, path::PathBuf, sync::Arc, time::Duration};

use pam_core::ContentDigest;
use sha2::{Digest, Sha256};
use tokio::sync::Notify;
use uuid::Uuid;

use crate::model_import::{
    ImportProgress, ModelImportManager, ModelImportParams, ModelImportSnapshot, ModelImportStage,
    ModelImportStatusKind, notice_digest, parse_model_key, run_model_inspect, verify_and_register,
};

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("pam-gui-model-{name}-{}", Uuid::new_v4()));
        fs::create_dir_all(&path).unwrap();
        Self(path.canonicalize().unwrap())
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub(crate) fn one_tensor_gguf() -> Vec<u8> {
    let mut bytes = b"GGUF".to_vec();
    bytes.extend_from_slice(&3_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u64.to_le_bytes());
    bytes.extend_from_slice(&0_u64.to_le_bytes());
    bytes.extend_from_slice(&6_u64.to_le_bytes());
    bytes.extend_from_slice(b"weight");
    bytes.extend_from_slice(&1_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u64.to_le_bytes());
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    bytes.extend_from_slice(&0_u64.to_le_bytes());
    while !bytes.len().is_multiple_of(32) {
        bytes.push(0);
    }
    bytes.resize(bytes.len() + 4, 0);
    bytes
}

fn params(path: PathBuf) -> ModelImportParams {
    ModelImportParams {
        model: "vendor/model".to_owned(),
        path,
        license_id: "Apache-2.0".to_owned(),
        license_url: "https://example.test/license".to_owned(),
        license_notice_text: "Apache License 2.0 notice text".to_owned(),
        allow_small: true,
    }
}

#[test]
fn rejects_models_under_the_recommended_floor_without_the_override() {
    let dir = TestDirectory::new("floor");
    let path = dir.0.join("tiny.gguf");
    fs::write(&path, one_tensor_gguf()).unwrap();
    let mut small = params(path.clone());
    small.allow_small = false;
    let failure = verify_and_register(small, 1, &ImportProgress::default()).unwrap_err();
    assert!(failure.detail.contains("recommended minimum"));
    assert!(failure.recovery.as_deref().unwrap().contains("Advanced"));
    // The same file registers once the override is granted.
    assert!(verify_and_register(params(path), 1, &ImportProgress::default()).is_ok());
}

#[test]
fn parses_only_the_vendor_name_form() {
    assert_eq!(
        parse_model_key("vendor/model").unwrap().id(),
        "vendor/model"
    );
    assert!(parse_model_key("no-slash").is_err());
    assert!(parse_model_key("bad segment/model").is_err());
    assert!(parse_model_key("/model").is_err());
}

#[test]
fn hashes_the_exact_accepted_notice_text() {
    let notice = "the exact accepted notice";
    let expected = ContentDigest::from_sha256(Sha256::digest(notice.as_bytes()).into());
    assert_eq!(notice_digest(notice), expected);
}

#[test]
fn computes_the_descriptor_itself_and_registers_the_gguf_in_place() {
    let directory = TestDirectory::new("register");
    let bytes = one_tensor_gguf();
    let path = directory.0.join("model.gguf");
    fs::write(&path, &bytes).unwrap();

    let registered = verify_and_register(params(path.clone()), 7, &ImportProgress::default())
        .unwrap_or_else(|failure| {
            panic!("import failed: {}", failure.detail);
        });

    assert_eq!(registered.key.id(), "vendor/model");
    assert_eq!(registered.path, path);
    assert_eq!(registered.size_bytes, u64::try_from(bytes.len()).unwrap());
    assert_eq!(
        registered.digest,
        ContentDigest::from_sha256(Sha256::digest(&bytes).into())
    );
    assert_eq!(
        registered.license.notice_digest(),
        &notice_digest("Apache License 2.0 notice text")
    );
    assert_eq!(registered.registered_at_ms, 7);
}

#[test]
fn rejects_a_non_gguf_file_with_a_bounded_failure() {
    let directory = TestDirectory::new("invalid");
    let path = directory.0.join("model.gguf");
    fs::write(&path, b"not a gguf at all").unwrap();

    let failure = verify_and_register(params(path), 7, &ImportProgress::default())
        .err()
        .unwrap();
    assert!(!failure.detail.is_empty());
    assert!(failure.recovery.is_some());
}

#[test]
fn reports_a_missing_file_without_raw_internals() {
    let directory = TestDirectory::new("missing");
    let failure = verify_and_register(
        params(directory.0.join("model.gguf")),
        7,
        &ImportProgress::default(),
    )
    .err()
    .unwrap();
    assert!(
        failure
            .detail
            .starts_with("Pam could not open the model file")
    );
    assert!(failure.recovery.is_some());
}

/// A FIFO with no writer would block forever in `open()`; the pre-open
/// `symlink_metadata` guard must reject it immediately instead of hanging.
#[cfg(unix)]
#[test]
fn rejects_a_fifo_instead_of_blocking_on_open() {
    let directory = TestDirectory::new("fifo");
    let path = directory.0.join("model.gguf");
    let status = std::process::Command::new("mkfifo")
        .arg(&path)
        .status()
        .expect("mkfifo must be available on macOS and Linux CI");
    assert!(status.success());

    let failure = verify_and_register(params(path), 1, &ImportProgress::default())
        .err()
        .unwrap();
    assert!(failure.detail.contains("regular file"));
    assert!(failure.recovery.is_some());
}

/// A symlink must be rejected even when it points at an otherwise-valid
/// GGUF: `symlink_metadata` never follows the final component.
#[cfg(unix)]
#[test]
fn rejects_a_symlink_even_to_a_valid_gguf() {
    let directory = TestDirectory::new("symlink");
    let target = directory.0.join("real.gguf");
    fs::write(&target, one_tensor_gguf()).unwrap();
    let link = directory.0.join("model.gguf");
    std::os::unix::fs::symlink(&target, &link).unwrap();

    let failure = verify_and_register(params(link), 1, &ImportProgress::default())
        .err()
        .unwrap();
    assert!(failure.detail.contains("regular file"));
    assert!(failure.recovery.is_some());
}

#[test]
fn hashing_advances_the_progress_counter_to_the_file_size() {
    let directory = TestDirectory::new("progress");
    let bytes = one_tensor_gguf();
    let path = directory.0.join("model.gguf");
    fs::write(&path, &bytes).unwrap();

    let progress = ImportProgress::default();
    verify_and_register(params(path), 1, &progress).unwrap();

    // Only the GUI-owned first hash reports into the counter;
    // `import_existing`'s internal re-hash never touches it.
    assert_eq!(progress.hashed_bytes(), u64::try_from(bytes.len()).unwrap());
}

async fn wait_for_terminal(manager: &Arc<ModelImportManager>) -> ModelImportSnapshot {
    for _ in 0..200 {
        let snapshot = manager.snapshot();
        if snapshot.status != ModelImportStatusKind::Running {
            return snapshot;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("import manager never reached a terminal state");
}

#[tokio::test]
async fn a_fresh_import_manager_is_idle() {
    let manager = ModelImportManager::new();
    let snapshot = manager.snapshot();
    assert_eq!(snapshot.status, ModelImportStatusKind::Idle);
    assert_eq!(snapshot.model, None);
    assert_eq!(snapshot.stage, None);
    assert_eq!(snapshot.hashed_bytes, 0);
    assert_eq!(snapshot.total_bytes, 0);
    assert!(snapshot.failure.is_none());
    assert!(!snapshot.calibrated);
}

#[tokio::test]
async fn a_completed_import_flags_whether_the_artifact_is_calibrated() {
    let directory = TestDirectory::new("calibrated");
    let path = directory.0.join("model.gguf");
    fs::write(&path, one_tensor_gguf()).unwrap();

    // The tiny test GGUF is no calibrated artifact: the import still
    // succeeds, with the flag false.
    let manager = ModelImportManager::new();
    Arc::clone(&manager)
        .start_with(params(path.clone()), |params, _progress| async move {
            verify_and_register(params, 1, &ImportProgress::default())
        })
        .unwrap();
    let complete = wait_for_terminal(&manager).await;
    assert_eq!(complete.status, ModelImportStatusKind::Complete);
    assert!(!complete.calibrated);

    // A registration whose digest and size match a calibrated artifact
    // exactly flips the flag.
    let artifact = &pam_model::CALIBRATED_ARTIFACTS[0];
    let mut registered = verify_and_register(params(path), 1, &ImportProgress::default()).unwrap();
    registered.digest = ContentDigest::parse(format!("sha256:{}", artifact.digest)).unwrap();
    registered.size_bytes = artifact.size_bytes;

    let manager = ModelImportManager::new();
    let registered_path = registered.path.clone();
    Arc::clone(&manager)
        .start_with(params(registered_path), move |_params, _progress| {
            let registered = registered.clone();
            async move { Ok(registered) }
        })
        .unwrap();
    let complete = wait_for_terminal(&manager).await;
    assert_eq!(complete.status, ModelImportStatusKind::Complete);
    assert!(complete.calibrated);
}

#[tokio::test]
async fn an_import_reports_live_stages_and_progress_through_to_complete() {
    let directory = TestDirectory::new("manager-complete");
    let bytes = one_tensor_gguf();
    let path = directory.0.join("model.gguf");
    fs::write(&path, &bytes).unwrap();
    let size = u64::try_from(bytes.len()).unwrap();
    // A real registration result for the injected runner to hand back.
    let registered =
        verify_and_register(params(path.clone()), 7, &ImportProgress::default()).unwrap();

    let manager = ModelImportManager::new();
    let step_done = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    Arc::clone(&manager)
        .start_with(params(path), {
            let step_done = Arc::clone(&step_done);
            let release = Arc::clone(&release);
            move |_params, progress| async move {
                progress.add_hashed(48);
                step_done.notify_one();
                release.notified().await;
                progress.begin_registering();
                step_done.notify_one();
                release.notified().await;
                Ok(registered)
            }
        })
        .unwrap();

    step_done.notified().await;
    let hashing = manager.snapshot();
    assert_eq!(hashing.status, ModelImportStatusKind::Running);
    assert_eq!(hashing.model.as_deref(), Some("vendor/model"));
    assert_eq!(hashing.stage, Some(ModelImportStage::Hashing));
    assert_eq!(hashing.hashed_bytes, 48);
    assert_eq!(hashing.total_bytes, size);

    release.notify_one();
    step_done.notified().await;
    assert_eq!(
        manager.snapshot().stage,
        Some(ModelImportStage::Registering)
    );

    release.notify_one();
    let complete = wait_for_terminal(&manager).await;
    assert_eq!(complete.status, ModelImportStatusKind::Complete);
    assert_eq!(complete.model.as_deref(), Some("vendor/model"));
    assert_eq!(complete.stage, None);
    assert_eq!(complete.hashed_bytes, size);
    assert_eq!(complete.total_bytes, size);
    assert!(complete.failure.is_none());
}

#[tokio::test]
async fn a_second_import_is_rejected_while_one_runs_and_a_failure_frees_the_slot() {
    let directory = TestDirectory::new("manager-single-flight");
    let path = directory.0.join("model.gguf");
    fs::write(&path, one_tensor_gguf()).unwrap();

    let manager = ModelImportManager::new();
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    Arc::clone(&manager)
        .start_with(params(path.clone()), {
            let entered = Arc::clone(&entered);
            let release = Arc::clone(&release);
            move |params, _progress| async move {
                entered.notify_one();
                release.notified().await;
                verify_and_register(params, 1, &ImportProgress::default())
            }
        })
        .unwrap();
    entered.notified().await;

    let rejected = Arc::clone(&manager)
        .start_with(params(path.clone()), |params, _progress| async move {
            verify_and_register(params, 1, &ImportProgress::default())
        })
        .unwrap_err();
    assert!(rejected.detail.contains("already running"));

    // Failing the run frees the single-flight slot for a fresh start.
    fs::remove_file(&path).unwrap();
    release.notify_one();
    let failed = wait_for_terminal(&manager).await;
    assert_eq!(failed.status, ModelImportStatusKind::Failed);
    assert_eq!(failed.model.as_deref(), Some("vendor/model"));
    assert!(failed.failure.is_some());

    fs::write(&path, one_tensor_gguf()).unwrap();
    Arc::clone(&manager)
        .start_with(params(path), |params, _progress| async move {
            verify_and_register(params, 1, &ImportProgress::default())
        })
        .unwrap();
    let restarted = wait_for_terminal(&manager).await;
    assert_eq!(restarted.status, ModelImportStatusKind::Complete);
}

/// `model_inspect` must reject a non-regular-file path fast, before ever
/// calling into `pam_model::inspect_model_file`.
#[cfg(unix)]
#[tokio::test]
async fn model_inspect_rejects_a_non_regular_file_fast() {
    let directory = TestDirectory::new("inspect-fifo");
    let path = directory.0.join("model.gguf");
    let status = std::process::Command::new("mkfifo")
        .arg(&path)
        .status()
        .expect("mkfifo must be available on macOS and Linux CI");
    assert!(status.success());

    let failure = tokio::time::timeout(Duration::from_secs(5), run_model_inspect(path))
        .await
        .expect("must reject before the inspect timeout, let alone hang")
        .err()
        .unwrap();
    assert!(failure.detail.contains("regular file"));
}
