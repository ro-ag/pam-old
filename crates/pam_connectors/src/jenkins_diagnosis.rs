//! Deterministic, evidence-backed diagnosis of one collected Jenkins console log.

use std::{error::Error, fmt};

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
    jenkins::{CollectConsoleLogResponse, MAX_CONSOLE_LOG_BYTES},
};

pub const MAX_DIAGNOSIS_FINDINGS: usize = 64;
pub const MAX_DIAGNOSIS_MANIFEST_BYTES: usize = 256 * 1024;
pub const DIAGNOSIS_SCHEMA_VERSION: &str = "pam-jenkins-diagnosis-v1";

const COMPILATION_PATTERNS: &[&[u8]] = &[
    b"compilation error",
    b"compilation failure",
    b"compilation failed",
    b"cannot find symbol",
    b"could not compile",
    b"undefined reference",
];
const TEST_PATTERNS: &[&[u8]] = &[
    b"there are test failures",
    b"there were failing tests",
    b"tests failed",
    b"test result: failed",
    b"assertionerror",
];
const CHECKOUT_PATTERNS: &[&[u8]] = &[
    b"error cloning remote repo",
    b"couldn't find any revision",
    b"failed to fetch from",
    b"gitexception",
    b"checkout failed",
];
const TIMEOUT_PATTERNS: &[&[u8]] = &[b"timed out", b"timeout", b"deadline exceeded"];
const AUTHORIZATION_PATTERNS: &[&[u8]] = &[
    b"permission denied",
    b"unauthorized",
    b"forbidden",
    b"authentication failed",
    b"access denied",
    b"invalid credentials",
];
const ABORTED_PATTERNS: &[&[u8]] = &[b"finished: aborted", b"build was aborted", b"aborted by"];
const FAILURE_OR_UNKNOWN_PATTERNS: &[&[u8]] = &[
    b"finished: failure",
    b"error:",
    b"fatal:",
    b"exception:",
    b"exit code",
    b"service unavailable",
    b"connection reset",
];
const JENKINS_LOG_EVIDENCE_PREFIX: &str = "evidence://jenkins/log";
const JENKINS_LOG_MEDIA_TYPE: &str = "application/octet-stream";

/// Explicit durable destination for exact console log evidence used by diagnosis.
#[derive(Clone)]
pub struct DiagnosisPersistence {
    store: Store,
    project_id: ProjectId,
    retention: EvidenceRetention,
    redaction: EvidenceRedaction,
    captured_at_ms: u64,
}

impl DiagnosisPersistence {
    /// Binds diagnosis to one project-scoped durable evidence store and retention policy.
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
    ) -> Result<Self, DiagnosisError> {
        if project_id.as_str().is_empty()
            || project_id.as_str().len() > MAX_DIAGNOSIS_PROJECT_ID_BYTES
        {
            return Err(DiagnosisError::InvalidPersistenceProject);
        }
        if i64::try_from(captured_at_ms).is_err() {
            return Err(DiagnosisError::InvalidPersistenceTimestamp);
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

    async fn persist(&self, bytes: &[u8]) -> Result<SourceEvidence, DiagnosisError> {
        let expected = canonical_source(bytes);
        let metadata = self
            .store
            .put_evidence(
                PutEvidence {
                    handle: expected.handle.clone(),
                    project_id: self.project_id.clone(),
                    media_type: JENKINS_LOG_MEDIA_TYPE.to_owned(),
                    retention: self.retention,
                    redaction: self.redaction,
                    bytes: bytes.to_vec(),
                },
                self.captured_at_ms,
            )
            .await
            .map_err(|_| DiagnosisError::EvidencePersistence)?;
        let size_bytes = u64::try_from(bytes.len()).expect("bounded evidence length fits u64");
        if metadata.handle != expected.handle
            || metadata.digest != expected.digest
            || metadata.project_id != self.project_id
            || metadata.size_bytes != size_bytes
            || metadata.media_type != JENKINS_LOG_MEDIA_TYPE
            || metadata.retention != self.retention
            || metadata.redaction != self.redaction
        {
            return Err(DiagnosisError::PersistedIdentityMismatch);
        }
        Ok(SourceEvidence {
            handle: metadata.handle,
            digest: metadata.digest,
        })
    }
}

impl fmt::Debug for DiagnosisPersistence {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DiagnosisPersistence")
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
        "{JENKINS_LOG_EVIDENCE_PREFIX}/{}",
        digest.sha256_hex()
    ))
    .expect("a lowercase SHA-256 digest is a canonical evidence segment");
    SourceEvidence { handle, digest }
}

