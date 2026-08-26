use std::{fs, path::PathBuf};

use pam_core::ContentDigest;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::model_import::{
    ModelImportParams, notice_digest, parse_model_key, run_model_inspect, verify_and_register,
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
    let failure = verify_and_register(small, 1).unwrap_err();
    assert!(failure.detail.contains("recommended minimum"));
    assert!(failure.recovery.as_deref().unwrap().contains("Advanced"));
    // The same file registers once the override is granted.
    assert!(verify_and_register(params(path), 1).is_ok());
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

    let registered = verify_and_register(params(path.clone()), 7).unwrap_or_else(|failure| {
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

    let failure = verify_and_register(params(path), 7).err().unwrap();
    assert!(!failure.detail.is_empty());
    assert!(failure.recovery.is_some());
}

#[test]
fn reports_a_missing_file_without_raw_internals() {
    let directory = TestDirectory::new("missing");
    let failure = verify_and_register(params(directory.0.join("model.gguf")), 7)
        .err()
        .unwrap();
    assert!(
        failure
            .detail
            .starts_with("PAM could not open the model file")
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

    let failure = verify_and_register(params(path), 1).err().unwrap();
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

    let failure = verify_and_register(params(link), 1).err().unwrap();
    assert!(failure.detail.contains("regular file"));
    assert!(failure.recovery.is_some());
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

    let failure = tokio::time::timeout(std::time::Duration::from_secs(5), run_model_inspect(path))
        .await
        .expect("must reject before the inspect timeout, let alone hang")
        .err()
        .unwrap();
    assert!(failure.detail.contains("regular file"));
}
