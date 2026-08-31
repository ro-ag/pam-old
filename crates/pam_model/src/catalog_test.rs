use std::{fs, path::PathBuf};

use pam_core::ContentDigest;
use sha2::{Digest as _, Sha256};
use uuid::Uuid;

use super::{
    GgufMetadata, LicenseSnapshot, ModelError, ModelKey, ModelSource, RegisteredModel,
    WeightsRefusal, delete_registered_weights, effective_models_dir, health_label,
    revalidate_registered_model, sweep_models_directory, weights_deletion_allowed,
    weights_refusal_message,
};

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("pam-catalog-{name}-{}", Uuid::new_v4()));
        fs::create_dir_all(&path).unwrap();
        Self(path.canonicalize().unwrap())
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// The same minimal one-tensor GGUF `acquisition_test` builds: a valid bounded
/// header followed by a small zeroed payload.
fn gguf_bytes(payload_bytes: usize) -> Vec<u8> {
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
    bytes.resize(bytes.len() + payload_bytes, 0);
    bytes
}

/// Writes a GGUF at `<root>/<vendor>/<name>.gguf` and returns the registration
/// that exactly describes it.
fn write_registered(
    root: &TestDirectory,
    vendor: &str,
    name: &str,
    source: ModelSource,
) -> RegisteredModel {
    let bytes = gguf_bytes(8);
    let directory = root.0.join(vendor);
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join(format!("{name}.gguf"));
    fs::write(&path, &bytes).unwrap();
    registration(ModelKey::new(vendor, name).unwrap(), path, &bytes, source)
}

fn registration(
    key: ModelKey,
    path: PathBuf,
    bytes: &[u8],
    source: ModelSource,
) -> RegisteredModel {
    RegisteredModel {
        key,
        path,
        digest: ContentDigest::from_sha256(Sha256::digest(bytes).into()),
        size_bytes: u64::try_from(bytes.len()).unwrap(),
        gguf: GgufMetadata {
            version: 3,
            tensor_count: 1,
            metadata_kv_count: 0,
            architecture: None,
            model_name: None,
            license: None,
        },
        license: LicenseSnapshot::new(
            "Apache-2.0",
            "https://example.test/LICENSE",
            ContentDigest::from_sha256([9; 32]),
        )
        .unwrap(),
        source,
        registered_at_ms: 42,
    }
}

fn https_source() -> ModelSource {
    ModelSource::https("https://models.example/model.gguf").unwrap()
}

#[test]
fn a_registered_model_whose_bytes_are_untouched_verifies() {
    let root = TestDirectory::new("verify-healthy");
    let model = write_registered(&root, "vendor", "model", https_source());

    assert!(revalidate_registered_model(&model).is_ok());
}

#[test]
fn a_moved_a_truncated_and_a_drifted_model_each_report_their_own_failure() {
    let root = TestDirectory::new("verify-failures");

    // Moved: the registry still names a path disk no longer has.
    let moved = write_registered(&root, "vendor", "moved", https_source());
    fs::remove_file(&moved.path).unwrap();
    let failure = revalidate_registered_model(&moved).unwrap_err();
    assert!(matches!(failure, ModelError::Io(_)), "{failure:?}");
    assert_eq!(health_label(&failure), "path_missing");

    // Truncated: the file is still there but shorter than it was registered.
    let truncated = write_registered(&root, "vendor", "truncated", https_source());
    let mut bytes = fs::read(&truncated.path).unwrap();
    bytes.truncate(bytes.len() - 1);
    fs::write(&truncated.path, &bytes).unwrap();
    let failure = revalidate_registered_model(&truncated).unwrap_err();
    assert!(
        matches!(failure, ModelError::SizeMismatch { expected, actual }
            if expected == truncated.size_bytes && actual == truncated.size_bytes - 1),
        "{failure:?}"
    );
    assert_eq!(health_label(&failure), "size_mismatch");

    // Drifted: the same byte count, different bytes — only the hash sees it.
    let drifted = write_registered(&root, "vendor", "drifted", https_source());
    let mut bytes = fs::read(&drifted.path).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff;
    fs::write(&drifted.path, &bytes).unwrap();
    let failure = revalidate_registered_model(&drifted).unwrap_err();
    assert!(matches!(failure, ModelError::DigestMismatch), "{failure:?}");
    assert_eq!(health_label(&failure), "digest_mismatch");
}

