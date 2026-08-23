use std::{
    env, fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

use pam_core::{ContentDigest, ProjectId};
use pam_store::{EvidenceRedaction, EvidenceRetention, Store};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

use super::sonarqube::{DiscoverIssuesResponse, FetchQualityGateResponse};
use super::sonarqube_research::{
    QualityResearch, ResearchError, ResearchPersistence, ResearchStatus, research_quality_snapshot,
};
use super::{BoundedSummary, ConnectorOutput, Truth};

const TEST_PROJECT: &str = "sonarqube-research-test";
static NEXT_RESEARCH_STORE: AtomicU64 = AtomicU64::new(1);

fn digest(bytes: &[u8]) -> ContentDigest {
    ContentDigest::from_sha256(Sha256::digest(bytes).into())
}

fn canonical_handle(bytes: &[u8]) -> String {
    format!(
        "evidence://sonarqube/analysis-log/{}",
        digest(bytes).sha256_hex()
    )
}

fn research_store(name: &str) -> (PathBuf, Store) {
    let fixture_id = NEXT_RESEARCH_STORE.fetch_add(1, Ordering::Relaxed);
    let root = env::temp_dir().join(format!(
        "pam-connectors-{name}-{}-{fixture_id}",
        std::process::id()
    ));
    fs::create_dir_all(&root).unwrap();
    let store = Store::open(root.join("pam.sqlite3")).unwrap();
    (root, store)
}

fn persistence(store: &Store) -> ResearchPersistence {
    ResearchPersistence::new(
        store.clone(),
        ProjectId::from(TEST_PROJECT),
        EvidenceRetention::Project,
        EvidenceRedaction::Unredacted,
        42,
    )
    .unwrap()
}

async fn research(
    gate: &ConnectorOutput<FetchQualityGateResponse>,
    issues: &ConnectorOutput<DiscoverIssuesResponse>,
    analysis_log: Option<&[u8]>,
) -> Result<QualityResearch, ResearchError> {
    let (root, store) = research_store("sonarqube-research");
    let persistence = persistence(&store);
    let result = research_quality_snapshot(gate, issues, analysis_log, &persistence).await;
    drop(persistence);
    store.shutdown().await.unwrap();
    fs::remove_dir_all(root).unwrap();
    result
}

fn wrap<T>(value: T, complete: bool) -> ConnectorOutput<T> {
    let truth = if complete {
        Truth::Complete
    } else {
        Truth::Partial {
            reason: BoundedSummary::new("only one page was retained").unwrap(),
        }
    };
    ConnectorOutput::new(
        value,
        BoundedSummary::new("test connector output").unwrap(),
        truth,
        Vec::new(),
        Vec::new(),
    )
    .unwrap()
}

fn failing_gate(project: &str) -> FetchQualityGateResponse {
    serde_json::from_value(json!({
        "project": project,
        "status": "ERROR",
        "failed_conditions": [{
            "metricKey": "new_coverage",
            "status": "ERROR",
            "comparator": "LT",
            "errorThreshold": "85",
            "actualValue": "62.5"
        }]
    }))
    .unwrap()
}

fn passing_gate(project: &str) -> FetchQualityGateResponse {
    serde_json::from_value(json!({
        "project": project,
        "status": "OK",
        "failed_conditions": []
    }))
    .unwrap()
}

fn issue(key: &str, rule: &str, severity: &str, component: &str) -> Value {
    json!({
        "key": key,
        "rule": rule,
        "severity": severity,
        "component": component,
        "line": 5,
        "message": format!("fix {rule}"),
        "type": "CODE_SMELL"
    })
}

fn issues_response(project: &str, total: u64, issues: &[Value]) -> DiscoverIssuesResponse {
    serde_json::from_value(json!({
        "project": project,
        "total": total,
        "issues": issues
    }))
    .unwrap()
}

#[tokio::test]
async fn failed_gates_group_issues_and_anchor_conditions_to_the_analysis_log() {
    let gate = wrap(failing_gate("org:app"), true);
    let issues = wrap(
        issues_response(
            "org:app",
            4,
            &[
                issue("i1", "java:S1481", "MAJOR", "org:app:src/A.java"),
                issue("i2", "java:S1481", "MAJOR", "org:app:src/A.java"),
                issue("i3", "java:S1481", "MAJOR", "org:app:src/B.java"),
                issue("i4", "java:S2095", "BLOCKER", "org:app:src/C.java"),
            ],
        ),
        true,
    );
    let log: &[u8] = b"INFO: Analysis started\n\
QUALITY GATE STATUS: FAILED\n\
Condition new_coverage is 62.5 but must be at least 85\n\
INFO: Analysis finished\n";
    let research = research(&gate, &issues, Some(log)).await.unwrap();

    assert_eq!(research.status(), ResearchStatus::Failing);
    assert_eq!(research.project(), "org:app");
    assert_eq!(research.gate_status(), "ERROR");
    assert_eq!(research.gate_findings().len(), 1);
    let finding = &research.gate_findings()[0];
    assert!(finding.is_inference());
    assert_eq!(finding.condition().metric_key(), "new_coverage");
    let evidence = finding.evidence().expect("the log names the failed metric");
    assert_eq!(evidence.handle.as_str(), canonical_handle(log));
    let start = usize::try_from(evidence.offset).unwrap();
    let end = start + usize::try_from(evidence.length).unwrap();
    assert_eq!(
        &log[start..end],
        b"Condition new_coverage is 62.5 but must be at least 85\n"
    );

    assert_eq!(research.rule_groups().len(), 2);
    assert_eq!(research.rule_groups()[0].rule(), "java:S1481");
    assert_eq!(research.rule_groups()[0].count(), 3);
    assert_eq!(research.rule_groups()[0].severity(), "MAJOR");
    assert_eq!(
        research.rule_groups()[0].components(),
        ["org:app:src/A.java", "org:app:src/B.java"]
    );
    assert_eq!(research.rule_groups()[1].rule(), "java:S2095");
    assert_eq!(research.severity_totals()["MAJOR"], 3);
    assert_eq!(research.severity_totals()["BLOCKER"], 1);
    assert_eq!(research.file_groups()[0].component(), "org:app:src/A.java");
    assert_eq!(research.file_groups()[0].count(), 2);

    let compacted = research
        .compacted()
        .expect("the analysis log was compacted");
    assert_eq!(compacted.source.digest, digest(log));
    assert_eq!(compacted.source.handle.as_str(), canonical_handle(log));

    let manifest: Value = serde_json::from_slice(research.manifest().bytes()).unwrap();
    assert_eq!(manifest["schema_version"], "pam-sonarqube-research-v1");
    assert_eq!(manifest["project"], "org:app");
    assert_eq!(manifest["input_complete"], true);
    assert_eq!(manifest["status"], "failing");
    assert_eq!(manifest["gate"]["status"], "ERROR");
    assert_eq!(
        manifest["gate"]["findings"][0]["condition"]["metricKey"],
        "new_coverage"
    );
    assert_eq!(manifest["issues"]["total"], 4);
    assert_eq!(manifest["issues"]["retained"], 4);
    assert_eq!(
        manifest["analysis_log"]["source"]["handle"],
        canonical_handle(log)
    );
    assert_eq!(
        research.manifest().name(),
        "sonarqube-org-app-research.json"
    );
}

#[tokio::test]
async fn partial_inputs_never_become_failing_and_clean_snapshots_pass() {
    let gate = wrap(failing_gate("demo"), true);
    let issues = wrap(
        issues_response(
            "demo",
            40,
            &[issue("i1", "java:S1481", "MAJOR", "demo:src/A.java")],
        ),
        false,
    );
    let partial = research(&gate, &issues, None).await.unwrap();
    assert_eq!(partial.status(), ResearchStatus::Partial);
    assert!(partial.summary().as_str().starts_with("partial"));
    assert!(partial.compacted().is_none());

    let clean_gate = wrap(passing_gate("demo"), true);
    let clean_issues = wrap(issues_response("demo", 0, &[]), true);
    let clean = research(&clean_gate, &clean_issues, None).await.unwrap();
    assert_eq!(clean.status(), ResearchStatus::Passing);
    assert!(clean.gate_findings().is_empty());
    assert!(clean.rule_groups().is_empty());
    let manifest: Value = serde_json::from_slice(clean.manifest().bytes()).unwrap();
    assert_eq!(manifest["status"], "passing");
    assert_eq!(manifest["analysis_log"], Value::Null);
}

#[tokio::test]
async fn inconsistent_inputs_are_rejected_before_any_evidence_is_persisted() {
    let gate = wrap(failing_gate("org:app"), true);
    let issues = wrap(issues_response("other", 0, &[]), true);
    let log: &[u8] = b"QUALITY GATE STATUS: FAILED\n";
    let (root, store) = research_store("sonarqube-invalid");
    let persistence = persistence(&store);
    assert!(matches!(
        research_quality_snapshot(&gate, &issues, Some(log), &persistence).await,
        Err(ResearchError::ProjectMismatch)
    ));
    let handle = pam_core::EvidenceHandle::parse(canonical_handle(log)).unwrap();
    assert!(
        store
            .inspect_evidence(ProjectId::from(TEST_PROJECT), handle)
            .await
            .is_err()
    );
    drop(persistence);
    store.shutdown().await.unwrap();
    fs::remove_dir_all(root).unwrap();

    let issues = wrap(issues_response("org:app", 0, &[]), true);
    let oversized = vec![b'x'; super::sonarqube_research::MAX_ANALYSIS_LOG_BYTES + 1];
    assert!(matches!(
        research(&gate, &issues, Some(&oversized)).await,
        Err(ResearchError::LogTooLarge)
    ));
}
