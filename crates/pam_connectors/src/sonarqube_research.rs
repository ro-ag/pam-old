//! Deterministic, evidence-backed research over one `SonarQube` quality snapshot.

use std::{collections::BTreeMap, error::Error, fmt};

use pam_compact::{
    CompactError, CompactedLog, CompactionPolicy, LogMetadata, SourceEvidence, compact_log,
};
use pam_core::{ContentDigest, EvidenceHandle, EvidenceReference, ProjectId};
use pam_store::{EvidenceRedaction, EvidenceRetention, PutEvidence, Store};
use serde::Serialize;
use sha2::{Digest as _, Sha256};

use crate::{
    BoundedSummary, ConnectorOutput, ExactArtifact,
    github_diagnosis::MAX_DIAGNOSIS_PROJECT_ID_BYTES,
    sonarqube::{DiscoverIssuesResponse, FetchQualityGateResponse, GateCondition, SonarIssue},
};

pub const MAX_RESEARCH_RULE_GROUPS: usize = 64;
pub const MAX_RESEARCH_GROUP_COMPONENTS: usize = 10;
pub const MAX_RESEARCH_FILE_GROUPS: usize = 64;
pub const MAX_ANALYSIS_LOG_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_RESEARCH_MANIFEST_BYTES: usize = 256 * 1024;
pub const RESEARCH_SCHEMA_VERSION: &str = "pam-sonarqube-research-v1";

const SONARQUBE_LOG_EVIDENCE_PREFIX: &str = "evidence://sonarqube/analysis-log";
const SONARQUBE_LOG_MEDIA_TYPE: &str = "application/octet-stream";

/// Explicit durable destination for exact analysis log evidence used by research.
#[derive(Clone)]
pub struct ResearchPersistence {
    store: Store,
    project_id: ProjectId,
    retention: EvidenceRetention,
    redaction: EvidenceRedaction,
    captured_at_ms: u64,
}

impl ResearchPersistence {
    /// Binds research to one project-scoped durable evidence store and retention policy.
    ///
    /// # Errors
    ///
    /// Returns an error for an empty or oversized project identifier or a timestamp that cannot
    /// be represented by the durable store.
    pub fn new(
        store: Store,
        project_id: ProjectId,
        retention: EvidenceRetention,
        redaction: EvidenceRedaction,
        captured_at_ms: u64,
    ) -> Result<Self, ResearchError> {
        if project_id.as_str().is_empty()
            || project_id.as_str().len() > MAX_DIAGNOSIS_PROJECT_ID_BYTES
        {
            return Err(ResearchError::InvalidPersistenceProject);
        }
        if i64::try_from(captured_at_ms).is_err() {
            return Err(ResearchError::InvalidPersistenceTimestamp);
        }
        Ok(Self {
            store,
            project_id,
            retention,
            redaction,
            captured_at_ms,
        })
    }

    #[must_use]
    pub const fn project_id(&self) -> &ProjectId {
        &self.project_id
    }

    async fn persist(&self, bytes: &[u8]) -> Result<SourceEvidence, ResearchError> {
        let expected = canonical_source(bytes);
        let metadata = self
            .store
            .put_evidence(
                PutEvidence {
                    handle: expected.handle.clone(),
                    project_id: self.project_id.clone(),
                    media_type: SONARQUBE_LOG_MEDIA_TYPE.to_owned(),
                    retention: self.retention,
                    redaction: self.redaction,
                    bytes: bytes.to_vec(),
                },
                self.captured_at_ms,
            )
            .await
            .map_err(|_| ResearchError::EvidencePersistence)?;
        let size_bytes = u64::try_from(bytes.len()).expect("bounded evidence length fits u64");
        if metadata.handle != expected.handle
            || metadata.digest != expected.digest
            || metadata.project_id != self.project_id
            || metadata.size_bytes != size_bytes
            || metadata.media_type != SONARQUBE_LOG_MEDIA_TYPE
            || metadata.retention != self.retention
            || metadata.redaction != self.redaction
        {
            return Err(ResearchError::PersistedIdentityMismatch);
        }
        Ok(SourceEvidence {
            handle: metadata.handle,
            digest: metadata.digest,
        })
    }
}