/// A deliberately coarse deterministic failure class for Jenkins console logs.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingCategory {
    Compilation,
    Tests,
    Checkout,
    Timeout,
    Authorization,
    Aborted,
    FailureOrUnknown,
}

impl FindingCategory {
    const fn summary(self) -> &'static str {
        match self {
            Self::Compilation => "console log contains a compilation failure signature",
            Self::Tests => "console log contains a test failure signature",
            Self::Checkout => "console log contains a checkout failure signature",
            Self::Timeout => "console log contains a timeout signature",
            Self::Authorization => "console log contains an authorization failure signature",
            Self::Aborted => "console log contains an aborted-build signature",
            Self::FailureOrUnknown => "console log contains an unclassified failure signature",
        }
    }
}

/// One lexical inference anchored to an exact source byte range.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DiagnosisFinding {
    category: FindingCategory,
    evidence: EvidenceReference,
    inference: bool,
    summary: &'static str,
}

impl DiagnosisFinding {
    #[must_use]
    pub const fn category(&self) -> FindingCategory {
        self.category
    }

    #[must_use]
    pub const fn evidence(&self) -> &EvidenceReference {
        &self.evidence
    }

    #[must_use]
    pub const fn is_inference(&self) -> bool {
        self.inference
    }

    #[must_use]
    pub const fn summary(&self) -> &'static str {
        self.summary
    }
}

/// Completeness-aware outcome. `Partial` is never promoted to a solved or verified claim.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosisStatus {
    Diagnosed,
    Unresolved,
    Partial,
}

/// Pure diagnosis output and its canonical deterministic manifest artifact.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConsoleDiagnosis {
    status: DiagnosisStatus,
    summary: BoundedSummary,
    artifact_name: String,
    compacted: CompactedLog,
    findings: Vec<DiagnosisFinding>,
    manifest: ExactArtifact,
}

impl ConsoleDiagnosis {
    #[must_use]
    pub const fn status(&self) -> DiagnosisStatus {
        self.status
    }

    #[must_use]
    pub const fn summary(&self) -> &BoundedSummary {
        &self.summary
    }

    #[must_use]
    pub fn artifact_name(&self) -> &str {
        &self.artifact_name
    }

    #[must_use]
    pub const fn compacted(&self) -> &CompactedLog {
        &self.compacted
    }

    #[must_use]
    pub fn findings(&self) -> &[DiagnosisFinding] {
        &self.findings
    }

    #[must_use]
    pub const fn manifest(&self) -> &ExactArtifact {
        &self.manifest
    }
}

/// Persists, compacts, and lexically classifies one exact console log without model calls.
///
/// # Errors
///
/// Returns an error for an inconsistent collection/evidence pair, exceeded bounds, durable
/// evidence failure, compaction failure, or a manifest that cannot fit its fixed byte budget.
/// Every input is validated before the first durable write.
pub async fn diagnose_console_log(
    collection: &ConnectorOutput<CollectConsoleLogResponse>,
    exact_log: &[u8],
    persistence: &DiagnosisPersistence,
) -> Result<ConsoleDiagnosis, DiagnosisError> {
    let response = collection.value();
    let input_complete = collection.truth().is_complete();
    validate_collection(collection, exact_log)?;
    let source = persistence.persist(exact_log).await?;

    let compacted = compact_log(
        &source,
        exact_log,
        &LogMetadata::default(),
        &CompactionPolicy::default(),
    )
    .map_err(DiagnosisError::Compaction)?;
    let mut findings = classify_log(&source, exact_log);
    findings.sort_by_key(|finding| (finding.category, finding.evidence.offset));
    let findings_truncated = findings.len() > MAX_DIAGNOSIS_FINDINGS;
    findings.truncate(MAX_DIAGNOSIS_FINDINGS);

    let status = if !input_complete || findings_truncated {
        DiagnosisStatus::Partial
    } else if findings.is_empty() {
        DiagnosisStatus::Unresolved
    } else {
        DiagnosisStatus::Diagnosed
    };
    let summary = diagnosis_summary(status, findings.len())?;
    let manifest_bytes = manifest_bytes(
        response,
        input_complete,
        findings_truncated,
        status,
        &summary,
        &compacted,
        &findings,
    )?;
    let manifest_name = format!(
        "{}-diagnosis.json",
        response
            .artifact_name()
            .strip_suffix(".log")
            .unwrap_or(response.artifact_name())
    );
    let manifest = ExactArtifact::new(manifest_name, manifest_bytes)
        .map_err(|_| DiagnosisError::ManifestTooLarge)?;

    Ok(ConsoleDiagnosis {
        status,
        summary,
        artifact_name: response.artifact_name().to_owned(),
        compacted,
        findings,
        manifest,
    })
}

