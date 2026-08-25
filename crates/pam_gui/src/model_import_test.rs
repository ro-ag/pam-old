use std::{fs, path::PathBuf};

use pam_core::ContentDigest;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::model_import::{ModelImportParams, notice_digest, parse_model_key, verify_and_register};

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

fn one_tensor_gguf() -> Vec<u8> {
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
    }
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