impl fmt::Debug for ResearchPersistence {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResearchPersistence")
            .field("project_id", &self.project_id)
            .field("retention", &self.retention)
            .field("redaction", &self.redaction)
            .field("captured_at_ms", &self.captured_at_ms)
            .finish_non_exhaustive()
    }
}

fn canonical_source(bytes: &[u8]) -> SourceEvidence {
    let digest = ContentDigest::from_sha256(Sha256::digest(bytes).into());
    let handle = EvidenceHandle::parse(format!(
        "{SONARQUBE_LOG_EVIDENCE_PREFIX}/{}",
        digest.sha256_hex()
    ))
    .expect("a lowercase SHA-256 digest is a canonical evidence segment");
    SourceEvidence { handle, digest }
}

/// One failed quality gate condition, anchored to an analysis log line when one exists.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct GateFinding {
    condition: GateCondition,
    evidence: Option<EvidenceReference>,
    inference: bool,
}

impl GateFinding {
    #[must_use]
    pub const fn condition(&self) -> &GateCondition {
        &self.condition
    }

    #[must_use]
    pub const fn evidence(&self) -> Option<&EvidenceReference> {
        self.evidence.as_ref()
    }

    #[must_use]
    pub const fn is_inference(&self) -> bool {
        self.inference
    }
}

/// Unresolved issues sharing one rule, ordered by descending count then rule.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RuleGroup {
    rule: String,
    severity: String,
    issue_type: String,
    count: u64,
    components: Vec<String>,
    sample_message: String,
}

impl RuleGroup {
    #[must_use]
    pub fn rule(&self) -> &str {
        &self.rule
    }

    #[must_use]
    pub fn severity(&self) -> &str {
        &self.severity
    }

    #[must_use]
    pub fn issue_type(&self) -> &str {
        &self.issue_type
    }

    #[must_use]
    pub const fn count(&self) -> u64 {
        self.count
    }

    #[must_use]
    pub fn components(&self) -> &[String] {
        &self.components
    }

    #[must_use]
    pub fn sample_message(&self) -> &str {
        &self.sample_message
    }
}

/// Unresolved issues sharing one component, ordered by descending count then component.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct FileGroup {
    component: String,
    count: u64,
}

impl FileGroup {
    #[must_use]
    pub fn component(&self) -> &str {
        &self.component
    }

    #[must_use]
    pub const fn count(&self) -> u64 {
        self.count
    }
}

/// Completeness-aware outcome. `Partial` is never promoted to a passing or failing claim.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResearchStatus {
    Failing,
    Passing,
    Partial,
}

/// Pure research output and its canonical deterministic manifest artifact.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QualityResearch {
    status: ResearchStatus,
    summary: BoundedSummary,
    project: String,
    gate_status: String,
    gate_findings: Vec<GateFinding>,
    rule_groups: Vec<RuleGroup>,
    severity_totals: BTreeMap<String, u64>,
    file_groups: Vec<FileGroup>,
    compacted: Option<CompactedLog>,
    manifest: ExactArtifact,
}

impl QualityResearch {
    #[must_use]
    pub const fn status(&self) -> ResearchStatus {
        self.status
    }

    #[must_use]
    pub const fn summary(&self) -> &BoundedSummary {
        &self.summary
    }

    #[must_use]
    pub fn project(&self) -> &str {
        &self.project
    }

    #[must_use]
    pub fn gate_status(&self) -> &str {
        &self.gate_status
    }

    #[must_use]
    pub fn gate_findings(&self) -> &[GateFinding] {
        &self.gate_findings
    }

    #[must_use]
    pub fn rule_groups(&self) -> &[RuleGroup] {
        &self.rule_groups
    }

    #[must_use]
    pub const fn severity_totals(&self) -> &BTreeMap<String, u64> {
        &self.severity_totals
    }