fn validate_collection(
    collection: &ConnectorOutput<CollectConsoleLogResponse>,
    exact_log: &[u8],
) -> Result<(), DiagnosisError> {
    let response = collection.value();
    if response.byte_len() > MAX_CONSOLE_LOG_BYTES || exact_log.len() > MAX_CONSOLE_LOG_BYTES {
        return Err(DiagnosisError::LogTooLarge);
    }
    if response.byte_len() != exact_log.len() {
        return Err(DiagnosisError::ByteLengthMismatch);
    }
    if collection.artifacts().len() != 1 {
        return Err(DiagnosisError::ArtifactPayloadCountMismatch);
    }
    let artifact = &collection.artifacts()[0];
    if artifact.name() != response.artifact_name() {
        return Err(DiagnosisError::ArtifactNameMismatch);
    }
    if artifact.bytes() != exact_log {
        return Err(DiagnosisError::ArtifactPayloadMismatch);
    }
    Ok(())
}

fn classify_log(source: &SourceEvidence, bytes: &[u8]) -> Vec<DiagnosisFinding> {
    let mut seen = std::collections::BTreeSet::new();
    let mut findings = Vec::new();
    let mut offset = 0_usize;
    while offset < bytes.len() {
        let relative_end = bytes[offset..].iter().position(|byte| *byte == b'\n');
        let end = relative_end.map_or(bytes.len(), |index| offset + index + 1);
        let line = &bytes[offset..end];
        if let Some(category) = classify_line(line)
            && seen.insert(category)
        {
            findings.push(DiagnosisFinding {
                category,
                evidence: EvidenceReference {
                    handle: source.handle.clone(),
                    offset: u64::try_from(offset).expect("usize fits in u64"),
                    length: u64::try_from(end - offset).expect("usize fits in u64"),
                },
                inference: true,
                summary: category.summary(),
            });
        }
        offset = end;
    }
    findings
}

fn classify_line(line: &[u8]) -> Option<FindingCategory> {
    if contains_any(line, AUTHORIZATION_PATTERNS) {
        Some(FindingCategory::Authorization)
    } else if contains_any(line, TIMEOUT_PATTERNS) {
        Some(FindingCategory::Timeout)
    } else if contains_any(line, ABORTED_PATTERNS) {
        Some(FindingCategory::Aborted)
    } else if contains_any(line, CHECKOUT_PATTERNS) {
        Some(FindingCategory::Checkout)
    } else if contains_any(line, TEST_PATTERNS) {
        Some(FindingCategory::Tests)
    } else if contains_any(line, COMPILATION_PATTERNS) {
        Some(FindingCategory::Compilation)
    } else if contains_any(line, FAILURE_OR_UNKNOWN_PATTERNS) {
        Some(FindingCategory::FailureOrUnknown)
    } else {
        None
    }
}

fn contains_any(line: &[u8], patterns: &[&[u8]]) -> bool {
    patterns.iter().any(|pattern| {
        line.windows(pattern.len())
            .any(|window| window.eq_ignore_ascii_case(pattern))
    })
}

fn diagnosis_summary(
    status: DiagnosisStatus,
    finding_count: usize,
) -> Result<BoundedSummary, DiagnosisError> {
    let text = match status {
        DiagnosisStatus::Diagnosed => format!(
            "found {finding_count} deterministic failure signature(s) in a complete Jenkins console log"
        ),
        DiagnosisStatus::Unresolved => {
            "no supported lexical failure signature was found in a complete Jenkins console log"
                .to_owned()
        }
        DiagnosisStatus::Partial => format!(
            "partial Jenkins diagnosis retained {finding_count} deterministic failure signature(s)"
        ),
    };
    BoundedSummary::new(text).map_err(|_| DiagnosisError::SummaryTooLarge)
}