#[test]
fn the_sweep_reports_both_directions_with_sizes_and_an_honest_directory_total() {
    let root = TestDirectory::new("sweep");
    let registered = write_registered(&root, "vendor", "model", https_source());

    // An orphan: a GGUF nobody registered.
    let orphan_path = root.0.join("vendor").join("stray.gguf");
    fs::write(&orphan_path, gguf_bytes(16)).unwrap();

    // A dangling row: registered, then the file left.
    let dangling = write_registered(&root, "vendor", "gone", https_source());
    fs::remove_file(&dangling.path).unwrap();

    // In-flight download siblings are not orphans and never will be.
    let partial = root.0.join("vendor").join(".model.gguf.pam-model.part");
    fs::write(&partial, vec![0_u8; 5]).unwrap();
    let checkpoint = root.0.join("vendor").join(".model.gguf.pam-model.json");
    fs::write(&checkpoint, b"{}").unwrap();
    let lock = root.0.join("vendor").join(".model.gguf.pam-model.lock");
    fs::write(&lock, b"").unwrap();

    let sweep = sweep_models_directory(&root.0, &[registered.clone(), dangling.clone()]);

    assert_eq!(sweep.models_dir, root.0);
    assert_eq!(sweep.dangling.len(), 1);
    assert_eq!(sweep.dangling[0].key, dangling.key);
    assert_eq!(sweep.dangling[0].path, dangling.path);
    assert_eq!(sweep.dangling[0].size_bytes, dangling.size_bytes);
    assert_eq!(sweep.orphans.len(), 1, "{:?}", sweep.orphans);
    assert_eq!(sweep.orphans[0].path, orphan_path);
    assert_eq!(
        sweep.orphans[0].size_bytes,
        fs::metadata(&orphan_path).unwrap().len()
    );

    // Every regular file counts toward what the directory costs, siblings
    // included: that is the number a user is deciding about.
    let expected_total = [&registered.path, &orphan_path, &partial, &checkpoint, &lock]
        .into_iter()
        .map(|path| fs::metadata(path).unwrap().len())
        .sum::<u64>();
    assert_eq!(sweep.total_bytes, expected_total);
}

#[test]
fn an_empty_models_directory_and_a_missing_one_both_sweep_to_nothing() {
    let root = TestDirectory::new("sweep-empty");
    let empty = sweep_models_directory(&root.0, &[]);
    assert_eq!(empty.total_bytes, 0);
    assert!(empty.orphans.is_empty());
    assert!(empty.dangling.is_empty());

    let absent = sweep_models_directory(&root.0.join("never-created"), &[]);
    assert_eq!(absent.total_bytes, 0);
    assert!(absent.orphans.is_empty());
}

#[test]
fn deleting_downloaded_weights_under_the_models_directory_reclaims_their_bytes() {
    let root = TestDirectory::new("delete-owned");
    let model = write_registered(&root, "vendor", "model", https_source());
    let expected = fs::metadata(&model.path).unwrap().len();

    assert_eq!(weights_deletion_allowed(&root.0, &model), Ok(()));
    assert_eq!(delete_registered_weights(&root.0, &model), Ok(expected));
    assert!(!model.path.exists());
}

#[test]
fn an_imported_in_place_model_is_refused_because_pam_never_downloaded_it() {
    let root = TestDirectory::new("delete-local");
    let model = write_registered(&root, "vendor", "model", ModelSource::Local);

    assert_eq!(
        weights_deletion_allowed(&root.0, &model),
        Err(WeightsRefusal::NotDownloadedByPam)
    );
    assert_eq!(
        delete_registered_weights(&root.0, &model),
        Err(WeightsRefusal::NotDownloadedByPam)
    );
    assert_eq!(
        weights_refusal_message(WeightsRefusal::NotDownloadedByPam),
        "PAM did not download this model, so it will not delete the file"
    );
    // The refusal never removes anything: the user's file is still theirs.
    assert!(model.path.exists());
}