    #[must_use]
    pub fn file_groups(&self) -> &[FileGroup] {
        &self.file_groups
    }

    #[must_use]
    pub const fn compacted(&self) -> Option<&CompactedLog> {
        self.compacted.as_ref()
    }

    #[must_use]
    pub const fn manifest(&self) -> &ExactArtifact {
        &self.manifest
    }
}

/// Persists, compacts, and deterministically groups one quality snapshot without model calls.
///
/// The optional analysis log is the scanner output for the analyzed project; when present it is
/// persisted as exact evidence, compacted through `pam_compact`, and searched for the failed
/// gate metrics so each finding carries an exact byte-range anchor.
///
/// # Errors
///
/// Returns an error for an inconsistent gate/issues pair, exceeded bounds, durable evidence
/// failure, compaction failure, or a manifest that cannot fit its fixed byte budget. Every
/// input is validated before the first durable write.
pub async fn research_quality_snapshot(
    gate: &ConnectorOutput<FetchQualityGateResponse>,
    issues: &ConnectorOutput<DiscoverIssuesResponse>,
    analysis_log: Option<&[u8]>,
    persistence: &ResearchPersistence,
) -> Result<QualityResearch, ResearchError> {
    let gate_response = gate.value();
    let issues_response = issues.value();
    if gate_response.project() != issues_response.project() {
        return Err(ResearchError::ProjectMismatch);
    }
    if analysis_log.is_some_and(|log| log.len() > MAX_ANALYSIS_LOG_BYTES) {
        return Err(ResearchError::LogTooLarge);
    }
    let input_complete = gate.truth().is_complete() && issues.truth().is_complete();

    let (source, compacted) = match analysis_log {
        Some(log) => {
            let source = persistence.persist(log).await?;
            let compacted = compact_log(
                &source,
                log,
                &LogMetadata::default(),
                &CompactionPolicy::default(),
            )
            .map_err(ResearchError::Compaction)?;
            (Some(source), Some(compacted))
        }
        None => (None, None),
    };

    let gate_findings = gate_response
        .failed_conditions()
        .iter()
        .map(|condition| GateFinding {
            condition: condition.clone(),
            evidence: source
                .as_ref()
                .zip(analysis_log)
                .and_then(|(source, log)| find_metric_line(source, log, condition.metric_key())),
            inference: true,
        })
        .collect::<Vec<_>>();

    let (mut rule_groups, severity_totals, mut file_groups) =
        group_issues(issues_response.issues());
    let groups_truncated = rule_groups.len() > MAX_RESEARCH_RULE_GROUPS
        || file_groups.len() > MAX_RESEARCH_FILE_GROUPS;
    rule_groups.truncate(MAX_RESEARCH_RULE_GROUPS);
    file_groups.truncate(MAX_RESEARCH_FILE_GROUPS);

    let status = if !input_complete || groups_truncated {
        ResearchStatus::Partial
    } else if gate_response.is_passing() && issues_response.issues().is_empty() {
        ResearchStatus::Passing
    } else {
        ResearchStatus::Failing
    };
    let summary = research_summary(
        status,
        gate_response.status(),
        gate_findings.len(),
        issues_response.issues().len(),
    )?;
    let manifest_bytes = manifest_bytes(
        gate_response,
        issues_response,
        input_complete,
        groups_truncated,
        status,
        &summary,
        &gate_findings,
        &rule_groups,
        &severity_totals,
        &file_groups,
        compacted.as_ref(),
    )?;
    let manifest_name = format!(
        "sonarqube-{}-research.json",
        gate_response.project().replace([':', '/'], "-")
    );
    let manifest = ExactArtifact::new(manifest_name, manifest_bytes)
        .map_err(|_| ResearchError::ManifestTooLarge)?;

    Ok(QualityResearch {
        status,
        summary,
        project: gate_response.project().to_owned(),
        gate_status: gate_response.status().to_owned(),
        gate_findings,
        rule_groups,
        severity_totals,
        file_groups,
        compacted,
        manifest,
    })
}