#[derive(Serialize)]
struct Manifest<'a> {
    schema_version: &'static str,
    job: &'a str,
    build_number: u64,
    input_complete: bool,
    findings_truncated: bool,
    status: DiagnosisStatus,
    summary: &'a str,
    log: ManifestLog<'a>,
    findings: &'a [DiagnosisFinding],
}

#[derive(Serialize)]
struct ManifestLog<'a> {
    artifact_name: &'a str,
    source: &'a SourceEvidence,
    algorithm_version: &'a str,
    policy_version: &'a str,
    policy_digest: &'a ContentDigest,
    source_byte_count: u64,
    retained_byte_count: u64,
    source_record_count: u64,
    retained_record_count: u64,
}

fn manifest_bytes(
    response: &CollectConsoleLogResponse,
    input_complete: bool,
    findings_truncated: bool,
    status: DiagnosisStatus,
    summary: &BoundedSummary,
    compacted: &CompactedLog,
    findings: &[DiagnosisFinding],
) -> Result<Vec<u8>, DiagnosisError> {
    let manifest = Manifest {
        schema_version: DIAGNOSIS_SCHEMA_VERSION,
        job: response.job(),
        build_number: response.build().get(),
        input_complete,
        findings_truncated,
        status,
        summary: summary.as_str(),
        log: ManifestLog {
            artifact_name: response.artifact_name(),
            source: &compacted.source,
            algorithm_version: &compacted.algorithm_version,
            policy_version: &compacted.policy_version,
            policy_digest: &compacted.policy_digest,
            source_byte_count: compacted.source_byte_count,
            retained_byte_count: compacted.retained_byte_count,
            source_record_count: compacted.source_record_count,
            retained_record_count: compacted.retained_record_count,
        },
        findings,
    };
    let bytes = serde_json::to_vec(&manifest).map_err(|_| DiagnosisError::ManifestEncoding)?;
    if bytes.len() > MAX_DIAGNOSIS_MANIFEST_BYTES {
        return Err(DiagnosisError::ManifestTooLarge);
    }
    Ok(bytes)
}

#[derive(Debug)]
pub enum DiagnosisError {
    InvalidPersistenceProject,
    InvalidPersistenceTimestamp,
    LogTooLarge,
    ByteLengthMismatch,
    ArtifactPayloadCountMismatch,
    ArtifactNameMismatch,
    ArtifactPayloadMismatch,
    EvidencePersistence,
    PersistedIdentityMismatch,
    Compaction(CompactError),
    SummaryTooLarge,
    ManifestEncoding,
    ManifestTooLarge,
}

impl fmt::Display for DiagnosisError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPersistenceProject => {
                formatter.write_str("diagnosis evidence project identifier is invalid")
            }
            Self::InvalidPersistenceTimestamp => {
                formatter.write_str("diagnosis evidence timestamp is invalid")
            }
            Self::LogTooLarge => formatter.write_str("console log exceeds its byte bound"),
            Self::ByteLengthMismatch => {
                formatter.write_str("exact evidence length differs from the collected log")
            }
            Self::ArtifactPayloadCountMismatch => {
                formatter.write_str("collection must carry exactly one console log artifact")
            }
            Self::ArtifactNameMismatch => {
                formatter.write_str("collected artifact name is not canonical")
            }
            Self::ArtifactPayloadMismatch => {
                formatter.write_str("exact evidence differs from the collected artifact payload")
            }
            Self::EvidencePersistence => {
                formatter.write_str("exact diagnosis evidence could not be persisted")
            }
            Self::PersistedIdentityMismatch => {
                formatter.write_str("persisted diagnosis evidence identity is inconsistent")
            }
            Self::Compaction(source) => {
                write!(formatter, "console log compaction failed: {source}")
            }
            Self::SummaryTooLarge => formatter.write_str("diagnosis summary exceeds its bound"),
            Self::ManifestEncoding => formatter.write_str("diagnosis manifest encoding failed"),
            Self::ManifestTooLarge => formatter.write_str("diagnosis manifest exceeds its bound"),
        }
    }
}

impl Error for DiagnosisError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Compaction(source) => Some(source),
            _ => None,
        }
    }
}
