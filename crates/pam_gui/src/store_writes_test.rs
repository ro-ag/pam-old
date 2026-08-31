use std::path::PathBuf;

use pam_core::{CallerId, ContentDigest, IdempotencyKey, ProjectId, RequestId};
use pam_model::{GgufMetadata, LicenseSnapshot, ModelKey, ModelSource, RegisteredModel};
use pam_platform::DaemonRuntimeState;
use pam_protocol::RequestEnvelope;
use sha2::{Digest as _, Sha256};

use crate::store_writes::{owns_store, registration_of};

fn digest(seed: &str) -> ContentDigest {
    ContentDigest::from_sha256(Sha256::digest(seed.as_bytes()).into())
}

fn model(source: ModelSource) -> RegisteredModel {
    RegisteredModel {
        key: ModelKey::new("byteshape", "qwen3.6-q4ks").unwrap(),
        path: PathBuf::from("/models/byteshape/qwen3.6-q4ks.gguf"),
        digest: digest("artifact"),
        size_bytes: 17_456_012_448,
        gguf: GgufMetadata {
            version: 3,
            tensor_count: 579,
            metadata_kv_count: 27,
            architecture: Some("qwen3".to_owned()),
            model_name: Some("Qwen3".to_owned()),
            license: Some("apache-2.0".to_owned()),
        },
        license: LicenseSnapshot::new(
            "apache-2.0",
            "https://example.test/license",
            digest("notice"),
        )
        .unwrap(),
        source,
        registered_at_ms: 1_777_000_000_000,
    }
}

/// A running daemon owns the store; so does a lock nobody can read, because an
/// unknown owner is never a licence to become a second writer.
#[test]
fn only_an_observed_free_lock_leaves_the_store_unowned() {
    assert!(!owns_store(Some(DaemonRuntimeState::NotRunning)));
    assert!(owns_store(Some(DaemonRuntimeState::Running { pid: None })));
    assert!(owns_store(Some(DaemonRuntimeState::Running {
        pid: Some(4242)
    })));
    assert!(owns_store(None));
}

/// The wire form a routed registration sends must satisfy the protocol
/// contract for both provenance kinds, or the GUI could only ever register
/// with the daemon stopped.
#[test]
fn every_registered_model_survives_the_registration_contract() {
    for source in [
        ModelSource::Local,
        ModelSource::https("https://models.example.test/qwen3.6-q4ks.gguf").unwrap(),
    ] {
        let model = model(source);
        let registration = registration_of(&model);
        assert_eq!(registration.model, model.key.id());
        assert_eq!(registration.digest, model.digest.as_str());
        assert_eq!(registration.size_bytes, model.size_bytes);
        let request = RequestEnvelope::model_register(
            RequestId::from("gui-model-register"),
            CallerId::from("gui-caller"),
            ProjectId::daemon_scope(),
            IdempotencyKey::from("gui-model-register-key"),
            registration,
        )
        .expect("a registered model must be a valid registration");
        assert!(request.validate_model_request().is_ok());
    }
}