fn find_metric_line(
    source: &SourceEvidence,
    bytes: &[u8],
    metric: &str,
) -> Option<EvidenceReference> {
    let pattern = metric.as_bytes();
    let mut offset = 0_usize;
    while offset < bytes.len() {
        let relative_end = bytes[offset..].iter().position(|byte| *byte == b'\n');
        let end = relative_end.map_or(bytes.len(), |index| offset + index + 1);
        let line = &bytes[offset..end];
        if line
            .windows(pattern.len())
            .any(|window| window.eq_ignore_ascii_case(pattern))
        {
            return Some(EvidenceReference {
                handle: source.handle.clone(),
                offset: u64::try_from(offset).expect("usize fits in u64"),
                length: u64::try_from(end - offset).expect("usize fits in u64"),
            });
        }
        offset = end;
    }
    None
}

#[allow(clippy::type_complexity)]
fn group_issues(issues: &[SonarIssue]) -> (Vec<RuleGroup>, BTreeMap<String, u64>, Vec<FileGroup>) {
    let mut by_rule: BTreeMap<&str, RuleGroup> = BTreeMap::new();
    let mut severity_totals: BTreeMap<String, u64> = BTreeMap::new();
    let mut by_file: BTreeMap<&str, u64> = BTreeMap::new();
    for issue in issues {
        let group = by_rule.entry(issue.rule()).or_insert_with(|| RuleGroup {
            rule: issue.rule().to_owned(),
            severity: issue.severity().to_owned(),
            issue_type: issue.issue_type().to_owned(),
            count: 0,
            components: Vec::new(),
            sample_message: issue.message().to_owned(),
        });
        group.count += 1;
        if !group
            .components
            .iter()
            .any(|known| known == issue.component())
            && group.components.len() < MAX_RESEARCH_GROUP_COMPONENTS
        {
            group.components.push(issue.component().to_owned());
        }
        *severity_totals
            .entry(issue.severity().to_owned())
            .or_default() += 1;
        *by_file.entry(issue.component()).or_default() += 1;
    }
    let mut rule_groups = by_rule.into_values().collect::<Vec<_>>();
    rule_groups.sort_by(|left, right| {
        right
            .count
            .cmp(&left.count)
            .then_with(|| left.rule.cmp(&right.rule))
    });
    let mut file_groups = by_file
        .into_iter()
        .map(|(component, count)| FileGroup {
            component: component.to_owned(),
            count,
        })
        .collect::<Vec<_>>();
    file_groups.sort_by(|left, right| {
        right
            .count
            .cmp(&left.count)
            .then_with(|| left.component.cmp(&right.component))
    });
    (rule_groups, severity_totals, file_groups)
}

fn research_summary(
    status: ResearchStatus,
    gate_status: &str,
    condition_count: usize,
    issue_count: usize,
) -> Result<BoundedSummary, ResearchError> {
    let text = match status {
        ResearchStatus::Failing => format!(
            "SonarQube quality gate is {gate_status} with {condition_count} failed condition(s) \
             and {issue_count} unresolved issue(s)"
        ),
        ResearchStatus::Passing => {
            "SonarQube quality gate is passing with no unresolved issues".to_owned()
        }
        ResearchStatus::Partial => format!(
            "partial SonarQube research retained {condition_count} failed condition(s) and \
             {issue_count} unresolved issue(s)"
        ),
    };
    BoundedSummary::new(text).map_err(|_| ResearchError::SummaryTooLarge)
}

#[derive(Serialize)]
struct Manifest<'a> {
    schema_version: &'static str,
    project: &'a str,
    input_complete: bool,
    groups_truncated: bool,
    status: ResearchStatus,
    summary: &'a str,
    gate: ManifestGate<'a>,
    issues: ManifestIssues<'a>,
    analysis_log: Option<ManifestLog<'a>>,
}

#[derive(Serialize)]
struct ManifestGate<'a> {
    status: &'a str,
    findings: &'a [GateFinding],
}

