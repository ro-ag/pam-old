use std::{
    env, fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

use pam_core::{ContentDigest, ProjectId};
use pam_store::{EvidenceRedaction, EvidenceRetention, Store};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

use super::jenkins::CollectConsoleLogResponse;
use super::jenkins_diagnosis::{
    ConsoleDiagnosis, DiagnosisError, DiagnosisPersistence, DiagnosisStatus, FindingCategory,
    diagnose_console_log,
};
use super::{BoundedSummary, ConnectorOutput, ExactArtifact, Truth};

const TEST_PROJECT: &str = "jenkins-diagnosis-test";
static NEXT_DIAGNOSIS_STORE: AtomicU64 = AtomicU64::new(1);

fn digest(bytes: &[u8]) -> ContentDigest {
    ContentDigest::from_sha256(Sha256::digest(bytes).into())
}

fn canonical_handle(bytes: &[u8]) -> String {
    format!("evidence://jenkins/log/{}", digest(bytes).sha256_hex())
}

fn diagnosis_store(name: &str) -> (PathBuf, Store) {
    let fixture_id = NEXT_DIAGNOSIS_STORE.fetch_add(1, Ordering::Relaxed);
    let root = env::temp_dir().join(format!(
        "pam-connectors-{name}-{}-{fixture_id}",
        std::process::id()
    ));
    fs::create_dir_all(&root).unwrap();
    let store = Store::open(root.join("pam.sqlite3")).unwrap();
    (root, store)
}

fn persistence(store: &Store) -> DiagnosisPersistence {
    DiagnosisPersistence::new(
        store.clone(),
        ProjectId::from(TEST_PROJECT),
        EvidenceRetention::Project,
        EvidenceRedaction::Unredacted,
        42,
    )
    .unwrap()
}

async fn diagnose(
    collection: &ConnectorOutput<CollectConsoleLogResponse>,
    exact_log: &[u8],
) -> Result<ConsoleDiagnosis, DiagnosisError> {
    let (root, store) = diagnosis_store("jenkins-diagnosis");
    let persistence = persistence(&store);
    let result = diagnose_console_log(collection, exact_log, &persistence).await;
    drop(persistence);
    store.shutdown().await.unwrap();
    fs::remove_dir_all(root).unwrap();
    result
}

fn response(byte_len: usize) -> CollectConsoleLogResponse {
    serde_json::from_value(json!({
        "job": "folder/app",
        "build": 8,
        "artifact_name": "jenkins-folder-app-build-8.log",
        "byte_len": byte_len
    }))
    .unwrap()
}

fn collection(
    response: CollectConsoleLogResponse,
    complete: bool,
    payload: Vec<u8>,
) -> ConnectorOutput<CollectConsoleLogResponse> {
    let artifact = ExactArtifact::new("jenkins-folder-app-build-8.log", payload).unwrap();
    let truth = if complete {
        Truth::Complete
    } else {
        Truth::Partial {
            reason: BoundedSummary::new("the console log was unavailable").unwrap(),
        }
    };
    ConnectorOutput::new(
        response,
        BoundedSummary::new("collected test console log").unwrap(),
        truth,
        Vec::new(),
        vec![artifact],
    )
    .unwrap()
}

#[tokio::test]
async fn maven_compile_errors_are_classified_with_exact_hash_and_span_provenance() {
    let bytes: &[u8] = b"Started by user ci\n\
[INFO] Compiling 12 source files\n\
[ERROR] COMPILATION ERROR :\n\
[ERROR] /src/main/java/App.java:[5,9] cannot find symbol\n\
[INFO] BUILD FAILURE\n\
Finished: FAILURE\n";
    let collection = collection(response(bytes.len()), true, bytes.to_vec());
    let diagnosis = diagnose(&collection, bytes).await.unwrap();

    assert_eq!(diagnosis.status(), DiagnosisStatus::Diagnosed);
    assert_eq!(diagnosis.artifact_name(), "jenkins-folder-app-build-8.log");
    assert!(
        diagnosis
            .findings()
            .iter()
            .any(|finding| finding.category() == FindingCategory::Compilation)
    );
    for finding in diagnosis.findings() {
        assert!(finding.is_inference());
        assert_eq!(finding.evidence().handle.as_str(), canonical_handle(bytes));
        let start = usize::try_from(finding.evidence().offset).unwrap();
        let end = start + usize::try_from(finding.evidence().length).unwrap();
        assert!(end <= bytes.len());
        assert!(!&bytes[start..end].is_empty());
    }

    let compacted = diagnosis.compacted();
    assert_eq!(compacted.source.digest, digest(bytes));
    assert_eq!(compacted.source.handle.as_str(), canonical_handle(bytes));
    assert_eq!(
        compacted
            .fragments
            .iter()
            .map(|fragment| fragment.source.length)
            .sum::<u64>(),
        bytes.len() as u64
    );

    let manifest: Value = serde_json::from_slice(diagnosis.manifest().bytes()).unwrap();
    assert_eq!(manifest["schema_version"], "pam-jenkins-diagnosis-v1");
    assert_eq!(manifest["job"], "folder/app");
    assert_eq!(manifest["build_number"], 8);
    assert_eq!(manifest["input_complete"], true);
    assert_eq!(manifest["status"], "diagnosed");
    assert_eq!(manifest["log"]["source"]["handle"], canonical_handle(bytes));
    assert_eq!(
        diagnosis.manifest().name(),
        "jenkins-folder-app-build-8-diagnosis.json"
    );
}

#[tokio::test]
async fn junit_test_failures_are_classified() {
    let bytes: &[u8] = b"[INFO] Running com.example.AppTest\n\
[ERROR] Tests run: 3, Failures: 1, Errors: 0, Skipped: 0\n\
[ERROR] There are test failures.\n\
Finished: FAILURE\n";
    let collection = collection(response(bytes.len()), true, bytes.to_vec());
    let diagnosis = diagnose(&collection, bytes).await.unwrap();

    assert_eq!(diagnosis.status(), DiagnosisStatus::Diagnosed);
    assert!(
        diagnosis
            .findings()
            .iter()
            .any(|finding| finding.category() == FindingCategory::Tests)
    );
    assert!(
        diagnosis
            .findings()
            .iter()
            .any(|finding| finding.category() == FindingCategory::FailureOrUnknown)
    );
}

#[tokio::test]
async fn aborted_builds_are_classified() {
    let bytes: &[u8] = b"Started by timer\n\
Build was aborted\n\
Aborted by admin\n\
Finished: ABORTED\n";
    let collection = collection(response(bytes.len()), true, bytes.to_vec());
    let diagnosis = diagnose(&collection, bytes).await.unwrap();

    assert_eq!(diagnosis.status(), DiagnosisStatus::Diagnosed);
    let categories = diagnosis
        .findings()
        .iter()
        .map(super::jenkins_diagnosis::DiagnosisFinding::category)
        .collect::<Vec<_>>();
    assert_eq!(categories, vec![FindingCategory::Aborted]);
}

#[tokio::test]
async fn partial_input_never_becomes_diagnosed_and_benign_complete_input_is_unresolved() {
    let failing: &[u8] = b"[ERROR] There are test failures.\n";
    let partial_collection = collection(response(failing.len()), false, failing.to_vec());
    let partial = diagnose(&partial_collection, failing).await.unwrap();
    assert_eq!(partial.status(), DiagnosisStatus::Partial);
    assert_eq!(partial.findings().len(), 1);
    assert!(partial.summary().as_str().starts_with("partial"));

    let benign: &[u8] = b"Started by user ci\nAll checks passed\nFinished: SUCCESS\n";
    let complete_collection = collection(response(benign.len()), true, benign.to_vec());
    let unresolved = diagnose(&complete_collection, benign).await.unwrap();
    assert_eq!(unresolved.status(), DiagnosisStatus::Unresolved);
    assert!(unresolved.findings().is_empty());
}

#[tokio::test]
async fn inconsistent_collections_are_rejected_before_any_evidence_is_persisted() {
    let bytes: &[u8] = b"[ERROR] exact bytes\n";
    let collected = collection(response(bytes.len()), true, bytes.to_vec());

    let replacement: &[u8] = b"fatal: alteredby\n123";
    assert_eq!(replacement.len(), bytes.len());
    let (root, store) = diagnosis_store("jenkins-invalid-batch");
    let persistence = persistence(&store);
    assert!(matches!(
        diagnose_console_log(&collected, replacement, &persistence).await,
        Err(DiagnosisError::ArtifactPayloadMismatch)
    ));
    let handle = pam_core::EvidenceHandle::parse(canonical_handle(bytes)).unwrap();
    assert!(
        store
            .inspect_evidence(ProjectId::from(TEST_PROJECT), handle)
            .await
            .is_err()
    );
    drop(persistence);
    store.shutdown().await.unwrap();
    fs::remove_dir_all(root).unwrap();

    let short = &bytes[..bytes.len() - 1];
    assert!(matches!(
        diagnose(&collected, short).await,
        Err(DiagnosisError::ByteLengthMismatch)
    ));

    let wrong_name: CollectConsoleLogResponse = serde_json::from_value(json!({
        "job": "folder/app",
        "build": 8,
        "artifact_name": "not-canonical.log",
        "byte_len": bytes.len()
    }))
    .unwrap();
    let wrong_name = collection(wrong_name, true, bytes.to_vec());
    assert!(matches!(
        diagnose(&wrong_name, bytes).await,
        Err(DiagnosisError::ArtifactNameMismatch)
    ));
}