#[test]
fn a_downloaded_model_that_moved_outside_the_models_directory_is_refused() {
    let root = TestDirectory::new("delete-outside");
    let elsewhere = TestDirectory::new("delete-elsewhere");
    let model = write_registered(&elsewhere, "vendor", "model", https_source());

    assert_eq!(
        weights_deletion_allowed(&root.0, &model),
        Err(WeightsRefusal::OutsideModelsDirectory)
    );
    assert_eq!(
        delete_registered_weights(&root.0, &model),
        Err(WeightsRefusal::OutsideModelsDirectory)
    );
    assert!(model.path.exists());
}

#[test]
fn a_sibling_directory_sharing_the_roots_name_prefix_is_still_outside_it() {
    let parent = TestDirectory::new("delete-prefix");
    let root = parent.0.join("models");
    let sibling = parent.0.join("models-archive");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&sibling).unwrap();
    let bytes = gguf_bytes(8);
    let path = sibling.join("model.gguf");
    fs::write(&path, &bytes).unwrap();
    let model = registration(
        ModelKey::new("vendor", "model").unwrap(),
        path.clone(),
        &bytes,
        https_source(),
    );

    // Containment is decided on path components, so a shared name prefix is
    // not containment.
    assert_eq!(
        delete_registered_weights(&root, &model),
        Err(WeightsRefusal::OutsideModelsDirectory)
    );
    assert!(path.exists());
}

#[cfg(unix)]
#[test]
fn a_symlink_out_of_the_models_directory_is_never_followed_or_removed() {
    let root = TestDirectory::new("delete-symlink");
    let outside = TestDirectory::new("delete-symlink-target");
    let bytes = gguf_bytes(8);
    let target = outside.0.join("precious.gguf");
    fs::write(&target, &bytes).unwrap();

    // A registry row pointing at a symlink that lives inside the models
    // directory but resolves outside it.
    let link = root.0.join("model.gguf");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let model = registration(
        ModelKey::new("vendor", "model").unwrap(),
        link.clone(),
        &bytes,
        https_source(),
    );

    // The canonical path leaves the root, so the gate refuses before any
    // removal is attempted, and the target survives.
    assert_eq!(
        delete_registered_weights(&root.0, &model),
        Err(WeightsRefusal::OutsideModelsDirectory)
    );
    assert!(target.exists());
    assert!(link.symlink_metadata().is_ok());

    // A symlinked directory on the way in is refused the same way.
    let linked_dir = root.0.join("vendor");
    std::os::unix::fs::symlink(&outside.0, &linked_dir).unwrap();
    let nested = registration(
        ModelKey::new("vendor", "precious").unwrap(),
        linked_dir.join("precious.gguf"),
        &bytes,
        https_source(),
    );
    assert_eq!(
        delete_registered_weights(&root.0, &nested),
        Err(WeightsRefusal::OutsideModelsDirectory)
    );
    assert!(target.exists());
}

#[cfg(unix)]
#[test]
fn the_sweep_never_descends_or_counts_a_symlink() {
    let root = TestDirectory::new("sweep-symlink");
    let outside = TestDirectory::new("sweep-symlink-target");
    fs::write(outside.0.join("elsewhere.gguf"), gguf_bytes(64)).unwrap();
    std::os::unix::fs::symlink(&outside.0, root.0.join("linked")).unwrap();
    std::os::unix::fs::symlink(outside.0.join("elsewhere.gguf"), root.0.join("linked.gguf"))
        .unwrap();

    let sweep = sweep_models_directory(&root.0, &[]);

    assert!(sweep.orphans.is_empty(), "{:?}", sweep.orphans);
    assert_eq!(sweep.total_bytes, 0);
}

#[test]
fn the_effective_models_directory_is_the_persisted_override_or_the_default() {
    let data_dir = TestDirectory::new("settings");
    let home = TestDirectory::new("home");

    assert_eq!(
        effective_models_dir(&data_dir.0, &home.0),
        home.0.join("llm")
    );

    fs::write(
        data_dir.0.join("settings.json"),
        r#"{"models_dir":"/opt/weights"}"#,
    )
    .unwrap();
    assert_eq!(
        effective_models_dir(&data_dir.0, &home.0),
        PathBuf::from("/opt/weights")
    );

    // A corrupt preference file must never fail an unrelated read.
    fs::write(data_dir.0.join("settings.json"), "not json").unwrap();
    assert_eq!(
        effective_models_dir(&data_dir.0, &home.0),
        home.0.join("llm")
    );
}