#[derive(Serialize)]
struct ManifestIssues<'a> {
    total: u64,
    retained: u64,
    rule_groups: &'a [RuleGroup],
    severity_totals: &'a BTreeMap<String, u64>,
    file_groups: &'a [FileGroup],
}

#[derive(Serialize)]
struct ManifestLog<'a> {
    source: &'a SourceEvidence,
    algorithm_version: &'a str,
    policy_version: &'a str,
    policy_digest: &'a ContentDigest,
    source_byte_count: u64,
    retained_byte_count: u64,
    source_record_count: u64,
    retained_record_count: u64,
}

#[allow(clippy::too_many_arguments)]
fn manifest_bytes(
    gate: &FetchQualityGateResponse,
    issues: &DiscoverIssuesResponse,
    input_complete: bool,
    groups_truncated: bool,
    status: ResearchStatus,
    summary: &BoundedSummary,
    gate_findings: &[GateFinding],
    rule_groups: &[RuleGroup],
    severity_totals: &BTreeMap<String, u64>,
    file_groups: &[FileGroup],
    compacted: Option<&CompactedLog>,
) -> Result<Vec<u8>, ResearchError> {
    let manifest = Manifest {
        schema_version: RESEARCH_SCHEMA_VERSION,
        project: gate.project(),
        input_complete,
        groups_truncated,
        status,
        summary: summary.as_str(),
        gate: ManifestGate {
            status: gate.status(),
            findings: gate_findings,
        },
        issues: ManifestIssues {
            total: issues.total(),
            retained: u64::try_from(issues.issues().len()).expect("bounded issue count fits u64"),
            rule_groups,
            severity_totals,
            file_groups,
        },
        analysis_log: compacted.map(|compacted| ManifestLog {
            source: &compacted.source,
            algorithm_version: &compacted.algorithm_version,
            policy_version: &compacted.policy_version,
            policy_digest: &compacted.policy_digest,
            source_byte_count: compacted.source_byte_count,
            retained_byte_count: compacted.retained_byte_count,
            source_record_count: compacted.source_record_count,
            retained_record_count: compacted.retained_record_count,
        }),
    };
    let bytes = serde_json::to_vec(&manifest).map_err(|_| ResearchError::ManifestEncoding)?;
    if bytes.len() > MAX_RESEARCH_MANIFEST_BYTES {
        return Err(ResearchError::ManifestTooLarge);
    }
    Ok(bytes)
}

#[derive(Debug)]
pub enum ResearchError {
    InvalidPersistenceProject,
    InvalidPersistenceTimestamp,
    ProjectMismatch,
    LogTooLarge,
    EvidencePersistence,
    PersistedIdentityMismatch,
    Compaction(CompactError),
    SummaryTooLarge,
    ManifestEncoding,
    ManifestTooLarge,
}

impl fmt::Display for ResearchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPersistenceProject => {
                formatter.write_str("research evidence project identifier is invalid")
            }
            Self::InvalidPersistenceTimestamp => {
                formatter.write_str("research evidence timestamp is invalid")
            }
            Self::ProjectMismatch => {
                formatter.write_str("gate and issue inputs describe different SonarQube projects")
            }
            Self::LogTooLarge => formatter.write_str("analysis log exceeds its byte bound"),
            Self::EvidencePersistence => {
                formatter.write_str("exact research evidence could not be persisted")
            }
            Self::PersistedIdentityMismatch => {
                formatter.write_str("persisted research evidence identity is inconsistent")
            }
            Self::Compaction(source) => {
                write!(formatter, "analysis log compaction failed: {source}")
            }
            Self::SummaryTooLarge => formatter.write_str("research summary exceeds its bound"),
            Self::ManifestEncoding => formatter.write_str("research manifest encoding failed"),
            Self::ManifestTooLarge => formatter.write_str("research manifest exceeds its bound"),
        }
    }
}

impl Error for ResearchError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Compaction(source) => Some(source),
            _ => None,
        }
    }
}
