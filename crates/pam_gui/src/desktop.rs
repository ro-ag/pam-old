use std::{
    collections::{HashMap, HashSet, VecDeque},
    fmt,
    future::Future,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::Arc,
    time::Duration,
};

#[cfg(unix)]
use std::os::unix::process::CommandExt;

use pam_core::{CallerCredential, CallerId, EvidenceHandle as ProtocolEvidenceHandle, ProjectId};
use pam_daemon::registered_projects;
use pam_flow::{FlowDefinition, FlowStep, MAX_FLOW_DOCUMENT_BYTES, StepAction, StepCondition};
use pam_platform::{CallerKind, caller_id, discover_project, user_data_dir};
use pam_protocol::{
    ActivityDaySummary, ActivityEventSummary, ActivityResult, ApprovalDecision, CallerListResult,
    CallerSummary, ConnectorConfigureResult, ConnectorCredentialAction, ConnectorListResult,
    ConnectorSummary, ConnectorTestDisposition, ConnectorTestResult, DaemonLogsResult,
    DaemonStatsResult, FailureCode, FlowProjectRoot, LogSeverity, MAX_MODEL_OUTPUT_TOKENS,
    ModelFinishReason, ModelGenerationResult, ModelMessage, ModelRole, ModelStatusResult,
    ModelSummary, ProjectRequestState, ProjectRequestSummary, ProjectUsageSummary,
};
use pam_store::Store;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::{
    access_config::{AccessConfigState, AccessConfigView, load_access_config},
    control_center::{
        HealthState, MAX_PROJECTS, bounded_name, load_credential, load_project_health_access,
        load_project_surfaces, probe_health_authenticated, project_name,
        request_daemon_stop_authenticated,
    },
    current::{
        ApprovalDecisionFailure, ApprovalDecisionView, CurrentState, CurrentUnavailableCode,
        CurrentView, EvidencePreview, EvidenceState, OutcomeView, PendingApproval, RunView,
        TimelineFact, TimelineKind, decide_current_approval, load_evidence,
    },
    daemon_access::{DaemonAccessDto, read_daemon_access, update_daemon_access},
    flow_editor::{
        ActionAuthority, DaemonAuthority, DryRunCondition, FlowDryRunPlan, FlowEditorDocument,
        FlowEditorError, FlowEditorModel, FlowIdentity, FlowVersionDiff, FlowVersionDiffLineKind,
    },
    model_discovery::discover_license,
    model_download::{
        MIN_SUPPORTED_HOST_MEMORY_BYTES, ModelDownloadManager, ModelDownloadStatusKind,
        host_memory_total_bytes,
    },
    model_import::{
        MIN_RECOMMENDED_MODEL_BYTES, ModelImportManager, ModelImportParams, ModelImportStage,
        ModelImportStatusKind, run_model_inspect,
    },
    model_presets,
    observatory::{
        ObservatoryState, load_caller_registry, load_connector_registry, load_daemon_activity,
        load_daemon_logs, load_daemon_stats, load_model_status, run_connector_configure,
        run_connector_test, run_model_infer,
    },
    settings,
    skill_audit::{SkillAuditDto, load_persisted_skill_audit, run_skill_audit_report},
    skill_inventory::{SkillInventoryDto, SkillInventoryEnvironment, load_skill_inventory},
    skill_library::{
        SkillLibraryAction, SkillLibraryDataDto, SkillLibraryDto, SkillLibraryEnvironment,
        SkillLibraryRequest, execute_skill_library, project_key, project_scope_required,
    },
};

const MAX_OPERATIONS: usize = 256;
const MAX_DETAILS_BYTES: usize = 4 * 1024;
const MAX_PROJECT_PATH_BYTES: usize = 4 * 1024;
const MAX_TIMELINE_FACTS: usize = 256;
const MAX_EVIDENCE_HANDLES: usize = 256;
/// The post-spawn startup poll's fixed allowance: enough for the daemon to
/// open its store, bind, and answer health when no model is involved.
const STARTUP_BASE_TIMEOUT: Duration = Duration::from_mins(2);
/// On top of the base allowance, one minute per this many bytes of the
/// requested model's artifact: the daemon revalidates the whole GGUF digest
/// and then maps it into the Metal runtime, which is minutes of honest work
/// for a multi-GB file. Deliberately conservative — the budget only decides
/// when the GUI stops waiting, never whether the daemon lives.
const MODEL_LOAD_BYTES_PER_MINUTE: u64 = 8 * 1024 * 1024 * 1024;
/// Whatever the artifact size, the GUI stops holding its command gate here.
const STARTUP_TIMEOUT_CAP: Duration = Duration::from_mins(10);
const STARTUP_POLL_INTERVAL: Duration = Duration::from_millis(250);
/// Graceful daemon shutdown after a stop request is quick; this bounds the
/// wait before the GUI kills and collects the daemon child itself.
const DAEMON_STOP_GRACE: Duration = Duration::from_secs(5);
const DAEMON_STOP_POLL: Duration = Duration::from_millis(50);
const GUI_REGISTRATION_TIMEOUT: Duration = Duration::from_secs(15);
const GUI_REGISTRATION_ARGS: [&str; 4] = ["caller", "register", "--kind", "gui"];
const GUI_REGISTRATION_RECOVERY: &str = "Use Register GUI caller in PAM.";
const MODEL_INFER_DEFAULT_OUTPUT_TOKENS: u32 = 512;
/// The reserved client-side authority literal for daemon-scoped commands.
const DAEMON_AUTHORITY: &str = "daemon";

macro_rules! uuid_handle {
    ($name:ident $(, $literal:literal)*) => {
        #[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Self::parse(value).map_err(|_| {
                    serde::de::Error::custom("desktop handles must be canonical UUID strings")
                })
            }
        }

        impl $name {
            #[must_use]
            pub fn new() -> Self {
                Self(Uuid::new_v4().to_string())
            }

            /// Parses one canonical UUID handle.
            ///
            /// # Errors
            ///
            /// Rejects malformed or non-canonical UUID text.
            pub fn parse(value: impl Into<String>) -> DesktopResult<Self> {
                let value = value.into();
                if !Self::ALLOWED_LITERALS.contains(&value.as_str()) {
                    validate_uuid(&value)?;
                }
                Ok(Self(value))
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }

            const ALLOWED_LITERALS: &'static [&'static str] = &[$($literal),*];

            fn validate(&self) -> DesktopResult<()> {
                if Self::ALLOWED_LITERALS.contains(&self.0.as_str()) {
                    return Ok(());
                }
                validate_uuid(&self.0)
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }
    };
}

uuid_handle!(ProjectHandle, "daemon");
uuid_handle!(GenerationId, "daemon");
uuid_handle!(OperationId);
uuid_handle!(ApprovalHandle);
uuid_handle!(EvidenceHandleDto);
uuid_handle!(FlowDefinitionHandle);
uuid_handle!(FlowDocumentHandle);

fn validate_uuid(value: &str) -> DesktopResult<()> {
    let parsed = Uuid::parse_str(value).map_err(|_| {
        DesktopErrorDto::invalid_input("Desktop handles must be canonical UUID strings.")
    })?;
    if parsed.hyphenated().to_string() != value {
        return Err(DesktopErrorDto::invalid_input(
            "Desktop handles must be canonical UUID strings.",
        ));
    }
    Ok(())
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommandFence {
    pub project_handle: ProjectHandle,
    pub generation: GenerationId,
    pub operation_id: OperationId,
}

impl CommandFence {
    #[must_use]
    pub const fn new(
        project_handle: ProjectHandle,
        generation: GenerationId,
        operation_id: OperationId,
    ) -> Self {
        Self {
            project_handle,
            generation,
            operation_id,
        }
    }

    fn validate(&self) -> DesktopResult<()> {
        self.project_handle.validate()?;
        self.generation.validate()?;
        self.operation_id.validate()
    }
}

pub type SnapshotFence = CommandFence;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DesktopErrorKind {
    InvalidInput,
    NotFound,
    Stale,
    Conflict,
    Unavailable,
    Internal,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DesktopErrorDto {
    pub kind: DesktopErrorKind,
    pub message: String,
    pub recovery: Option<String>,
}

impl DesktopErrorDto {
    pub(crate) fn new(
        kind: DesktopErrorKind,
        message: impl Into<String>,
        recovery: Option<String>,
    ) -> Self {
        Self {
            kind,
            message: bounded_detail(message.into()),
            recovery: recovery.map(bounded_detail),
        }
    }

    pub(crate) fn invalid_input(message: impl Into<String>) -> Self {
        Self::new(DesktopErrorKind::InvalidInput, message, None)
    }

    fn stale(message: impl Into<String>) -> Self {
        Self::new(
            DesktopErrorKind::Stale,
            message,
            Some("Refresh the active project and retry with its new fence.".to_owned()),
        )
    }

    pub(crate) fn unavailable(message: impl Into<String>, recovery: Option<String>) -> Self {
        Self::new(DesktopErrorKind::Unavailable, message, recovery)
    }
}

impl fmt::Display for DesktopErrorDto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for DesktopErrorDto {}

pub type DesktopResult<T> = Result<T, DesktopErrorDto>;

/// The bootstrap result: the discovered catalog plus an activated project
/// snapshot, or no snapshot at all in global-only mode (empty catalog).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BootstrapDto {
    pub catalog: CatalogDto,
    pub snapshot: Option<SnapshotDto>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CatalogDto {
    pub projects: Vec<ProjectSummaryDto>,
    pub warning: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectSummaryDto {
    pub handle: ProjectHandle,
    pub name: String,
    pub location: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SnapshotDto {
    pub fence: CommandFence,
    pub data: SnapshotDataDto,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SnapshotDataDto {
    pub project: ProjectSummaryDto,
    pub health: HealthDto,
    pub current: CurrentDto,
    pub access: AccessConfigDto,
    pub catalog_warning: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "status",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum HealthDto {
    Healthy {
        daemon_version: String,
        queue_depth: u64,
    },
    Offline,
    Degraded {
        detail: String,
        recovery: Option<String>,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureKindDto {
    Blocked,
    Unavailable,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FailureDto {
    pub kind: FailureKindDto,
    pub code: Option<String>,
    pub detail: String,
    pub recovery: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "status",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum AccessConfigDto {
    Available {
        truth: String,
        platform_roots_enabled: bool,
        system_proxy_discovery_enabled: bool,
        proxy_environment: String,
        no_proxy: String,
        pac: String,
    },
    Blocked {
        failure: FailureDto,
        approval_id: Option<String>,
        expires_at_ms: Option<u64>,
    },
    Unavailable {
        failure: FailureDto,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "status",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum CurrentDto {
    Available {
        queued: Vec<RequestSummaryDto>,
        truncated: bool,
        run: Option<RunDto>,
    },
    ApprovalRequired {
        approval: ApprovalHandle,
        expires_at_ms: u64,
    },
    Blocked {
        failure: FailureDto,
    },
    Unavailable {
        failure: FailureDto,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestSummaryDto {
    pub request_id: String,
    pub operation_kind: String,
    pub state: String,
    pub queue_sequence: u64,
    pub accepted_at_ms: u64,
    pub completed_at_ms: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunDto {
    pub request: RequestSummaryDto,
    pub timeline: Vec<TimelineFactDto>,
    pub outcome: Option<OutcomeDto>,
    pub detail_error: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TimelineFactDto {
    pub kind: TimelineKindDto,
    pub label: String,
    pub summary: String,
    pub verified: bool,
    pub evidence: Vec<EvidenceHandleDto>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TimelineKindDto {
    Request,
    Evidence,
    Change,
    Verification,
    Failure,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutcomeDto {
    pub heading: String,
    pub solved: bool,
    pub sections: Vec<OutcomeSectionDto>,
    pub evidence: Vec<EvidenceHandleDto>,
    pub evidence_truncated: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutcomeSectionDto {
    pub label: String,
    pub summary: String,
    pub satisfied: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecisionDto {
    Approve,
    Deny,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecisionDispositionDto {
    Approved,
    Denied,
    Expired,
}

impl From<pam_protocol::ApprovalDecisionDisposition> for ApprovalDecisionDispositionDto {
    fn from(value: pam_protocol::ApprovalDecisionDisposition) -> Self {
        match value {
            pam_protocol::ApprovalDecisionDisposition::Approved => Self::Approved,
            pam_protocol::ApprovalDecisionDisposition::Denied => Self::Denied,
            pam_protocol::ApprovalDecisionDisposition::Expired => Self::Expired,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApprovalDecisionResponseDto {
    pub disposition: ApprovalDecisionDispositionDto,
    pub snapshot: SnapshotDto,
}

impl From<ApprovalDecisionDto> for ApprovalDecision {
    fn from(value: ApprovalDecisionDto) -> Self {
        match value {
            ApprovalDecisionDto::Approve => Self::Approve,
            ApprovalDecisionDto::Deny => Self::Deny,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EvidenceDto {
    pub fence: CommandFence,
    pub data: EvidenceDataDto,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EvidenceDataDto {
    pub handle: EvidenceHandleDto,
    pub digest: String,
    pub size_bytes: u64,
    pub media_type: String,
    pub body: Option<String>,
    pub truncated: bool,
    pub truth: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FlowWorkspaceDto {
    pub fence: CommandFence,
    pub data: FlowWorkspaceDataDto,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FlowWorkspaceDataDto {
    pub definitions: Vec<FlowDefinitionDto>,
    /// Definition IDs copied into the global library from a legacy
    /// project-local `.pam/flows` catalog during this load. Always empty
    /// once a project's legacy flows have all been migrated once.
    pub migrated: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FlowDefinitionDto {
    pub handle: FlowDefinitionHandle,
    pub identity: FlowIdentityDto,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FlowIdentityDto {
    pub file_name: String,
    pub id: String,
    pub revision: u64,
    pub digest: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FlowDocumentDto {
    pub fence: CommandFence,
    pub data: FlowDocumentDataDto,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FlowDocumentDataDto {
    pub handle: FlowDocumentHandle,
    pub identity: Option<FlowIdentityDto>,
    pub source: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FlowReviewDto {
    pub fence: CommandFence,
    pub data: FlowReviewDataDto,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FlowReviewDataDto {
    pub document: FlowDocumentHandle,
    pub identity: FlowIdentityDto,
    pub normalized_toml: String,
    pub dry_run: FlowDryRunDto,
    pub diff: FlowVersionDiffDto,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FlowDryRunDto {
    pub daemon_definition_eligible: bool,
    pub steps: Vec<FlowDryRunStepDto>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FlowDryRunStepDto {
    pub index: usize,
    pub id: String,
    pub semantic_role: String,
    pub condition: String,
    pub approval: String,
    pub effect: String,
    pub max_attempts: u8,
    pub initial_backoff_ms: u64,
    pub max_backoff_ms: u64,
    pub action: String,
    pub daemon_authority: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FlowVersionDiffDto {
    pub changed: bool,
    pub truncated: bool,
    pub lines: Vec<FlowVersionDiffLineDto>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FlowVersionDiffLineDto {
    pub kind: String,
    pub text: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FlowSaveDto {
    pub fence: CommandFence,
    pub data: FlowSaveDataDto,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FlowSaveDataDto {
    pub document: FlowDocumentHandle,
    pub identity: FlowIdentityDto,
    pub created: bool,
    pub durability_confirmed: bool,
    pub cleanup_complete: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "status",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ActivityDto {
    Ok {
        events: Vec<ActivityEventDto>,
        truncated: bool,
    },
    Blocked {
        failure: FailureDto,
    },
    Unavailable {
        failure: FailureDto,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActivityEventDto {
    pub sequence: u64,
    pub project_id: String,
    pub caller_id: String,
    pub action: String,
    pub decision: String,
    pub outcome: String,
    pub occurred_at_ms: u64,
    pub project_root: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "status",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum DaemonLogsDto {
    Ok { entries: Vec<DaemonLogEntryDto> },
    Blocked { failure: FailureDto },
    Unavailable { failure: FailureDto },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DaemonLogEntryDto {
    pub timestamp_ms: u64,
    pub severity: String,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "status",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum DaemonStatsDto {
    Ok {
        days: Vec<ActivityDayDto>,
        projects: Vec<ProjectUsageDto>,
    },
    Blocked {
        failure: FailureDto,
    },
    Unavailable {
        failure: FailureDto,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActivityDayDto {
    pub day_start_ms: u64,
    pub events: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectUsageDto {
    pub project_id: String,
    pub events: u64,
    pub last_event_ms: u64,
    pub root: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "status",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum CallersDto {
    Ok { callers: Vec<CallerDto> },
    Blocked { failure: FailureDto },
    Unavailable { failure: FailureDto },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CallerDto {
    pub caller_id: String,
    pub registered_at_ms: u64,
    pub revoked_at_ms: Option<u64>,
    /// Self-declared local caller surface (`cli`, `gui`, `coding-agent`, or
    /// `local-application`). `None` for callers registered before this field
    /// existed.
    pub kind: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "status",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ModelStatusDto {
    Ok {
        loaded: Option<ModelSummaryDto>,
        registered: Vec<ModelSummaryDto>,
        /// Why the daemon is serving without the model it was started with.
        /// Present for as long as that daemon runs; `None` once a model is
        /// loaded or none was requested.
        load_failure: Option<String>,
        /// The daemon this GUI spawned is running but has not answered yet:
        /// its model load is still in flight. `false` for every daemon that
        /// answered, and for one this GUI does not own a handle on.
        loading: bool,
    },
    Blocked {
        failure: FailureDto,
    },
    Unavailable {
        failure: FailureDto,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelSummaryDto {
    pub model_id: String,
    pub size_bytes: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "status",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ModelInferDto {
    Ok {
        model: String,
        text: String,
        finish_reason: String,
        usage: ModelUsageDto,
    },
    Blocked {
        failure: FailureDto,
    },
    Unavailable {
        failure: FailureDto,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelUsageDto {
    pub input_tokens: u32,
    pub sampled_output_tokens: u32,
    pub emitted_output_tokens: u32,
}

/// Starts a background import. Like [`ModelDownloadDto`], business refusals
/// (bad path, already running) are `Unavailable` data, not an `Err`; the
/// import itself is polled through [`ModelImportStatusDto`].
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "status",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ModelImportDto {
    Ok,
    Blocked { failure: FailureDto },
    Unavailable { failure: FailureDto },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelImportStatusKindDto {
    Idle,
    Running,
    Complete,
    Failed,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelImportStageDto {
    Hashing,
    Registering,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelImportStatusDto {
    pub status: ModelImportStatusKindDto,
    pub model: Option<String>,
    pub stage: Option<ModelImportStageDto>,
    pub hashed_bytes: u64,
    pub total_bytes: u64,
    pub failure: Option<FailureDto>,
    /// True when the completed artifact is one of PAM's calibrated GGUFs.
    /// Only meaningful on `complete`; false while idle, running, or failed.
    pub calibrated: bool,
}

/// Pre-import preview of a candidate GGUF for the Models view's manual
/// import flow: identity metadata and the recommended-size floor verdict,
/// read without hashing the file. Failures use the same bounded envelope as
/// [`ModelImportDto`] rather than a raw error.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "status",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ModelInspectDto {
    Ok {
        file_name: String,
        size_bytes: u64,
        architecture: Option<String>,
        model_name: Option<String>,
        license: Option<String>,
        below_floor: bool,
        floor_bytes: u64,
    },
    Blocked {
        failure: FailureDto,
    },
    Unavailable {
        failure: FailureDto,
    },
}

/// Hugging Face license discovery for a manual import: given the model name
/// a GGUF declares, the matching repository and its raw `license:` tag. An
/// enhancement over manual entry — failures are bounded data, never a hard
/// error, and the form keeps working without it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "status",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ModelLicenseDiscoveryDto {
    Ok { repo_id: String, license_id: String },
    Unavailable { failure: FailureDto },
}

/// One curated, pre-verified downloadable model preset.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelPresetDto {
    pub id: String,
    pub label: String,
    pub model: String,
    pub file_name: String,
    pub url: String,
    pub expected_size_bytes: u64,
    pub sha256: String,
    pub license_id: String,
    pub license_url: String,
    pub license_notice_text: String,
    /// True when this exact artifact is in PAM's measured, known-good set.
    /// False is not a refusal — the picker warns, it does not hide.
    pub calibrated: bool,
    /// Whether this Mac can run the preset, by the daemon's own admission
    /// arithmetic. True when the host memory probe is unavailable: PAM
    /// refuses nothing it could not measure.
    pub fits_host: bool,
    pub params_label: String,
    pub quant_label: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelPresetsDto {
    pub presets: Vec<ModelPresetDto>,
    /// The largest artifact this Mac can devote to a model: its runtime
    /// ceiling less the projection contingency. `None` when the host memory
    /// probe is unavailable.
    pub host_model_budget_bytes: Option<u64>,
}

/// Starts a guided download. Unlike the fence itself going stale, business
/// refusals (unknown preset, already running) are reported as `Unavailable`
/// data here, exactly like [`ModelImportDto`], rather than an `Err`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "status",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ModelDownloadDto {
    Ok,
    Blocked { failure: FailureDto },
    Unavailable { failure: FailureDto },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelDownloadStatusKindDto {
    Idle,
    Running,
    Complete,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelDownloadStatusDto {
    pub status: ModelDownloadStatusKindDto,
    pub preset_id: Option<String>,
    pub received_bytes: u64,
    pub total_bytes: u64,
    pub failure: Option<FailureDto>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StartupPhaseDto {
    Verifying,
    Loading,
}

/// The live model load of a daemon start in flight. `model_id` and `phase`
/// are `None` when no start is loading a registered model, which is what an
/// idle meter renders as. `loaded_bytes` is only meaningful in the `Loading`
/// phase; the verification phase reports `elapsed_seconds` instead.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DaemonStartupProgressDto {
    pub model_id: Option<String>,
    pub phase: Option<StartupPhaseDto>,
    pub loaded_bytes: u64,
    pub total_bytes: u64,
    pub elapsed_seconds: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostMemoryDto {
    pub total_bytes: u64,
    /// PAM's supported system minimum: local AI needs a 32 GiB machine.
    pub supported_minimum_bytes: u64,
}

/// Settings v1: visibility into where PAM keeps things, and the one
/// persisted preference so far. Global, like [`Self::model_presets`] and
/// [`Self::host_memory`] — it works under the daemon authority with zero
/// active projects.
///
/// `flowsDir` is the daemon-global flow-definition library
/// (`<flow_library_root>/.pam/flows`), the same on-disk catalog the Flows
/// view and the CLI open.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AppSettingsDto {
    pub models_dir: String,
    pub models_dir_is_default: bool,
    pub data_dir: String,
    pub flows_dir: String,
    pub logs_dir: String,
    pub logs_size_bytes: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelRoleDto {
    System,
    User,
    Assistant,
}

impl From<ModelRoleDto> for ModelRole {
    fn from(value: ModelRoleDto) -> Self {
        match value {
            ModelRoleDto::System => Self::System,
            ModelRoleDto::User => Self::User,
            ModelRoleDto::Assistant => Self::Assistant,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelMessageDto {
    pub role: ModelRoleDto,
    pub content: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "status",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ConnectorsDto {
    Ok {
        connectors: Vec<ConnectorSummaryDto>,
    },
    Blocked {
        failure: FailureDto,
    },
    Unavailable {
        failure: FailureDto,
    },
}

/// One connector's configuration state; credential values never cross this
/// contract, only their presence does.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConnectorSummaryDto {
    pub connector_id: String,
    pub enabled: bool,
    pub base_url: Option<String>,
    pub credential_present: bool,
    pub last_test_status: Option<String>,
    pub last_test_at_ms: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "status",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ConnectorConfigureDto {
    Ok { connector: ConnectorSummaryDto },
    Blocked { failure: FailureDto },
    Unavailable { failure: FailureDto },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "status",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ConnectorTestDto {
    Ok {
        connector_id: String,
        result: String,
        detail: String,
    },
    Blocked {
        failure: FailureDto,
    },
    Unavailable {
        failure: FailureDto,
    },
}

/// One connector configuration change; the optional credential action passes
/// through in memory only and its secret stays redacted in debug output.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConnectorConfigureParams {
    pub connector: String,
    pub enabled: Option<bool>,
    pub base_url: Option<String>,
    pub credential: Option<ConnectorCredentialAction>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(
    tag = "status",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum FlowGraphDto {
    Ok { definition: serde_json::Value },
    Invalid { failure: FailureDto },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "status",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum FlowComposeDto {
    Ok { source: String },
    Invalid { failure: FailureDto },
}

#[derive(Clone)]
pub struct DesktopCore {
    inner: Arc<Mutex<DesktopState>>,
    command_gate: Arc<Mutex<()>>,
    downloads: Arc<ModelDownloadManager>,
    imports: Arc<ModelImportManager>,
    startup_progress: StartupProgressCell,
}

impl fmt::Debug for DesktopCore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DesktopCore { authority: [REDACTED] }")
    }
}

struct DesktopState {
    startup_root: PathBuf,
    daemon_executable: PathBuf,
    catalog: HashMap<ProjectHandle, CatalogProject>,
    active: Option<ActiveProject>,
    activation_operation: Option<OperationId>,
    used_operations: VecDeque<OperationId>,
    daemon_operations: VecDeque<OperationId>,
    approvals: HashMap<ApprovalHandle, PendingApproval>,
    evidence: HashMap<EvidenceHandleDto, ProtocolEvidenceHandle>,
    flow_workspace: Option<FlowWorkspaceState>,
    daemon_child: Option<Child>,
    catalog_warning: Option<String>,
}

#[derive(Clone)]
struct CatalogProject {
    handle: ProjectHandle,
    name: String,
    root: PathBuf,
}

#[derive(Clone)]
struct ActiveProject {
    catalog: CatalogProject,
    project_id: ProjectId,
    generation: GenerationId,
}

/// The daemon-global flow-definition workspace. Unlike other per-command
/// state, this carries no project or generation stamp: flow definitions are
/// authored once against the daemon-global library and are usable from any
/// project, so the open catalog and its documents outlive project switches.
struct FlowWorkspaceState {
    model: FlowEditorModel,
    definitions: HashMap<FlowDefinitionHandle, String>,
    documents: HashMap<FlowDocumentHandle, FlowEditorDocument>,
}

impl DesktopCore {
    #[must_use]
    pub fn new(startup_root: impl Into<PathBuf>) -> Self {
        let daemon_executable = default_daemon_executable();
        Self::with_daemon_executable(startup_root, daemon_executable)
    }

    #[must_use]
    pub fn with_daemon_executable(
        startup_root: impl Into<PathBuf>,
        daemon_executable: impl Into<PathBuf>,
    ) -> Self {
        Self {
            inner: Arc::new(Mutex::new(DesktopState {
                startup_root: startup_root.into(),
                daemon_executable: daemon_executable.into(),
                catalog: HashMap::new(),
                active: None,
                activation_operation: None,
                used_operations: VecDeque::new(),
                daemon_operations: VecDeque::new(),
                approvals: HashMap::new(),
                evidence: HashMap::new(),
                flow_workspace: None,
                daemon_child: None,
                catalog_warning: None,
            })),
            command_gate: Arc::new(Mutex::new(())),
            downloads: ModelDownloadManager::new(),
            imports: ModelImportManager::new(),
            startup_progress: StartupProgressCell::default(),
        }
    }

    /// Loads direct discovery and optional ptrack hints without activating a
    /// hint or exposing its path as a filesystem authority.
    pub async fn catalog(&self) -> CatalogDto {
        let startup_root = self.inner.lock().await.startup_root.clone();
        let direct = discover_project(&startup_root)
            .ok()
            .map(|identity| (project_name(identity.root()), identity.root().to_path_buf()));
        let registered = registered_projects(&startup_root).await;
        let warning = registered.as_ref().err().cloned().map(bounded_detail);

        let mut candidates = Vec::new();
        if let Some(candidate) = direct {
            candidates.push(candidate);
        }
        if let Ok(entries) = registered {
            for entry in entries {
                let Ok(root) = entry.path().canonicalize() else {
                    continue;
                };
                if root.is_dir() {
                    candidates.push((bounded_name(entry.name(), &root), root));
                }
            }
        }

        let mut seen = HashSet::new();
        candidates.retain(|(_, root)| seen.insert(root.clone()));
        candidates.truncate(MAX_PROJECTS);

        let mut state = self.inner.lock().await;
        let old_by_root = state
            .catalog
            .values()
            .map(|candidate| (candidate.root.clone(), candidate.handle.clone()))
            .collect::<HashMap<_, _>>();
        let mut catalog = HashMap::new();
        let mut projects = Vec::with_capacity(candidates.len());
        for (name, root) in candidates {
            let handle = old_by_root
                .get(&root)
                .cloned()
                .unwrap_or_else(ProjectHandle::new);
            let candidate = CatalogProject {
                handle: handle.clone(),
                name: bounded_name(&name, &root),
                root,
            };
            projects.push(project_dto(&candidate));
            catalog.insert(handle, candidate);
        }
        state.catalog = catalog;
        state.catalog_warning.clone_from(&warning);
        CatalogDto { projects, warning }
    }

    /// Discovers and activates the first valid startup project, preferring the
    /// direct launch context before ordered ptrack catalog hints. An empty
    /// catalog is not an error: bootstrap then reports global-only mode with
    /// no snapshot.
    ///
    /// # Errors
    ///
    /// Returns a bounded error when a discovered candidate cannot be
    /// activated, authority setup fails, or a concurrent activation
    /// supersedes this operation.
    pub async fn bootstrap(&self, operation: OperationId) -> DesktopResult<BootstrapDto> {
        operation.validate()?;
        let catalog = self.catalog().await;
        self.bootstrap_with_catalog(operation, catalog).await
    }

    async fn bootstrap_with_catalog(
        &self,
        operation: OperationId,
        catalog: CatalogDto,
    ) -> DesktopResult<BootstrapDto> {
        let _command = self.command_gate.lock().await;
        let candidates = {
            let mut state = self.inner.lock().await;
            if state.activation_operation.is_some() {
                return Err(DesktopErrorDto::new(
                    DesktopErrorKind::Conflict,
                    "A project activation is already running.",
                    None,
                ));
            }
            state.activation_operation = Some(operation.clone());
            catalog
                .projects
                .iter()
                .filter_map(|project| state.catalog.get(&project.handle).cloned())
                .collect::<Vec<_>>()
        };

        if candidates.is_empty() {
            let mut state = self.inner.lock().await;
            if state.activation_operation.as_ref() == Some(&operation) {
                state.activation_operation = None;
            }
            return Ok(BootstrapDto {
                catalog,
                snapshot: None,
            });
        }

        let mut last_error = None;
        let mut selected = None;
        for candidate in candidates {
            match discover_project(&candidate.root) {
                Ok(identity) => {
                    selected = Some((candidate, identity));
                    break;
                }
                Err(error) => last_error = Some(error.to_string()),
            }
        }
        let Some((candidate, identity)) = selected else {
            let mut state = self.inner.lock().await;
            if state.activation_operation.as_ref() == Some(&operation) {
                state.activation_operation = None;
            }
            return Err(DesktopErrorDto::unavailable(
                last_error.unwrap_or_else(|| {
                    "PAM could not find a project in the launch context or ptrack catalog."
                        .to_owned()
                }),
                Some("Open PAM from a Git repository or initialized PAM project.".to_owned()),
            ));
        };
        let surfaces =
            load_surfaces(identity.id().clone(), request_project_root(identity.root())).await;
        let mut state = self.inner.lock().await;
        if state.activation_operation.as_ref() != Some(&operation) {
            return Err(DesktopErrorDto::stale(
                "A newer project activation replaced bootstrap.",
            ));
        }
        let generation = GenerationId::new();
        let active = ActiveProject {
            catalog: CatalogProject {
                root: identity.root().to_path_buf(),
                ..candidate
            },
            project_id: identity.id().clone(),
            generation,
        };
        state.active = Some(active.clone());
        state.activation_operation = None;
        state.used_operations.clear();
        state.approvals.clear();
        state.evidence.clear();
        state.flow_workspace = None;
        Ok(BootstrapDto {
            catalog,
            snapshot: Some(snapshot_from_surfaces(
                &mut state, &active, operation, surfaces,
            )),
        })
    }

    /// Activates one opaque catalog entry and loads one authenticated snapshot.
    ///
    /// # Errors
    ///
    /// Returns a bounded error for an invalid/stale handle, project discovery
    /// failure, or a concurrent activation.
    pub async fn activate(
        &self,
        project: ProjectHandle,
        operation: OperationId,
    ) -> DesktopResult<SnapshotDto> {
        let _command = self.command_gate.lock().await;
        project.validate()?;
        operation.validate()?;
        let candidate = {
            let mut state = self.inner.lock().await;
            if state.activation_operation.is_some() {
                return Err(DesktopErrorDto::new(
                    DesktopErrorKind::Conflict,
                    "A project activation is already running.",
                    None,
                ));
            }
            let candidate = state.catalog.get(&project).cloned().ok_or_else(|| {
                DesktopErrorDto::new(
                    DesktopErrorKind::NotFound,
                    "The selected project handle is not in the current catalog.",
                    Some("Reload the project catalog and select it again.".to_owned()),
                )
            })?;
            state.activation_operation = Some(operation.clone());
            candidate
        };

        let identity = match discover_project(&candidate.root) {
            Ok(identity) => identity,
            Err(error) => {
                let mut state = self.inner.lock().await;
                if state.activation_operation.as_ref() == Some(&operation) {
                    state.activation_operation = None;
                }
                return Err(DesktopErrorDto::unavailable(
                    error.to_string(),
                    Some("Choose a Git repository or initialized PAM project.".to_owned()),
                ));
            }
        };
        let surfaces =
            load_surfaces(identity.id().clone(), request_project_root(identity.root())).await;

        let mut state = self.inner.lock().await;
        if state.activation_operation.as_ref() != Some(&operation) {
            return Err(DesktopErrorDto::stale(
                "A newer project activation replaced this operation.",
            ));
        }
        let generation = GenerationId::new();
        let active = ActiveProject {
            catalog: CatalogProject {
                root: identity.root().to_path_buf(),
                ..candidate
            },
            project_id: identity.id().clone(),
            generation: generation.clone(),
        };
        state.active = Some(active.clone());
        state.activation_operation = None;
        state.used_operations.clear();
        state.approvals.clear();
        state.evidence.clear();
        state.flow_workspace = None;
        Ok(snapshot_from_surfaces(
            &mut state, &active, operation, surfaces,
        ))
    }

    /// Refreshes all authenticated project surfaces under one credential read.
    ///
    /// # Errors
    ///
    /// Returns a bounded error when the fence is invalid, stale, or reused.
    pub async fn refresh(&self, fence: CommandFence) -> DesktopResult<SnapshotDto> {
        let _command = self.command_gate.lock().await;
        let active = self.begin(&fence).await?;
        let surfaces = load_active_surfaces(&active).await;
        self.finish_snapshot(fence, active, surfaces).await
    }

    /// Starts the configured PAM daemon. Under a project fence it returns a
    /// newly fenced snapshot; under the daemon authority it returns no
    /// snapshot and spawns from the user home directory.
    ///
    /// # Errors
    ///
    /// Returns a bounded error for stale authority or process startup failure.
    pub async fn start_daemon(
        &self,
        fence: CommandFence,
        model: Option<String>,
    ) -> DesktopResult<Option<SnapshotDto>> {
        // Validate the optional model identity before any authority or process
        // work; the protocol's vendor/name contract is the single source of truth.
        if let Some(model) = &model {
            ModelSummary::new(model.clone(), 1).map_err(|_| {
                DesktopErrorDto::invalid_input(
                    "The model identity must be a registered vendor/name pair.",
                )
            })?;
        }
        let _command = self.command_gate.lock().await;
        let scope = self.begin_scoped(&fence).await?;
        let (executable, spawn_root) = {
            let state = self.inner.lock().await;
            let spawn_root = match &scope {
                CommandScope::Project(active) => active.catalog.root.clone(),
                CommandScope::Daemon => daemon_start_cwd(&state.startup_root),
            };
            (state.daemon_executable.clone(), spawn_root)
        };
        // The daemon starts only with a control-center launch grant, and runs
        // detached in its own process group: closing the UI never stops it.
        let endpoint = pam_platform::LocalEndpoint::default_for_user();
        let grant = pam_platform::issue_launch_grant(endpoint.runtime_dir()).map_err(|error| {
            DesktopErrorDto::unavailable(
                "PAM could not authorize a daemon launch.",
                Some(error.to_string()),
            )
        })?;
        let mut child = daemon_start_command(&executable, model.as_deref(), &spawn_root, grant)
            .spawn()
            .map_err(|error| {
                DesktopErrorDto::unavailable(
                    "PAM could not start the local daemon.",
                    Some(error.to_string()),
                )
            })?;
        let project_id = scope.project_id();
        // The startup probes authenticate as the GUI caller, exactly like
        // `daemon_health`/`model_status`. Without that credential (GUI caller
        // not registered yet) health stays Offline and only process liveness
        // can end the wait early; the deadline still bounds it.
        let startup_credential = match caller_id(CallerKind::Gui) {
            Ok(caller) => load_credential(caller.clone())
                .await
                .ok()
                .map(|credential| (caller, credential)),
            Err(_) => None,
        };
        let health = || {
            let startup_credential = startup_credential.clone();
            let project_id = project_id.clone();
            async move {
                match startup_credential {
                    Some((caller, credential)) => {
                        probe_health_authenticated(caller, credential, project_id).await
                    }
                    None => HealthState::Offline,
                }
            }
        };
        let requested = model.clone();
        let model_loaded = || {
            let startup_credential = startup_credential.clone();
            let project_id = project_id.clone();
            let requested = requested.clone();
            async move {
                let (Some((caller, credential)), Some(requested)) = (startup_credential, requested)
                else {
                    return false;
                };
                matches!(
                    load_model_status(caller, credential, project_id).await,
                    ObservatoryState::Available(result)
                        if result
                            .loaded
                            .as_ref()
                            .is_some_and(|loaded| loaded.model_id() == requested)
                )
            }
        };
        // A daemon that came up without its model — or that is still loading
        // one — is still a daemon: track the child exactly like a successful
        // start, then report what the wait observed.
        match self
            .verify_daemon_startup(&mut child, model.as_deref(), health, model_loaded)
            .await?
        {
            DaemonStartup::Serving => {}
            DaemonStartup::ModelMissing(failure) | DaemonStartup::StillStarting(failure) => {
                self.track_daemon_child(child).await;
                return Err(failure);
            }
        }
        match scope {
            CommandScope::Daemon => {
                self.track_daemon_child(child).await;
                Ok(None)
            }
            CommandScope::Project(active) => {
                let surfaces = load_active_surfaces(&active).await;
                let mut state = self.inner.lock().await;
                ensure_active_matches(&state, &active, &fence)?;
                replace_daemon_child(&mut state.daemon_child, child);
                finish_snapshot_locked(&mut state, fence, active, surfaces).map(Some)
            }
        }
    }

    /// The bounded post-spawn wait for one launch. One registry read sizes
    /// both the startup budget and the load meter's denominator, and the
    /// wait publishes its samples into this core's shared progress cell.
    async fn verify_daemon_startup<H, HF, M, MF>(
        &self,
        child: &mut Child,
        model: Option<&str>,
        health: H,
        model_loaded: M,
    ) -> DesktopResult<DaemonStartup>
    where
        H: FnMut() -> HF,
        HF: Future<Output = HealthState>,
        M: FnMut() -> MF,
        MF: Future<Output = bool>,
    {
        let model_bytes = registered_model_size(model).await;
        wait_for_daemon_serving(
            child,
            model,
            startup_budget_for_bytes(model_bytes),
            health,
            model_loaded,
            &self.startup_progress,
            model_bytes,
        )
        .await
    }

    /// Tracks a freshly started daemon child, collecting whatever handle it
    /// replaces. Every daemon this process spawned and left running is
    /// tracked, including one serving without its model.
    async fn track_daemon_child(&self, child: Child) {
        let mut state = self.inner.lock().await;
        replace_daemon_child(&mut state.daemon_child, child);
    }

    /// Reads the live model-load progress of a daemon start in flight, so a
    /// multi-minute load shows a moving meter instead of a bare spinner.
    ///
    /// This never takes the command gate: `start_daemon` holds it for the
    /// whole load, so a progress read queued behind it could only ever report
    /// a load that had already finished. Fence authorization and the
    /// staleness check synchronize through the state lock alone, exactly like
    /// `model_infer`.
    ///
    /// # Errors
    ///
    /// Returns a bounded error when the fence is invalid, stale, or reused.
    pub async fn daemon_startup_progress(
        &self,
        fence: CommandFence,
    ) -> DesktopResult<DaemonStartupProgressDto> {
        let scope = self.begin_scoped(&fence).await?;
        let progress = read_startup_progress(&self.startup_progress);
        let state = self.inner.lock().await;
        ensure_scope_matches(&state, &scope, &fence)?;
        Ok(progress.map_or(
            DaemonStartupProgressDto {
                model_id: None,
                phase: None,
                loaded_bytes: 0,
                total_bytes: 0,
                elapsed_seconds: 0,
            },
            |progress| DaemonStartupProgressDto {
                model_id: Some(progress.model_id),
                phase: Some(match progress.phase {
                    StartupPhase::Verifying => StartupPhaseDto::Verifying,
                    StartupPhase::Loading => StartupPhaseDto::Loading,
                }),
                loaded_bytes: progress.loaded_bytes,
                total_bytes: progress.total_bytes,
                elapsed_seconds: progress.elapsed_seconds,
            },
        ))
    }

    /// Requests an authenticated daemon stop. Under a project fence it
    /// returns a newly fenced snapshot; under the daemon authority it stops
    /// the daemon in its reserved scope and returns no snapshot.
    ///
    /// # Errors
    ///
    /// Returns a bounded error for stale authority, missing credentials, or a
    /// rejected/failed daemon lifecycle exchange.
    pub async fn stop_daemon(&self, fence: CommandFence) -> DesktopResult<Option<SnapshotDto>> {
        let _command = self.command_gate.lock().await;
        let scope = self.begin_scoped(&fence).await?;
        let caller = caller_id(CallerKind::Gui)
            .map_err(|error| DesktopErrorDto::unavailable(error.to_string(), None))?;
        let credential = load_credential(caller.clone()).await.map_err(|detail| {
            DesktopErrorDto::unavailable(detail, Some(GUI_REGISTRATION_RECOVERY.to_owned()))
        })?;
        // A daemon that stopped answering cannot be asked to stop, and that is
        // exactly when stopping matters most: a wedged daemon holds a
        // multi-gigabyte model and ignores SIGINT, so without this the owner
        // has no way to unload it short of `kill -9` in a terminal. This
        // process spawned that daemon and still holds its `Child`, so the
        // failed request is not the end of the road — `reap_daemon_child`
        // already waits `DAEMON_STOP_GRACE` and then kills. It only ever
        // touches a child this process started; a daemon we do not own still
        // reports the failure.
        let requested = request_daemon_stop_authenticated(
            caller.clone(),
            credential.clone(),
            scope.project_id(),
        )
        .await;
        match scope {
            CommandScope::Daemon => {
                let mut state = self.inner.lock().await;
                let owned = state.daemon_child.is_some();
                reap_daemon_child(&mut state.daemon_child);
                stop_outcome(requested, owned)?;
                Ok(None)
            }
            CommandScope::Project(active) => {
                let surfaces = load_surfaces_with_credential(
                    caller,
                    credential,
                    active.project_id.clone(),
                    request_project_root(&active.catalog.root),
                )
                .await;
                let mut state = self.inner.lock().await;
                ensure_active_matches(&state, &active, &fence)?;
                let owned = state.daemon_child.is_some();
                reap_daemon_child(&mut state.daemon_child);
                stop_outcome(requested, owned)?;
                finish_snapshot_locked(&mut state, fence, active, surfaces).map(Some)
            }
        }
    }

    /// Registers the GUI caller through the bundled PAM helper and refreshes
    /// the authenticated project snapshot.
    ///
    /// # Errors
    ///
    /// Returns a bounded error when the fence is stale, the fixed helper
    /// cannot be started, registration fails, or the helper exceeds its
    /// deadline.
    pub async fn register_gui_caller(&self, fence: CommandFence) -> DesktopResult<SnapshotDto> {
        let _command = self.command_gate.lock().await;
        let active = self.begin(&fence).await?;
        let (executable, root) = {
            let state = self.inner.lock().await;
            ensure_active_matches(&state, &active, &fence)?;
            (state.daemon_executable.clone(), active.catalog.root.clone())
        };
        run_gui_registration(&executable, &root).await?;
        let surfaces = load_active_surfaces(&active).await;
        self.finish_snapshot(fence, active, surfaces).await
    }

    /// Applies a decision to the exact retained approval request.
    ///
    /// # Errors
    ///
    /// Returns a bounded error when any fence or approval handle is stale, or
    /// when the authenticated refresh cannot be completed.
    pub async fn decide_approval(
        &self,
        fence: CommandFence,
        approval: ApprovalHandle,
        decision: ApprovalDecisionDto,
    ) -> DesktopResult<ApprovalDecisionResponseDto> {
        self.decide_approval_with(fence, approval, decision, decide_current_approval)
            .await
    }

    async fn decide_approval_with<F, Fut>(
        &self,
        fence: CommandFence,
        approval: ApprovalHandle,
        decision: ApprovalDecisionDto,
        decide: F,
    ) -> DesktopResult<ApprovalDecisionResponseDto>
    where
        F: FnOnce(PendingApproval, ApprovalDecision) -> Fut,
        Fut: Future<Output = Result<ApprovalDecisionView, ApprovalDecisionFailure>>,
    {
        let _command = self.command_gate.lock().await;
        approval.validate()?;
        let active = self.begin(&fence).await?;
        let pending = {
            let state = self.inner.lock().await;
            ensure_active_matches(&state, &active, &fence)?;
            state.approvals.get(&approval).cloned().ok_or_else(|| {
                DesktopErrorDto::stale("The approval handle is no longer current.")
            })?
        };
        if pending.project_id() != &active.project_id {
            return Err(DesktopErrorDto::stale(
                "The approval belongs to another project.",
            ));
        }
        let decision = decide(pending, decision.into())
            .await
            .map_err(|failure| DesktopErrorDto::unavailable(failure.detail, failure.recovery))?;
        let disposition = decision.disposition.into();
        let project_id = active.project_id.clone();
        let surfaces = approval_surfaces_with(decision.current, || async move {
            load_health_access(project_id).await
        })
        .await;
        let snapshot = self.finish_snapshot(fence, active, surfaces).await?;
        Ok(ApprovalDecisionResponseDto {
            disposition,
            snapshot,
        })
    }

    /// Loads a bounded 4 KiB preview through an opaque evidence handle.
    ///
    /// # Errors
    ///
    /// Returns a bounded error for stale authority, unavailable credentials,
    /// or an unexpected evidence protocol response.
    pub async fn load_evidence(
        &self,
        fence: CommandFence,
        handle: EvidenceHandleDto,
    ) -> DesktopResult<EvidenceDto> {
        let _command = self.command_gate.lock().await;
        handle.validate()?;
        let active = self.begin(&fence).await?;
        let protocol_handle = {
            let state = self.inner.lock().await;
            ensure_active_matches(&state, &active, &fence)?;
            state.evidence.get(&handle).cloned().ok_or_else(|| {
                DesktopErrorDto::stale("The evidence handle is no longer current.")
            })?
        };
        let caller = caller_id(CallerKind::Gui)
            .map_err(|error| DesktopErrorDto::unavailable(error.to_string(), None))?;
        let credential = load_credential(caller.clone()).await.map_err(|detail| {
            DesktopErrorDto::unavailable(detail, Some(GUI_REGISTRATION_RECOVERY.to_owned()))
        })?;
        let evidence = load_evidence(
            caller,
            credential,
            active.project_id.clone(),
            protocol_handle,
        )
        .await;
        let data = evidence_data(handle, evidence)?;
        let state = self.inner.lock().await;
        ensure_active_matches(&state, &active, &fence)?;
        Ok(EvidenceDto { fence, data })
    }

    /// Loads the daemon-global bounded flow catalog.
    ///
    /// When invoked with an active project fence, any of that project's
    /// legacy `.pam/flows` definitions absent from the global library are
    /// copied in once (idempotent by definition ID; the legacy files are left
    /// untouched) and reported back as `migrated`.
    ///
    /// # Errors
    ///
    /// Returns a bounded error for stale authority or an unsafe/invalid catalog.
    pub async fn flow_workspace(&self, fence: CommandFence) -> DesktopResult<FlowWorkspaceDto> {
        let root = pam_platform::flow_library_root().map_err(|_| flow_library_unavailable())?;
        self.flow_workspace_at(fence, root).await
    }

    async fn flow_workspace_at(
        &self,
        fence: CommandFence,
        root: PathBuf,
    ) -> DesktopResult<FlowWorkspaceDto> {
        let _command = self.command_gate.lock().await;
        let scope = self.begin_scoped(&fence).await?;
        let legacy_root = scope.project_root();
        let (model, migrated) = tokio::task::spawn_blocking(move || {
            // The global library root is not a project directory a user
            // creates; it is PAM's own user-data directory, so it may not
            // exist yet on a fresh install. Unlike a missing project root
            // (a real error), a missing global root is created on first use.
            std::fs::create_dir_all(&root).map_err(FlowEditorError::ProjectRoot)?;
            let mut model = FlowEditorModel::open(root)?;
            let migrated = match legacy_root {
                Some(legacy_root) => migrate_legacy_flows(&mut model, &legacy_root)?,
                None => Vec::new(),
            };
            Ok::<_, FlowEditorError>((model, migrated))
        })
        .await
        .map_err(|_| {
            DesktopErrorDto::new(
                DesktopErrorKind::Internal,
                "PAM could not join the flow catalog worker.",
                None,
            )
        })?
        .map_err(|error| flow_error(&error))?;
        let mut definitions = HashMap::new();
        let mut result = Vec::with_capacity(model.entries().len());
        for entry in model.entries() {
            let handle = FlowDefinitionHandle::new();
            definitions.insert(handle.clone(), entry.identity().file_name().to_owned());
            result.push(FlowDefinitionDto {
                handle,
                identity: identity_dto(entry.identity()),
            });
        }
        let mut state = self.inner.lock().await;
        ensure_scope_matches(&state, &scope, &fence)?;
        state.flow_workspace = Some(FlowWorkspaceState {
            model,
            definitions,
            documents: HashMap::new(),
        });
        Ok(FlowWorkspaceDto {
            fence,
            data: FlowWorkspaceDataDto {
                definitions: result,
                migrated,
            },
        })
    }

    /// Loads the bounded newest-first daemon activity feed.
    ///
    /// A `None` limit requests the daemon default; the daemon clamps any limit
    /// to its bounded maximum. Daemon failures are classified in the returned
    /// DTO: an explicit policy deny is blocked, everything else unavailable.
    ///
    /// # Errors
    ///
    /// Returns a bounded error when the fence is invalid, stale, or reused.
    pub async fn daemon_activity(
        &self,
        fence: CommandFence,
        limit: Option<u32>,
    ) -> DesktopResult<ActivityDto> {
        let _command = self.command_gate.lock().await;
        let scope = self.begin_scoped(&fence).await?;
        let observed = match observatory_credential().await {
            Ok((caller, credential)) => {
                load_daemon_activity(caller, credential, scope.project_id(), limit.unwrap_or(0))
                    .await
            }
            Err(state) => state,
        };
        let data = activity_dto(observed);
        let state = self.inner.lock().await;
        ensure_scope_matches(&state, &scope, &fence)?;
        Ok(data)
    }

    /// Loads the bounded oldest-first daemon diagnostic log slice.
    ///
    /// A `None` limit requests the daemon default; the daemon clamps any limit
    /// to its bounded maximum. Daemon failures are classified in the returned
    /// DTO: an explicit policy deny is blocked, everything else unavailable.
    ///
    /// # Errors
    ///
    /// Returns a bounded error when the fence is invalid, stale, or reused.
    pub async fn daemon_logs(
        &self,
        fence: CommandFence,
        limit: Option<u32>,
    ) -> DesktopResult<DaemonLogsDto> {
        let _command = self.command_gate.lock().await;
        let scope = self.begin_scoped(&fence).await?;
        let observed = match observatory_credential().await {
            Ok((caller, credential)) => {
                load_daemon_logs(caller, credential, scope.project_id(), limit.unwrap_or(0)).await
            }
            Err(state) => state,
        };
        let data = daemon_logs_dto(observed);
        let state = self.inner.lock().await;
        ensure_scope_matches(&state, &scope, &fence)?;
        Ok(data)
    }

    /// Loads the bounded per-day activity totals from the durable rollup.
    ///
    /// A `None` window requests the daemon default; the daemon clamps any
    /// window to its bounded maximum. Daemon failures are classified in the
    /// returned DTO: an explicit policy deny is blocked, everything else
    /// unavailable.
    ///
    /// # Errors
    ///
    /// Returns a bounded error when the fence is invalid, stale, or reused.
    pub async fn daemon_stats(
        &self,
        fence: CommandFence,
        days: Option<u32>,
    ) -> DesktopResult<DaemonStatsDto> {
        let _command = self.command_gate.lock().await;
        let scope = self.begin_scoped(&fence).await?;
        let observed = match observatory_credential().await {
            Ok((caller, credential)) => {
                load_daemon_stats(caller, credential, scope.project_id(), days.unwrap_or(0)).await
            }
            Err(state) => state,
        };
        let data = daemon_stats_dto(observed);
        let state = self.inner.lock().await;
        ensure_scope_matches(&state, &scope, &fence)?;
        Ok(data)
    }

    /// Loads the complete caller registry, including revoked callers.
    ///
    /// Daemon failures are classified in the returned DTO: an explicit policy
    /// deny is blocked, everything else unavailable.
    ///
    /// # Errors
    ///
    /// Returns a bounded error when the fence is invalid, stale, or reused.
    pub async fn caller_registry(&self, fence: CommandFence) -> DesktopResult<CallersDto> {
        let _command = self.command_gate.lock().await;
        let scope = self.begin_scoped(&fence).await?;
        let observed = match observatory_credential().await {
            Ok((caller, credential)) => {
                load_caller_registry(caller, credential, scope.project_id()).await
            }
            Err(state) => state,
        };
        let data = callers_dto(observed);
        let state = self.inner.lock().await;
        ensure_scope_matches(&state, &scope, &fence)?;
        Ok(data)
    }

    /// Loads the daemon's model surface: the loaded model and the catalog.
    ///
    /// Daemon failures are classified in the returned DTO: an explicit policy
    /// deny is blocked, everything else unavailable. When the daemon cannot
    /// be reached at all, the durable registered catalog still answers from
    /// the store — a daemon that is not serving has nothing loaded, but its
    /// registered models must stay reachable so one can be started with.
    ///
    /// # Errors
    ///
    /// Returns a bounded error when the fence is invalid, stale, or reused.
    pub async fn model_status(&self, fence: CommandFence) -> DesktopResult<ModelStatusDto> {
        let _command = self.command_gate.lock().await;
        let scope = self.begin_scoped(&fence).await?;
        // `unreachable` is what separates "the daemon has no model" from
        // "the daemon could not answer": only the second can be a load in
        // flight, and only the spawned child handle can tell.
        let (data, unreachable) = match observatory_credential().await {
            Ok((caller, credential)) => {
                match load_model_status(caller, credential, scope.project_id()).await {
                    // The registered catalog is durable store state: surface
                    // it even when the daemon is not serving. A missing or
                    // unreadable store keeps the original unavailable failure.
                    observed @ ObservatoryState::Unavailable { .. } => {
                        match registered_model_catalog().await {
                            Some(registered) => (
                                ModelStatusDto::Ok {
                                    loaded: None,
                                    registered,
                                    // The daemon is unreachable, so it has no
                                    // load failure to report — only the
                                    // durable catalog survives.
                                    load_failure: None,
                                    // Decided under the lock below, where the
                                    // spawned child handle lives.
                                    loading: false,
                                },
                                true,
                            ),
                            None => (model_status_dto(observed), false),
                        }
                    }
                    observed => (model_status_dto(observed), false),
                }
            }
            Err(state) => (model_status_dto(state), false),
        };
        let mut state = self.inner.lock().await;
        ensure_scope_matches(&state, &scope, &fence)?;
        if !unreachable {
            return Ok(data);
        }
        Ok(mark_model_loading(data, &mut state.daemon_child))
    }

    /// Runs one policy-gated direct inference on the embedded runtime.
    ///
    /// Inference can run for minutes, so it never takes the command gate:
    /// fence authorization and the post-exchange staleness check synchronize
    /// through the state lock alone, and the daemon policy-gates and
    /// serializes the generation itself. A missing or zero
    /// `max_output_tokens` requests the default budget and larger requests
    /// are clamped to the protocol maximum. Policy and approval refusals are
    /// classified as blocked in the returned DTO with recovery text;
    /// everything else is unavailable.
    ///
    /// # Errors
    ///
    /// Returns a bounded error when the fence is invalid, stale, or reused,
    /// or when the model identity or conversation violates the protocol
    /// contract before any daemon exchange.
    pub async fn model_infer(
        &self,
        fence: CommandFence,
        model: String,
        messages: Vec<ModelMessageDto>,
        max_output_tokens: Option<u32>,
    ) -> DesktopResult<ModelInferDto> {
        let scope = self.begin_scoped(&fence).await?;
        let messages = messages
            .into_iter()
            .map(|message| ModelMessage::new(message.role.into(), message.content))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| DesktopErrorDto::invalid_input(error.to_string()))?;
        let max_output_tokens = clamp_model_output_tokens(max_output_tokens);
        let observed = match observatory_credential().await {
            Ok((caller, credential)) => run_model_infer(
                caller,
                credential,
                scope.project_id(),
                model,
                messages,
                max_output_tokens,
            )
            .await
            .map_err(|error| DesktopErrorDto::invalid_input(error.to_string()))?,
            Err(state) => state,
        };
        let data = model_infer_dto(observed);
        let state = self.inner.lock().await;
        ensure_scope_matches(&state, &scope, &fence)?;
        Ok(data)
    }

    /// Starts a guided background import of a user-owned GGUF: PAM hashes
    /// the file and the accepted license notice itself, verifies the
    /// artifact through the shared import path, and registers it durably —
    /// all off the command gate, so hashing a multi-GB file never starves
    /// the rest of the GUI. Business refusals (already running) come back as
    /// `ModelImportDto::Unavailable`, not an `Err`; only fence problems do.
    ///
    /// # Errors
    ///
    /// Returns a bounded error when the fence is invalid, stale, or reused.
    pub async fn model_import(
        &self,
        fence: CommandFence,
        params: ModelImportParams,
    ) -> DesktopResult<ModelImportDto> {
        let _command = self.command_gate.lock().await;
        let scope = self.begin_scoped(&fence).await?;
        let data = match Arc::clone(&self.imports).start(params) {
            Ok(()) => ModelImportDto::Ok,
            Err(failure) => ModelImportDto::Unavailable {
                failure: unavailable_failure(
                    Some("import_already_running".to_owned()),
                    failure.detail,
                    failure.recovery,
                ),
            },
        };
        let state = self.inner.lock().await;
        ensure_scope_matches(&state, &scope, &fence)?;
        Ok(data)
    }

    /// Polls the current guided import status. There is no event bus in this
    /// codebase, so the GUI polls this instead of receiving progress —
    /// exactly like [`Self::model_download_status`].
    ///
    /// # Errors
    ///
    /// Returns a bounded error when the fence is invalid, stale, or reused.
    pub async fn model_import_status(
        &self,
        fence: CommandFence,
    ) -> DesktopResult<ModelImportStatusDto> {
        let _command = self.command_gate.lock().await;
        let scope = self.begin_scoped(&fence).await?;
        let snapshot = self.imports.snapshot();
        let data = ModelImportStatusDto {
            status: match snapshot.status {
                ModelImportStatusKind::Idle => ModelImportStatusKindDto::Idle,
                ModelImportStatusKind::Running => ModelImportStatusKindDto::Running,
                ModelImportStatusKind::Complete => ModelImportStatusKindDto::Complete,
                ModelImportStatusKind::Failed => ModelImportStatusKindDto::Failed,
            },
            model: snapshot.model.map(bounded_detail),
            stage: snapshot.stage.map(|stage| match stage {
                ModelImportStage::Hashing => ModelImportStageDto::Hashing,
                ModelImportStage::Registering => ModelImportStageDto::Registering,
            }),
            hashed_bytes: snapshot.hashed_bytes,
            total_bytes: snapshot.total_bytes,
            failure: snapshot.failure.map(|failure| {
                unavailable_failure(
                    Some("model_import_failed".to_owned()),
                    failure.detail,
                    failure.recovery,
                )
            }),
            calibrated: snapshot.calibrated,
        };
        let state = self.inner.lock().await;
        ensure_scope_matches(&state, &scope, &fence)?;
        Ok(data)
    }

    /// Previews a candidate GGUF before import: reads its bounded header and
    /// identity metadata without hashing, so the Models view can show the
    /// architecture, model name, and floor verdict before the user commits.
    ///
    /// This is a read-only local check on the user's own filesystem, like
    /// [`Self::model_import`]; failures are bounded unavailable data, never
    /// raw internals.
    ///
    /// # Errors
    ///
    /// Returns a bounded error when the fence is invalid, stale, or reused.
    pub async fn model_inspect(
        &self,
        fence: CommandFence,
        path: String,
    ) -> DesktopResult<ModelInspectDto> {
        let _command = self.command_gate.lock().await;
        let scope = self.begin_scoped(&fence).await?;
        let data = match run_model_inspect(PathBuf::from(path)).await {
            Ok(report) => ModelInspectDto::Ok {
                file_name: report.file_name,
                size_bytes: report.size_bytes,
                architecture: report.architecture,
                model_name: report.model_name,
                license: report.license,
                below_floor: report.below_floor,
                floor_bytes: MIN_RECOMMENDED_MODEL_BYTES,
            },
            Err(failure) => ModelInspectDto::Unavailable {
                failure: unavailable_failure(
                    Some("model_inspect_failed".to_owned()),
                    failure.detail,
                    failure.recovery,
                ),
            },
        };
        let state = self.inner.lock().await;
        ensure_scope_matches(&state, &scope, &fence)?;
        Ok(data)
    }

    /// Looks up a model's declared license on the public Hugging Face index,
    /// to prefill the manual import form when the GGUF metadata omits it.
    /// One bounded HTTPS search with a short timeout, like the connector
    /// self-tests; a miss is `Unavailable` data and the form falls back to
    /// manual entry.
    ///
    /// # Errors
    ///
    /// Returns a bounded error when the fence is invalid, stale, or reused.
    pub async fn model_license_discover(
        &self,
        fence: CommandFence,
        query: String,
    ) -> DesktopResult<ModelLicenseDiscoveryDto> {
        let _command = self.command_gate.lock().await;
        let scope = self.begin_scoped(&fence).await?;
        let data = match discover_license(&query).await {
            Ok(discovered) => ModelLicenseDiscoveryDto::Ok {
                repo_id: bounded_detail(discovered.repo_id),
                license_id: bounded_detail(discovered.license_id),
            },
            Err(failure) => ModelLicenseDiscoveryDto::Unavailable {
                failure: unavailable_failure(
                    Some("license_discovery_failed".to_owned()),
                    failure.detail,
                    failure.recovery,
                ),
            },
        };
        let state = self.inner.lock().await;
        ensure_scope_matches(&state, &scope, &fence)?;
        Ok(data)
    }

    /// Lists the curated, pre-verified model download presets, each already
    /// judged against this host. Static data plus one host memory probe:
    /// works under either the daemon authority or an active project, exactly
    /// like [`Self::model_status`].
    ///
    /// The fit verdict is computed here, not in the frontend, so the picker
    /// and the daemon's load-time admission share one rule. A failed probe
    /// leaves the budget `None` and every preset marked as fitting — PAM
    /// refuses nothing it could not measure.
    ///
    /// # Errors
    ///
    /// Returns a bounded error when the fence is invalid, stale, or reused.
    pub async fn model_presets(&self, fence: CommandFence) -> DesktopResult<ModelPresetsDto> {
        let _command = self.command_gate.lock().await;
        let scope = self.begin_scoped(&fence).await?;
        let host_total_bytes = host_memory_total_bytes().ok();
        let data = ModelPresetsDto {
            presets: model_presets::CATALOG
                .iter()
                .map(|preset| model_preset_dto(preset, host_total_bytes))
                .collect(),
            host_model_budget_bytes: host_total_bytes.map(model_presets::host_model_budget_bytes),
        };
        let state = self.inner.lock().await;
        ensure_scope_matches(&state, &scope, &fence)?;
        Ok(data)
    }

    /// Starts a guided background download for one curated preset. Business
    /// refusals (unknown preset, already running) come back as
    /// `ModelDownloadDto::Unavailable`, not an `Err`; only fence problems do.
    ///
    /// # Errors
    ///
    /// Returns a bounded error when the fence is invalid, stale, or reused.
    pub async fn model_download(
        &self,
        fence: CommandFence,
        preset_id: String,
    ) -> DesktopResult<ModelDownloadDto> {
        let _command = self.command_gate.lock().await;
        let scope = self.begin_scoped(&fence).await?;
        let data = match model_presets::find(&preset_id) {
            None => ModelDownloadDto::Unavailable {
                failure: unavailable_failure(
                    Some("unknown_preset".to_owned()),
                    "This preset is not offered by PAM.",
                    Some("Reload the preset list and select it again.".to_owned()),
                ),
            },
            Some(preset) => match Arc::clone(&self.downloads).start(*preset) {
                Ok(()) => ModelDownloadDto::Ok,
                Err(failure) => ModelDownloadDto::Unavailable {
                    failure: unavailable_failure(
                        Some("download_already_running".to_owned()),
                        failure.detail,
                        failure.recovery,
                    ),
                },
            },
        };
        let state = self.inner.lock().await;
        ensure_scope_matches(&state, &scope, &fence)?;
        Ok(data)
    }

    /// Polls the current guided download status. There is no event bus in
    /// this codebase, so the GUI polls this instead of receiving progress.
    ///
    /// # Errors
    ///
    /// Returns a bounded error when the fence is invalid, stale, or reused.
    pub async fn model_download_status(
        &self,
        fence: CommandFence,
    ) -> DesktopResult<ModelDownloadStatusDto> {
        let _command = self.command_gate.lock().await;
        let scope = self.begin_scoped(&fence).await?;
        let snapshot = self.downloads.snapshot();
        let data = ModelDownloadStatusDto {
            status: match snapshot.status {
                ModelDownloadStatusKind::Idle => ModelDownloadStatusKindDto::Idle,
                ModelDownloadStatusKind::Running => ModelDownloadStatusKindDto::Running,
                ModelDownloadStatusKind::Complete => ModelDownloadStatusKindDto::Complete,
                ModelDownloadStatusKind::Failed => ModelDownloadStatusKindDto::Failed,
                ModelDownloadStatusKind::Cancelled => ModelDownloadStatusKindDto::Cancelled,
            },
            preset_id: snapshot.preset_id,
            received_bytes: snapshot.received_bytes,
            total_bytes: snapshot.total_bytes,
            failure: snapshot.failure.map(|failure| {
                unavailable_failure(
                    Some("model_download_failed".to_owned()),
                    failure.detail,
                    failure.recovery,
                )
            }),
        };
        let state = self.inner.lock().await;
        ensure_scope_matches(&state, &scope, &fence)?;
        Ok(data)
    }

    /// Requests cancellation of the running guided download. The partial file
    /// stays on disk, so downloading the same preset again resumes. A refusal
    /// (no download running) comes back as `ModelDownloadDto::Unavailable`,
    /// not an `Err`; only fence problems do.
    ///
    /// # Errors
    ///
    /// Returns a bounded error when the fence is invalid, stale, or reused.
    pub async fn model_download_cancel(
        &self,
        fence: CommandFence,
    ) -> DesktopResult<ModelDownloadDto> {
        let _command = self.command_gate.lock().await;
        let scope = self.begin_scoped(&fence).await?;
        let data = match self.downloads.cancel() {
            Ok(()) => ModelDownloadDto::Ok,
            Err(failure) => ModelDownloadDto::Unavailable {
                failure: unavailable_failure(
                    Some("download_not_running".to_owned()),
                    failure.detail,
                    failure.recovery,
                ),
            },
        };
        let state = self.inner.lock().await;
        ensure_scope_matches(&state, &scope, &fence)?;
        Ok(data)
    }

    /// Probes total host physical memory, for a coarse "will this preset fit"
    /// hint in the picker. Advisory only: the daemon's llama.cpp admission
    /// check at load time stays authoritative.
    ///
    /// # Errors
    ///
    /// Returns a bounded error when the fence is invalid, stale, or reused,
    /// or when the host memory probe is unsupported or fails.
    pub async fn host_memory(&self, fence: CommandFence) -> DesktopResult<HostMemoryDto> {
        let _command = self.command_gate.lock().await;
        let scope = self.begin_scoped(&fence).await?;
        let data = host_memory_total_bytes()
            .map(|total_bytes| HostMemoryDto {
                total_bytes,
                supported_minimum_bytes: MIN_SUPPORTED_HOST_MEMORY_BYTES,
            })
            .map_err(|failure| DesktopErrorDto::unavailable(failure.detail, failure.recovery))?;
        let state = self.inner.lock().await;
        ensure_scope_matches(&state, &scope, &fence)?;
        Ok(data)
    }

    /// Reports Settings v1: where models download to, where PAM's local
    /// data lives, and the daemon's on-disk log location and size.
    ///
    /// Daemon-authority only, like [`Self::daemon_health`]: Settings has no
    /// project-specific behavior, so it never accepts a project fence.
    ///
    /// # Errors
    ///
    /// Returns a bounded error when the fence is not the exact daemon
    /// authority, its operation UUID was replayed, or the home directory or
    /// PAM's local data directory cannot be resolved.
    pub async fn app_settings(&self, fence: CommandFence) -> DesktopResult<AppSettingsDto> {
        let _command = self.command_gate.lock().await;
        self.begin_daemon(&fence).await?;
        settings_snapshot().map(|snapshot| app_settings_dto(&snapshot))
    }

    /// Persists (or clears, with `models_dir: None`) the one Settings v1
    /// preference: a custom models download directory. A future download
    /// reads it fresh at start, so no in-memory cache needs invalidating.
    ///
    /// # Errors
    ///
    /// Returns a bounded error when the fence is not the exact daemon
    /// authority, its operation UUID was replayed, or `models_dir` is
    /// relative, escapes with `..`, or names a directory PAM cannot create.
    pub async fn settings_update(
        &self,
        fence: CommandFence,
        models_dir: Option<String>,
    ) -> DesktopResult<AppSettingsDto> {
        let _command = self.command_gate.lock().await;
        self.begin_daemon(&fence).await?;
        let data_dir = settings_data_dir()?;
        let home = settings::resolve_home()
            .map_err(|failure| DesktopErrorDto::unavailable(failure.detail, failure.recovery))?;
        settings::update_models_dir(&data_dir, &home, models_dir)
            .map(|snapshot| app_settings_dto(&snapshot))
            .map_err(|failure| DesktopErrorDto::invalid_input(failure.detail))
    }

    /// Deletes the on-disk daemon log files, if any exist. Never touches the
    /// durable state store; the debug console keeps reading the daemon's
    /// in-memory ring buffer regardless.
    ///
    /// # Errors
    ///
    /// Returns a bounded error when the fence is not the exact daemon
    /// authority, its operation UUID was replayed, or a log file exists but
    /// cannot be removed.
    pub async fn logs_delete(&self, fence: CommandFence) -> DesktopResult<AppSettingsDto> {
        let _command = self.command_gate.lock().await;
        self.begin_daemon(&fence).await?;
        let data_dir = settings_data_dir()?;
        let home = settings::resolve_home()
            .map_err(|failure| DesktopErrorDto::unavailable(failure.detail, failure.recovery))?;
        settings::delete_logs(&data_dir, &home)
            .map(|snapshot| app_settings_dto(&snapshot))
            .map_err(|failure| DesktopErrorDto::unavailable(failure.detail, failure.recovery))
    }

    /// Validates that `path` is exactly one of today's Settings locations.
    /// The actual "open in Finder" side effect is Tauri-specific and lives in
    /// the desktop shell; this only guards against revealing an arbitrary
    /// filesystem path chosen by the frontend.
    ///
    /// # Errors
    ///
    /// Returns a bounded error when the fence is not the exact daemon
    /// authority, its operation UUID was replayed, or `path` does not match
    /// the models, data, flows, or logs directory.
    pub async fn reveal_path(&self, fence: CommandFence, path: String) -> DesktopResult<()> {
        let _command = self.command_gate.lock().await;
        self.begin_daemon(&fence).await?;
        let snapshot = settings_snapshot()?;
        if settings::is_known_location(&snapshot, Path::new(&path)) {
            Ok(())
        } else {
            Err(DesktopErrorDto::invalid_input(
                "This path is not a PAM Settings location.",
            ))
        }
    }

    /// Loads the complete connector registry without any credential material.
    ///
    /// `connector.list` is a baseline read: an explicit policy deny is
    /// blocked in the returned DTO, everything else unavailable.
    ///
    /// # Errors
    ///
    /// Returns a bounded error when the fence is invalid, stale, or reused.
    pub async fn connector_registry(&self, fence: CommandFence) -> DesktopResult<ConnectorsDto> {
        let _command = self.command_gate.lock().await;
        let scope = self.begin_scoped(&fence).await?;
        let observed = match observatory_credential().await {
            Ok((caller, credential)) => {
                load_connector_registry(caller, credential, scope.project_id()).await
            }
            Err(state) => state,
        };
        let data = connectors_dto(observed);
        let state = self.inner.lock().await;
        ensure_scope_matches(&state, &scope, &fence)?;
        Ok(data)
    }

    /// Applies one policy-gated connector configuration change.
    ///
    /// The optional credential secret passes through in memory only; it is
    /// never logged, retained, or echoed by any result. Policy and approval
    /// refusals are classified as blocked in the returned DTO with recovery
    /// text; everything else is unavailable.
    ///
    /// # Errors
    ///
    /// Returns a bounded error when the fence is invalid, stale, or reused,
    /// or when the connector identity or base URL violates the protocol
    /// contract before any daemon exchange.
    pub async fn connector_configure(
        &self,
        fence: CommandFence,
        params: ConnectorConfigureParams,
    ) -> DesktopResult<ConnectorConfigureDto> {
        let _command = self.command_gate.lock().await;
        let scope = self.begin_scoped(&fence).await?;
        let observed = match observatory_credential().await {
            Ok((caller, credential)) => run_connector_configure(
                caller,
                credential,
                scope.project_id(),
                params.connector,
                params.enabled,
                params.base_url,
                params.credential,
            )
            .await
            .map_err(|error| DesktopErrorDto::invalid_input(error.to_string()))?,
            Err(state) => state,
        };
        let data = connector_configure_dto(observed);
        let state = self.inner.lock().await;
        ensure_scope_matches(&state, &scope, &fence)?;
        Ok(data)
    }

    /// Runs one policy-gated connector self-test on the daemon.
    ///
    /// Policy and approval refusals are classified as blocked in the returned
    /// DTO with recovery text; everything else is unavailable.
    ///
    /// # Errors
    ///
    /// Returns a bounded error when the fence is invalid, stale, or reused,
    /// or when the connector identity violates the protocol contract before
    /// any daemon exchange.
    pub async fn connector_test(
        &self,
        fence: CommandFence,
        connector: String,
    ) -> DesktopResult<ConnectorTestDto> {
        let _command = self.command_gate.lock().await;
        let scope = self.begin_scoped(&fence).await?;
        let observed = match observatory_credential().await {
            Ok((caller, credential)) => {
                run_connector_test(caller, credential, scope.project_id(), connector)
                    .await
                    .map_err(|error| DesktopErrorDto::invalid_input(error.to_string()))?
            }
            Err(state) => state,
        };
        let data = connector_test_dto(observed);
        let state = self.inner.lock().await;
        ensure_scope_matches(&state, &scope, &fence)?;
        Ok(data)
    }

    /// Reads the GUI caller's daemon-scope capability grants.
    ///
    /// The grants are daemon-global, so this read carries no project identity
    /// and needs no active project.
    ///
    /// # Errors
    ///
    /// Returns a bounded error when the daemon authority fence is invalid or
    /// reused, the GUI caller identity is unreadable, or durable state is
    /// unavailable.
    pub async fn daemon_access(&self, fence: CommandFence) -> DesktopResult<DaemonAccessDto> {
        let _command = self.command_gate.lock().await;
        self.begin_daemon(&fence).await?;
        let (state_path, caller) = daemon_access_identity()?;
        read_daemon_access(state_path, caller).await
    }

    /// Reads the daemon's observed access boundary: TLS roots, proxy
    /// environment, `NO_PROXY`, and PAC state.
    ///
    /// The boundary is daemon-global, so this read carries no project identity
    /// and needs no active project.
    ///
    /// # Errors
    ///
    /// Returns a bounded error when the daemon authority fence is invalid or
    /// its operation UUID was replayed. Every other failure is classified in
    /// the returned DTO.
    pub async fn daemon_access_config(
        &self,
        fence: CommandFence,
    ) -> DesktopResult<AccessConfigDto> {
        let _command = self.command_gate.lock().await;
        self.begin_daemon(&fence).await?;
        let access = match caller_id(CallerKind::Gui) {
            Err(error) => AccessConfigState::Unavailable {
                code: None,
                detail: error.to_string(),
                recovery: None,
            },
            Ok(caller) => match load_credential(caller.clone()).await {
                Err(detail) => AccessConfigState::Unavailable {
                    code: None,
                    detail,
                    recovery: Some(GUI_REGISTRATION_RECOVERY.to_owned()),
                },
                Ok(credential) => {
                    load_access_config(caller, credential, ProjectId::daemon_scope()).await
                }
            },
        };
        Ok(access_dto(access))
    }

    /// Grants or revokes one daemon-scope capability for the GUI caller.
    ///
    /// This is the owner's explicit, reversible act: nothing else in the GUI
    /// writes a capability grant.
    ///
    /// # Errors
    ///
    /// Returns a bounded error when the daemon authority fence is invalid or
    /// reused, the capability is not one the GUI uses, the GUI caller identity
    /// is unreadable, or durable state is unavailable.
    pub async fn set_daemon_access(
        &self,
        fence: CommandFence,
        capability: String,
        granted: bool,
    ) -> DesktopResult<DaemonAccessDto> {
        let _command = self.command_gate.lock().await;
        self.begin_daemon(&fence).await?;
        let (state_path, caller) = daemon_access_identity()?;
        update_daemon_access(state_path, caller, capability, granted).await
    }

    /// Converts one TOML flow document into its structured definition, locally.
    ///
    /// Flow definitions are daemon-global, so this transform also runs under
    /// the daemon authority without an active project. Parse and validation
    /// failures are classified in the returned DTO; no daemon exchange is
    /// involved.
    ///
    /// # Errors
    ///
    /// Returns a bounded error when the fence is invalid, stale, or reused.
    pub async fn flow_graph(
        &self,
        fence: CommandFence,
        source: String,
    ) -> DesktopResult<FlowGraphDto> {
        let _command = self.command_gate.lock().await;
        let scope = self.begin_scoped(&fence).await?;
        let data = flow_graph_data(&source);
        let state = self.inner.lock().await;
        ensure_scope_matches(&state, &scope, &fence)?;
        Ok(data)
    }

    /// Serializes one structured flow definition into normalized TOML, locally.
    ///
    /// Flow definitions are daemon-global, so this transform also runs under
    /// the daemon authority without an active project. Parse and validation
    /// failures are classified in the returned DTO; no daemon exchange is
    /// involved.
    ///
    /// # Errors
    ///
    /// Returns a bounded error when the fence is invalid, stale, or reused.
    pub async fn flow_compose(
        &self,
        fence: CommandFence,
        definition: String,
    ) -> DesktopResult<FlowComposeDto> {
        let _command = self.command_gate.lock().await;
        let scope = self.begin_scoped(&fence).await?;
        let data = flow_compose_data(&definition);
        let state = self.inner.lock().await;
        ensure_scope_matches(&state, &scope, &fence)?;
        Ok(data)
    }

    /// Scans and persists one scope's bounded local agent artifact inventory:
    /// the active project, or global roots only under the daemon authority.
    ///
    /// # Errors
    ///
    /// Returns a bounded error for stale authority, incomplete filesystem scans,
    /// malformed plugin registries, or unavailable durable state.
    pub async fn skill_inventory(&self, fence: CommandFence) -> DesktopResult<SkillInventoryDto> {
        let _command = self.command_gate.lock().await;
        let scope = self.begin_scoped(&fence).await?;
        let environment = SkillInventoryEnvironment::discover(scope.project_root())?;
        let data = load_skill_inventory(scope.project_id(), environment).await?;
        let state = self.inner.lock().await;
        ensure_scope_matches(&state, &scope, &fence)?;
        Ok(SkillInventoryDto { fence, data })
    }

    /// Executes one exact metadata-only canonical skill-library action.
    ///
    /// Actions that touch only the global manifest also run under the daemon
    /// authority; per-project actions require an active project.
    ///
    /// # Errors
    ///
    /// Returns a bounded error when the fence is stale or reused, the action needs a project the
    /// daemon scope does not have, the global p-track home or derived agent root is unsafe, or the
    /// exact library action cannot be completed.
    pub async fn manage_skill_library(
        &self,
        request: SkillLibraryRequest,
    ) -> DesktopResult<SkillLibraryDto> {
        self.manage_skill_library_with(request, |scope, action| async move {
            let root = scope.project_root();
            let project = project_key(&scope.project_id())?;
            tokio::task::spawn_blocking(move || {
                let environment = SkillLibraryEnvironment::discover(root.as_deref())?;
                execute_skill_library(&environment, project, action)
            })
            .await
            .map_err(|_| {
                DesktopErrorDto::unavailable(
                    "PAM could not join the bounded skill library action.",
                    Some("Retry the exact skill library action.".to_owned()),
                )
            })?
        })
        .await
    }

    async fn manage_skill_library_with<F, Fut>(
        &self,
        request: SkillLibraryRequest,
        work: F,
    ) -> DesktopResult<SkillLibraryDto>
    where
        F: FnOnce(CommandScope, SkillLibraryAction) -> Fut,
        Fut: Future<Output = DesktopResult<SkillLibraryDataDto>>,
    {
        let _command = self.command_gate.lock().await;
        let fence = request.fence();
        let action = request.into_action();
        let scope = self.begin_scoped(&fence).await?;
        if matches!(scope, CommandScope::Daemon) && action.requires_project() {
            return Err(project_scope_required());
        }
        let result = work(scope.clone(), action).await;
        let state = self.inner.lock().await;
        ensure_scope_matches(&state, &scope, &fence)?;
        let data = result?;
        Ok(SkillLibraryDto { fence, data })
    }

    /// Loads the latest durable audit for one scope without running an evaluator.
    ///
    /// # Errors
    ///
    /// Returns a bounded error for stale authority or invalid/unavailable durable state.
    pub async fn load_skill_audit(&self, fence: CommandFence) -> DesktopResult<SkillAuditDto> {
        let _command = self.command_gate.lock().await;
        let scope = self.begin_scoped(&fence).await?;
        let environment = SkillInventoryEnvironment::discover(scope.project_root())?;
        let data = load_persisted_skill_audit(scope.project_id(), environment.state_path()).await?;
        let state = self.inner.lock().await;
        ensure_scope_matches(&state, &scope, &fence)?;
        Ok(SkillAuditDto { fence, data })
    }

    /// Runs and persists one fresh bounded audit for the fenced scope.
    ///
    /// # Errors
    ///
    /// Returns a bounded error for stale authority, incomplete scans, evaluator setup failures,
    /// or invalid/unavailable durable state.
    pub async fn run_skill_audit(&self, fence: CommandFence) -> DesktopResult<SkillAuditDto> {
        let _command = self.command_gate.lock().await;
        let scope = self.begin_scoped(&fence).await?;
        let environment = SkillInventoryEnvironment::discover(scope.project_root())?;
        let data = run_skill_audit_report(scope.project_id(), environment).await?;
        let state = self.inner.lock().await;
        ensure_scope_matches(&state, &scope, &fence)?;
        Ok(SkillAuditDto {
            fence,
            data: Some(data),
        })
    }

    /// Opens one flow selected only by an opaque catalog handle.
    ///
    /// # Errors
    ///
    /// Returns a bounded error for stale handles or unsafe catalog state.
    pub async fn open_flow(
        &self,
        fence: CommandFence,
        definition: FlowDefinitionHandle,
    ) -> DesktopResult<FlowDocumentDto> {
        let _command = self.command_gate.lock().await;
        definition.validate()?;
        let scope = self.begin_scoped(&fence).await?;
        let mut state = self.inner.lock().await;
        ensure_scope_matches(&state, &scope, &fence)?;
        let workspace = workspace_mut(&mut state)?;
        let selector = workspace.definitions.get(&definition).ok_or_else(|| {
            DesktopErrorDto::stale("The flow definition handle is no longer current.")
        })?;
        let document = workspace
            .model
            .open_document(selector)
            .map_err(|error| flow_error(&error))?;
        let handle = FlowDocumentHandle::new();
        let data = FlowDocumentDataDto {
            handle: handle.clone(),
            identity: document.saved_identity().map(identity_dto),
            source: document.source().to_owned(),
        };
        workspace.documents.insert(handle, document);
        Ok(FlowDocumentDto { fence, data })
    }

    /// Validates, dry-runs, and diffs a retained flow document.
    ///
    /// # Errors
    ///
    /// Returns bounded stale, size, syntax, schema, or revision feedback.
    pub async fn validate_flow(
        &self,
        fence: CommandFence,
        document: FlowDocumentHandle,
        source: String,
    ) -> DesktopResult<FlowReviewDto> {
        let _command = self.command_gate.lock().await;
        document.validate()?;
        let scope = self.begin_scoped(&fence).await?;
        let mut state = self.inner.lock().await;
        ensure_scope_matches(&state, &scope, &fence)?;
        let workspace = workspace_mut(&mut state)?;
        let draft = workspace.documents.get_mut(&document).ok_or_else(|| {
            DesktopErrorDto::stale("The flow document handle is no longer current.")
        })?;
        draft
            .replace_source(source)
            .map_err(|error| flow_error(&error))?;
        let validation = draft.validate().map_err(|error| flow_error(&error))?;
        let dry_run = draft.dry_run().map_err(|error| flow_error(&error))?;
        let diff = draft.version_diff().map_err(|error| flow_error(&error))?;
        let data = FlowReviewDataDto {
            document,
            identity: identity_dto(validation.identity()),
            normalized_toml: validation.normalized_toml().to_owned(),
            dry_run: dry_run_dto(&dry_run),
            diff: diff_dto(&diff),
        };
        Ok(FlowReviewDto { fence, data })
    }

    /// Atomically saves a retained, validated flow document.
    ///
    /// # Errors
    ///
    /// Returns bounded stale, conflict, validation, publication, or I/O errors.
    pub async fn save_flow(
        &self,
        fence: CommandFence,
        document: FlowDocumentHandle,
        source: String,
    ) -> DesktopResult<FlowSaveDto> {
        let _command = self.command_gate.lock().await;
        document.validate()?;
        let scope = self.begin_scoped(&fence).await?;
        let mut state = self.inner.lock().await;
        ensure_scope_matches(&state, &scope, &fence)?;
        let workspace = workspace_mut(&mut state)?;
        let draft = workspace.documents.get_mut(&document).ok_or_else(|| {
            DesktopErrorDto::stale("The flow document handle is no longer current.")
        })?;
        draft
            .replace_source(source)
            .map_err(|error| flow_error(&error))?;
        let interaction = draft.prepare_save().map_err(|error| flow_error(&error))?;
        let saved = draft
            .commit_save(interaction)
            .map_err(|error| flow_error(&error))?;
        workspace
            .model
            .reload()
            .map_err(|error| post_save_reload_error(&error))?;
        let data = FlowSaveDataDto {
            document,
            identity: identity_dto(saved.identity()),
            created: saved.created(),
            durability_confirmed: saved.durability_confirmed(),
            cleanup_complete: saved.cleanup_complete(),
        };
        Ok(FlowSaveDto { fence, data })
    }

    async fn begin(&self, fence: &CommandFence) -> DesktopResult<ActiveProject> {
        fence.validate()?;
        let mut state = self.inner.lock().await;
        let active = state
            .active
            .as_ref()
            .ok_or_else(|| DesktopErrorDto::stale("No project is active."))?
            .clone();
        if active.catalog.handle != fence.project_handle || active.generation != fence.generation {
            return Err(DesktopErrorDto::stale(
                "The project or generation fence is stale.",
            ));
        }
        if state
            .used_operations
            .iter()
            .any(|operation| operation == &fence.operation_id)
        {
            return Err(DesktopErrorDto::new(
                DesktopErrorKind::Conflict,
                "This operation UUID was already used for the active generation.",
                Some("Create a new operation UUID and retry.".to_owned()),
            ));
        }
        state.used_operations.push_back(fence.operation_id.clone());
        while state.used_operations.len() > MAX_OPERATIONS {
            state.used_operations.pop_front();
        }
        Ok(active)
    }

    /// Authorizes one daemon-scoped command under the constant daemon
    /// authority, without requiring an active project.
    async fn begin_daemon(&self, fence: &CommandFence) -> DesktopResult<()> {
        fence.operation_id.validate()?;
        if fence.project_handle.as_str() != DAEMON_AUTHORITY
            || fence.generation.as_str() != DAEMON_AUTHORITY
        {
            return Err(DesktopErrorDto::invalid_input(
                "Daemon-scoped commands require the exact daemon authority fence.",
            ));
        }
        let mut state = self.inner.lock().await;
        if state
            .daemon_operations
            .iter()
            .any(|operation| operation == &fence.operation_id)
        {
            return Err(DesktopErrorDto::new(
                DesktopErrorKind::Conflict,
                "This operation UUID was already used for the daemon scope.",
                Some("Create a new operation UUID and retry.".to_owned()),
            ));
        }
        state
            .daemon_operations
            .push_back(fence.operation_id.clone());
        while state.daemon_operations.len() > MAX_OPERATIONS {
            state.daemon_operations.pop_front();
        }
        Ok(())
    }

    /// Routes one fence to the daemon authority or the active project.
    async fn begin_scoped(&self, fence: &CommandFence) -> DesktopResult<CommandScope> {
        if is_daemon_fence(fence) {
            self.begin_daemon(fence).await?;
            Ok(CommandScope::Daemon)
        } else {
            Ok(CommandScope::Project(self.begin(fence).await?))
        }
    }

    /// Probes daemon health under the daemon authority, without any project.
    ///
    /// # Errors
    ///
    /// Returns a bounded error when the daemon authority fence is invalid or
    /// its operation UUID was replayed.
    pub async fn daemon_health(&self, fence: CommandFence) -> DesktopResult<HealthDto> {
        let _command = self.command_gate.lock().await;
        self.begin_daemon(&fence).await?;
        let health = match caller_id(CallerKind::Gui) {
            Err(error) => HealthState::Degraded {
                detail: error.to_string(),
                recovery: None,
            },
            Ok(caller) => match load_credential(caller.clone()).await {
                Err(detail) => HealthState::Degraded {
                    detail,
                    recovery: Some(GUI_REGISTRATION_RECOVERY.to_owned()),
                },
                Ok(credential) => {
                    probe_health_authenticated(caller, credential, ProjectId::daemon_scope()).await
                }
            },
        };
        Ok(health_dto(health))
    }

    async fn finish_snapshot(
        &self,
        fence: CommandFence,
        active: ActiveProject,
        surfaces: SurfaceBundle,
    ) -> DesktopResult<SnapshotDto> {
        let mut state = self.inner.lock().await;
        ensure_active_matches(&state, &active, &fence)?;
        finish_snapshot_locked(&mut state, fence, active, surfaces)
    }
}

/// A global daemon launch has no project root: it spawns from the user home
/// directory, or the startup root when no home directory is known.
fn daemon_start_cwd(fallback: &Path) -> PathBuf {
    std::env::home_dir().unwrap_or_else(|| fallback.to_path_buf())
}

fn default_daemon_executable() -> PathBuf {
    // Single-binary product: the daemon is this same executable in
    // `pam daemon` mode.
    std::env::current_exe().unwrap_or_else(|_| PathBuf::from(executable_name("pam")))
}

/// Best-effort append capture for the spawned daemon's stderr, next to the
/// daemon's own rotating log. Falls back to discarding when unavailable.
fn daemon_stderr_capture() -> Stdio {
    let Ok(data_dir) = user_data_dir() else {
        return Stdio::null();
    };
    let logs = data_dir.join("logs");
    if std::fs::create_dir_all(&logs).is_err() {
        return Stdio::null();
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(logs.join("daemon-stderr.log"))
        .map_or_else(|_| Stdio::null(), Stdio::from)
}

/// Recovery text pointing at the spawned daemon's captured stderr log — the
/// same file `daemon_stderr_capture` appends to. Falls back to naming the
/// file when the data directory cannot be resolved.
fn daemon_stderr_log_hint() -> String {
    let location = user_data_dir().map_or_else(
        |_| "logs/daemon-stderr.log under the PAM data directory".to_owned(),
        |dir| {
            dir.join("logs")
                .join("daemon-stderr.log")
                .display()
                .to_string()
        },
    );
    format!("See the daemon startup log at {location} for the failure reason.")
}

/// What the bounded startup verification observed, for a daemon that is up.
/// Every variant leaves the child alive and the caller owning it.
#[derive(Debug)]
pub(crate) enum DaemonStartup {
    /// Serving, with the requested model loaded when one was requested.
    Serving,
    /// Serving without the model it was started with. Carries the failure to
    /// report once the live child is tracked.
    ModelMissing(DesktopErrorDto),
    /// The budget ran out with the process still running: the model load is
    /// still in flight. Carries the notice to report once the live child is
    /// tracked.
    StillStarting(DesktopErrorDto),
}

/// The bounded wait for one launch. The daemon has to open its store, bind,
/// and answer health; when it was started with a model it must additionally
/// hash and map that artifact, so the budget grows with the artifact's size
/// on disk. Exceeding the budget is never fatal — it only ends the GUI's
/// wait — so the derivation stays conservative rather than exact.
fn startup_budget_for_bytes(model_bytes: Option<u64>) -> Duration {
    let load = model_bytes.map_or(0, |bytes| bytes.div_ceil(MODEL_LOAD_BYTES_PER_MINUTE));
    (STARTUP_BASE_TIMEOUT + Duration::from_mins(load)).min(STARTUP_TIMEOUT_CAP)
}

/// The recorded artifact size for `model`, from its registry row. It sizes
/// both the startup budget and the load meter's denominator. An unknown or
/// unregistered model has none; the daemon's own store read is the authority
/// on whether it exists at all.
async fn registered_model_size(model: Option<&str>) -> Option<u64> {
    let model = model?;
    registered_model_catalog().await.and_then(|catalog| {
        catalog
            .iter()
            .find(|summary| summary.model_id == model)
            .map(|summary| summary.size_bytes)
    })
}

/// What a start is doing right now, inferred from the child alone — no new
/// protocol message, no IPC into a daemon that answers nothing yet.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StartupPhase {
    /// Revalidating the artifact: a full-file SHA-256 that streams the GGUF,
    /// so resident memory says nothing at all about how far it has got. The
    /// GUI reports elapsed time here rather than a bar stuck at zero.
    Verifying,
    /// Mapping the weights: the resident-memory ramp is a real signal.
    Loading,
}

/// One sample of a start's model load. `loaded_bytes` is the high-water
/// resident set size clamped to the artifact size, and is zero for the whole
/// verification phase.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StartupProgress {
    pub(crate) model_id: String,
    pub(crate) phase: StartupPhase,
    pub(crate) loaded_bytes: u64,
    pub(crate) total_bytes: u64,
    pub(crate) elapsed_seconds: u64,
}

/// The resident-memory reading that separates a start's two phases. Measured
/// on a 39.2 GB artifact: 155 seconds flat at ~67 MB while it was hashed,
/// then 7.3 GB on the very next sample once the weights began mapping. A
/// gigabyte sits far above the hashing floor and far below the first mapping
/// sample, so the crossing is unambiguous.
///
/// ponytail: a fixed floor, so an artifact smaller than it stays in the
/// verification phase for its whole start. Those load in seconds — there is
/// no multi-minute wait to meter.
const LOAD_PHASE_RSS_FLOOR: u64 = 1024 * 1024 * 1024;

/// Folds one start's resident-memory samples into what the GUI shows.
pub(crate) struct StartupSampler {
    model_id: String,
    total_bytes: u64,
    observed_peak: u64,
}

impl StartupSampler {
    pub(crate) const fn new(model_id: String, total_bytes: u64) -> Self {
        Self {
            model_id,
            total_bytes,
            observed_peak: 0,
        }
    }

    /// Folds one resident-memory reading in.
    ///
    /// The reported value is the high-water mark, never the raw reading:
    /// resident memory is not monotonic and does not converge on the artifact
    /// size, because the Metal backend releases mapped pages as it settles. A
    /// measured 39.2 GB load peaked at 31.7 GB and settled at 15.8 GB, so a
    /// bar driven by the raw reading would walk backwards and none of them
    /// would ever reach the artifact size.
    pub(crate) fn fold(&mut self, resident_bytes: u64, elapsed: Duration) -> StartupProgress {
        self.observed_peak = self.observed_peak.max(resident_bytes);
        let loading = self.observed_peak >= LOAD_PHASE_RSS_FLOOR;
        StartupProgress {
            model_id: self.model_id.clone(),
            phase: if loading {
                StartupPhase::Loading
            } else {
                StartupPhase::Verifying
            },
            loaded_bytes: if loading {
                self.observed_peak.min(self.total_bytes)
            } else {
                0
            },
            total_bytes: self.total_bytes,
            elapsed_seconds: elapsed.as_secs(),
        }
    }
}

/// Where a start publishes its progress and `daemon_startup_progress` reads
/// it. A plain `std` mutex, never a tokio one: every access is a single
/// clone or store, so no guard is ever held across an await.
pub(crate) type StartupProgressCell = Arc<std::sync::Mutex<Option<StartupProgress>>>;

pub(crate) fn read_startup_progress(cell: &StartupProgressCell) -> Option<StartupProgress> {
    cell.lock().expect("startup progress lock").clone()
}

/// The spawned daemon's resident set size in bytes, or `None` when it cannot
/// be read.
///
/// ponytail: shells out to `ps` rather than binding libc or a process crate,
/// exactly like the `sysctl` host-memory probe. RSS is a progress *signal*,
/// not an accounting of loaded weights: the daemon's own model status stays
/// authoritative for "loaded".
#[cfg(target_os = "macos")]
fn process_resident_bytes(pid: u32) -> Option<u64> {
    let output = Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    // macOS `ps` reports the resident set in KiB.
    std::str::from_utf8(&output.stdout)
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()?
        .checked_mul(1024)
}

#[cfg(not(target_os = "macos"))]
const fn process_resident_bytes(_pid: u32) -> Option<u64> {
    None
}

/// Bounded post-spawn verification: the daemon must keep running AND serve
/// authenticated health before `start_daemon` may report success. When a
/// model was requested, that model must additionally be reported loaded —
/// health alone is not proof, because a daemon whose model load failed
/// still serves, just without the model.
///
/// A serving daemon whose model is missing is `Ok(ModelMissing)`, not an
/// error: the daemon's own degrade path deliberately keeps it running, so
/// killing it here would undo that. Its child stays alive for the caller to
/// track, which then reports the carried failure.
///
/// The only genuine failure is a process that exited during startup, which
/// `try_wait` has already reaped. A daemon still running when the budget
/// expires is `Ok(StillStarting)`: it is mid-load, and killing it there is
/// exactly what destroyed a 39 GB load mid-Metal-init (#34). Its child stays
/// alive for the caller to track, so it never becomes an untracked daemon
/// holding the ownership lock.
///
/// Only `HealthState::Healthy` is definitive: the daemon binds its socket
/// before loading the model, so mid-load it is alive but deaf and the health
/// probe's timeout disambiguation reports that as `Degraded` ("running but
/// did not respond in time"). Treating that as serving would fail a healthy
/// load seconds in, so `Degraded` and `Offline` both keep the poll going
/// until the deadline.
async fn wait_for_daemon_serving<H, HF, M, MF>(
    child: &mut Child,
    model: Option<&str>,
    budget: Duration,
    health: H,
    model_loaded: M,
    progress: &StartupProgressCell,
    model_bytes: Option<u64>,
) -> DesktopResult<DaemonStartup>
where
    H: FnMut() -> HF,
    HF: Future<Output = HealthState>,
    M: FnMut() -> MF,
    MF: Future<Output = bool>,
{
    let outcome = poll_daemon_startup(
        child,
        model,
        budget,
        health,
        model_loaded,
        progress,
        model_bytes,
    )
    .await;
    // Serving, model missing, still starting or exited: the wait is over
    // either way, so the meter must not linger on its last sample.
    *progress.lock().expect("startup progress lock") = None;
    outcome
}

/// The poll itself. Each tick samples the child's resident set size, so the
/// GUI can show how far a load that answers nothing yet has actually got.
async fn poll_daemon_startup<H, HF, M, MF>(
    child: &mut Child,
    model: Option<&str>,
    budget: Duration,
    mut health: H,
    mut model_loaded: M,
    progress: &StartupProgressCell,
    model_bytes: Option<u64>,
) -> DesktopResult<DaemonStartup>
where
    H: FnMut() -> HF,
    HF: Future<Output = HealthState>,
    M: FnMut() -> MF,
    MF: Future<Output = bool>,
{
    let started = tokio::time::Instant::now();
    let deadline = started + budget;
    // A start with no model — or with one the registry has no size for — has
    // no denominator, so it publishes nothing and the GUI keeps its plain
    // "starting" line.
    let mut sampler = model
        .zip(model_bytes.filter(|bytes| *bytes > 0))
        .map(|(model, bytes)| StartupSampler::new(model.to_owned(), bytes));
    loop {
        if let Some(status) = startup_exit_status(child)? {
            return Err(daemon_exited_during_startup(status));
        }
        if let Some(sampler) = sampler.as_mut()
            && let Some(resident) = process_resident_bytes(child.id())
        {
            *progress.lock().expect("startup progress lock") =
                Some(sampler.fold(resident, started.elapsed()));
        }
        match health().await {
            HealthState::Offline | HealthState::Degraded { .. } => {}
            HealthState::Healthy { .. } => {
                let Some(model) = model else {
                    return Ok(DaemonStartup::Serving);
                };
                if model_loaded().await {
                    return Ok(DaemonStartup::Serving);
                }
                return Ok(DaemonStartup::ModelMissing(DesktopErrorDto::unavailable(
                    format!("The local daemon is running, but the model {model} failed to load."),
                    Some(daemon_stderr_log_hint()),
                )));
            }
        }
        let now = tokio::time::Instant::now();
        if now >= deadline {
            return match startup_exit_status(child)? {
                Some(status) => Err(daemon_exited_during_startup(status)),
                None => Ok(DaemonStartup::StillStarting(still_starting_notice(budget))),
            };
        }
        tokio::time::sleep(STARTUP_POLL_INTERVAL.min(deadline - now)).await;
    }
}

/// One liveness read on the spawned daemon, with the bounded failure for a
/// handle that cannot be inspected at all.
fn startup_exit_status(child: &mut Child) -> DesktopResult<Option<std::process::ExitStatus>> {
    child.try_wait().map_err(|error| {
        DesktopErrorDto::unavailable(
            "PAM could not inspect the local daemon process.",
            Some(error.to_string()),
        )
    })
}

fn daemon_exited_during_startup(status: std::process::ExitStatus) -> DesktopErrorDto {
    DesktopErrorDto::unavailable(
        format!("The local daemon exited during startup with {status}."),
        Some(daemon_stderr_log_hint()),
    )
}

/// Not a failure: the daemon is running, it just has not finished loading.
/// The user is told to wait rather than left with a silent unreachable pill.
fn still_starting_notice(budget: Duration) -> DesktopErrorDto {
    DesktopErrorDto::unavailable(
        format!(
            "The local daemon is still starting: it is running but had not finished loading after {} seconds.",
            budget.as_secs()
        ),
        Some("Leave PAM running; the model panel reports the load when it finishes.".to_owned()),
    )
}

/// The detached daemon launch. `--recover` is idempotent: it only clears a
/// stale socket, and a live daemon still holds the ownership lock. A daemon
/// that dies at startup must leave its reason somewhere, so stderr lands next
/// to the daemon's own log file.
fn daemon_start_command(
    executable: &Path,
    model: Option<&str>,
    spawn_root: &Path,
    grant: String,
) -> Command {
    let mut command = Command::new(executable);
    command
        .args(["daemon", "--recover"])
        .args(model.map(|key| format!("--model={key}")))
        .env(pam_platform::LAUNCH_GRANT_ENV, grant)
        .current_dir(spawn_root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(daemon_stderr_capture());
    #[cfg(unix)]
    command.process_group(0);
    command
}

fn gui_registration_command(executable: &Path, root: &Path) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(executable);
    command
        .args(GUI_REGISTRATION_ARGS)
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        // The helper prints only sanitized, bounded diagnostics; its first
        // stderr line is the failure reason surfaced to the user.
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    command
}

async fn run_gui_registration(executable: &Path, root: &Path) -> DesktopResult<()> {
    let mut command = gui_registration_command(executable, root);
    let output = tokio::time::timeout(GUI_REGISTRATION_TIMEOUT, command.output())
        .await
        .map_err(|_| {
            DesktopErrorDto::unavailable(
                "PAM GUI caller registration exceeded its 15 second deadline.",
                Some("Retry GUI caller registration.".to_owned()),
            )
        })?
        .map_err(|error| {
            DesktopErrorDto::unavailable(
                "PAM could not start its bundled caller-registration helper.",
                Some(error.to_string()),
            )
        })?;
    if !output.status.success() {
        return Err(DesktopErrorDto::unavailable(
            registration_failure_detail(&output),
            Some("Retry registration or inspect the local PAM data store.".to_owned()),
        ));
    }
    Ok(())
}

pub(crate) fn registration_failure_detail(output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    match stderr.lines().map(str::trim).find(|line| !line.is_empty()) {
        Some(reason) => format!("PAM GUI caller registration failed: {reason}"),
        None => format!("PAM GUI caller registration failed with {}.", output.status),
    }
}

#[cfg(windows)]
fn executable_name(stem: &str) -> String {
    format!("{stem}.exe")
}

#[cfg(not(windows))]
fn executable_name(stem: &str) -> String {
    stem.to_owned()
}

struct SurfaceBundle {
    health: HealthState,
    current: CurrentState,
    access: AccessConfigState,
}

async fn load_active_surfaces(active: &ActiveProject) -> SurfaceBundle {
    load_surfaces(
        active.project_id.clone(),
        request_project_root(&active.catalog.root),
    )
    .await
}

async fn load_surfaces(
    project_id: ProjectId,
    project_root: Option<FlowProjectRoot>,
) -> SurfaceBundle {
    let caller = match caller_id(CallerKind::Gui) {
        Ok(caller) => caller,
        Err(error) => return unavailable_bundle(error.to_string(), None, None),
    };
    let credential = match load_credential(caller.clone()).await {
        Ok(credential) => credential,
        Err(detail) => return gui_registration_required_bundle(detail),
    };
    load_surfaces_with_credential(caller, credential, project_id, project_root).await
}

/// The project root the GUI sends alongside a project-scoped request, so the
/// daemon can remember a human-readable location for this project ID and the
/// activity feed can name it. Best-effort: a non-Unicode or non-canonical root
/// is simply left off.
fn request_project_root(root: &Path) -> Option<FlowProjectRoot> {
    FlowProjectRoot::new(root.to_str()?).ok()
}

async fn load_surfaces_with_credential(
    caller: CallerId,
    credential: CallerCredential,
    project_id: ProjectId,
    project_root: Option<FlowProjectRoot>,
) -> SurfaceBundle {
    let (health, current, access) =
        load_project_surfaces(caller, credential, project_id, project_root).await;
    SurfaceBundle {
        health,
        current,
        access,
    }
}

async fn load_health_access(project_id: ProjectId) -> (HealthState, AccessConfigState) {
    let caller = match caller_id(CallerKind::Gui) {
        Ok(caller) => caller,
        Err(error) => {
            return unavailable_health_access(error.to_string(), None);
        }
    };
    let credential = match load_credential(caller.clone()).await {
        Ok(credential) => credential,
        Err(detail) => {
            return unavailable_health_access(detail, Some(GUI_REGISTRATION_RECOVERY.to_owned()));
        }
    };
    load_project_health_access(caller, credential, project_id).await
}

async fn approval_surfaces_with<F, Fut>(current: CurrentState, load: F) -> SurfaceBundle
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = (HealthState, AccessConfigState)>,
{
    let (health, access) = load().await;
    SurfaceBundle {
        health,
        current,
        access,
    }
}

fn unavailable_health_access(
    detail: String,
    recovery: Option<String>,
) -> (HealthState, AccessConfigState) {
    (
        HealthState::Degraded {
            detail: detail.clone(),
            recovery: recovery.clone(),
        },
        AccessConfigState::Unavailable {
            code: None,
            detail,
            recovery,
        },
    )
}

fn gui_registration_required_bundle(detail: String) -> SurfaceBundle {
    unavailable_bundle(
        detail,
        Some(GUI_REGISTRATION_RECOVERY.to_owned()),
        Some(CurrentUnavailableCode::GuiRegistrationRequired),
    )
}

fn unavailable_bundle(
    detail: String,
    recovery: Option<String>,
    current_code: Option<CurrentUnavailableCode>,
) -> SurfaceBundle {
    SurfaceBundle {
        health: HealthState::Degraded {
            detail: detail.clone(),
            recovery: recovery.clone(),
        },
        current: CurrentState::Degraded {
            code: current_code,
            detail: detail.clone(),
            recovery: recovery.clone(),
        },
        access: AccessConfigState::Unavailable {
            code: None,
            detail,
            recovery,
        },
    }
}

/// The authorized scope of one fenced command: the constant daemon authority
/// or the active project generation.
#[derive(Clone)]
enum CommandScope {
    Daemon,
    Project(ActiveProject),
}

impl CommandScope {
    fn project_id(&self) -> ProjectId {
        match self {
            Self::Daemon => ProjectId::daemon_scope(),
            Self::Project(active) => active.project_id.clone(),
        }
    }

    /// The project root this scope reads: the daemon scope has none.
    fn project_root(&self) -> Option<PathBuf> {
        match self {
            Self::Daemon => None,
            Self::Project(active) => Some(active.catalog.root.clone()),
        }
    }
}

fn is_daemon_fence(fence: &CommandFence) -> bool {
    fence.project_handle.as_str() == DAEMON_AUTHORITY
        || fence.generation.as_str() == DAEMON_AUTHORITY
}

fn ensure_scope_matches(
    state: &DesktopState,
    scope: &CommandScope,
    fence: &CommandFence,
) -> DesktopResult<()> {
    match scope {
        // The daemon authority is constant: it never goes stale.
        CommandScope::Daemon => Ok(()),
        CommandScope::Project(active) => ensure_active_matches(state, active, fence),
    }
}

fn ensure_active_matches(
    state: &DesktopState,
    active: &ActiveProject,
    fence: &CommandFence,
) -> DesktopResult<()> {
    let Some(current) = state.active.as_ref() else {
        return Err(DesktopErrorDto::stale("No project is active."));
    };
    if current.catalog.handle != active.catalog.handle
        || current.catalog.handle != fence.project_handle
        || current.generation != active.generation
        || current.generation != fence.generation
    {
        return Err(DesktopErrorDto::stale(
            "The project changed while this operation was running.",
        ));
    }
    Ok(())
}

fn finish_snapshot_locked(
    state: &mut DesktopState,
    fence: CommandFence,
    mut active: ActiveProject,
    surfaces: SurfaceBundle,
) -> DesktopResult<SnapshotDto> {
    ensure_active_matches(state, &active, &fence)?;
    active.generation = GenerationId::new();
    state.active = Some(active.clone());
    state.used_operations.clear();
    state.approvals.clear();
    state.evidence.clear();
    state.flow_workspace = None;
    Ok(snapshot_from_surfaces(
        state,
        &active,
        fence.operation_id,
        surfaces,
    ))
}

fn snapshot_from_surfaces(
    state: &mut DesktopState,
    active: &ActiveProject,
    operation: OperationId,
    surfaces: SurfaceBundle,
) -> SnapshotDto {
    let project = project_dto(&active.catalog);
    let health = health_dto(surfaces.health);
    let current = current_dto(state, surfaces.current);
    let access = access_dto(surfaces.access);
    SnapshotDto {
        fence: CommandFence::new(
            active.catalog.handle.clone(),
            active.generation.clone(),
            operation,
        ),
        data: SnapshotDataDto {
            project,
            health,
            current,
            access,
            catalog_warning: state.catalog_warning.clone(),
        },
    }
}

fn project_dto(project: &CatalogProject) -> ProjectSummaryDto {
    ProjectSummaryDto {
        handle: project.handle.clone(),
        name: bounded_name(&project.name, &project.root),
        location: bounded_path(&project.root),
    }
}

fn health_dto(state: HealthState) -> HealthDto {
    match state {
        HealthState::Healthy {
            daemon_version,
            queue_depth,
        } => HealthDto::Healthy {
            daemon_version: bounded_detail(daemon_version),
            queue_depth,
        },
        HealthState::Offline => HealthDto::Offline,
        HealthState::Degraded { detail, recovery } => HealthDto::Degraded {
            detail: bounded_detail(detail),
            recovery: recovery.map(bounded_detail),
        },
    }
}

fn current_dto(state: &mut DesktopState, current: CurrentState) -> CurrentDto {
    match current {
        CurrentState::Available(view) => current_available_dto(state, view),
        CurrentState::ApprovalRequired(pending) => {
            let expires_at_ms = pending.challenge.expires_at_unix_ms;
            let handle = ApprovalHandle::new();
            state.approvals.insert(handle.clone(), pending);
            CurrentDto::ApprovalRequired {
                approval: handle,
                expires_at_ms,
            }
        }
        CurrentState::Blocked {
            code,
            detail,
            recovery,
        } => CurrentDto::Blocked {
            failure: failure_dto(&code, detail, recovery),
        },
        CurrentState::Degraded {
            code,
            detail,
            recovery,
        } => CurrentDto::Unavailable {
            failure: unavailable_failure(
                code.map(|code| code.as_str().to_owned()),
                detail,
                recovery,
            ),
        },
    }
}

fn current_available_dto(state: &mut DesktopState, view: CurrentView) -> CurrentDto {
    CurrentDto::Available {
        queued: view.queued.iter().map(request_dto).collect(),
        truncated: view.truncated,
        run: view.run.map(|run| run_dto(state, run)),
    }
}

fn request_dto(request: &ProjectRequestSummary) -> RequestSummaryDto {
    RequestSummaryDto {
        request_id: bounded_detail(request.request_id.as_str().to_owned()),
        operation_kind: bounded_detail(request.operation_kind().to_owned()),
        state: request_state_label(request.state).to_owned(),
        queue_sequence: request.queue_sequence,
        accepted_at_ms: request.accepted_at_ms,
        completed_at_ms: request.completed_at_ms,
    }
}

fn run_dto(state: &mut DesktopState, run: RunView) -> RunDto {
    RunDto {
        request: request_dto(&run.request),
        timeline: run
            .timeline
            .into_iter()
            .take(MAX_TIMELINE_FACTS)
            .map(|fact| timeline_dto(state, fact))
            .collect(),
        outcome: run.outcome.map(|outcome| outcome_dto(state, outcome)),
        detail_error: run.detail_error.map(bounded_detail),
    }
}

const fn request_state_label(state: ProjectRequestState) -> &'static str {
    match state {
        ProjectRequestState::Queued => "queued",
        ProjectRequestState::Leased => "leased",
        ProjectRequestState::CancellationRequested => "cancellation_requested",
        ProjectRequestState::Succeeded => "succeeded",
        ProjectRequestState::Failed => "failed",
        ProjectRequestState::Cancelled => "cancelled",
    }
}

fn timeline_dto(state: &mut DesktopState, fact: TimelineFact) -> TimelineFactDto {
    TimelineFactDto {
        kind: timeline_kind_dto(fact.kind),
        label: bounded_detail(fact.label),
        summary: bounded_detail(fact.summary),
        verified: fact.verified,
        evidence: fact
            .evidence
            .into_iter()
            .take(MAX_EVIDENCE_HANDLES)
            .map(|handle| register_evidence(state, handle))
            .collect(),
    }
}

const fn timeline_kind_dto(kind: TimelineKind) -> TimelineKindDto {
    match kind {
        TimelineKind::Request => TimelineKindDto::Request,
        TimelineKind::Evidence => TimelineKindDto::Evidence,
        TimelineKind::Change => TimelineKindDto::Change,
        TimelineKind::Verification => TimelineKindDto::Verification,
        TimelineKind::Failure => TimelineKindDto::Failure,
    }
}

fn outcome_dto(state: &mut DesktopState, outcome: OutcomeView) -> OutcomeDto {
    OutcomeDto {
        heading: outcome.heading.to_owned(),
        solved: outcome.solved,
        sections: outcome
            .sections
            .into_iter()
            .map(|section| OutcomeSectionDto {
                label: section.label.to_owned(),
                summary: bounded_detail(section.summary),
                satisfied: section.satisfied,
            })
            .collect(),
        evidence: outcome
            .evidence
            .into_iter()
            .take(MAX_EVIDENCE_HANDLES)
            .map(|handle| register_evidence(state, handle))
            .collect(),
        evidence_truncated: outcome.evidence_truncated,
    }
}

fn register_evidence(
    state: &mut DesktopState,
    protocol: ProtocolEvidenceHandle,
) -> EvidenceHandleDto {
    if let Some((handle, _)) = state
        .evidence
        .iter()
        .find(|(_, candidate)| candidate == &&protocol)
    {
        return handle.clone();
    }
    let handle = EvidenceHandleDto::new();
    state.evidence.insert(handle.clone(), protocol);
    handle
}

fn access_dto(state: AccessConfigState) -> AccessConfigDto {
    match state {
        AccessConfigState::Available(view) => available_access_dto(&view),
        AccessConfigState::Blocked {
            code,
            detail,
            recovery,
            approval_id,
            expires_at_ms,
        } => AccessConfigDto::Blocked {
            failure: failure_dto(&code, detail, recovery),
            approval_id: approval_id.map(bounded_detail),
            expires_at_ms,
        },
        AccessConfigState::Unavailable {
            code,
            detail,
            recovery,
        } => AccessConfigDto::Unavailable {
            failure: match code {
                Some(code) => failure_dto(&code, detail, recovery),
                None => unavailable_failure(None, detail, recovery),
            },
        },
    }
}

fn available_access_dto(view: &AccessConfigView) -> AccessConfigDto {
    AccessConfigDto::Available {
        truth: format!("{:?}", view.truth).to_ascii_lowercase(),
        platform_roots_enabled: view.platform_roots_enabled,
        system_proxy_discovery_enabled: view.system_proxy_discovery_enabled,
        proxy_environment: view.proxy_environment.to_owned(),
        no_proxy: view.no_proxy.to_owned(),
        pac: view.pac.to_owned(),
    }
}

async fn observatory_credential<T>() -> Result<(CallerId, CallerCredential), ObservatoryState<T>> {
    let caller = caller_id(CallerKind::Gui).map_err(|error| ObservatoryState::Unavailable {
        code: None,
        detail: error.to_string(),
        recovery: None,
    })?;
    let credential =
        load_credential(caller.clone())
            .await
            .map_err(|detail| ObservatoryState::Unavailable {
                code: Some(
                    CurrentUnavailableCode::GuiRegistrationRequired
                        .as_str()
                        .to_owned(),
                ),
                detail,
                recovery: Some(GUI_REGISTRATION_RECOVERY.to_owned()),
            })?;
    Ok((caller, credential))
}

fn activity_dto(state: ObservatoryState<ActivityResult>) -> ActivityDto {
    match state {
        ObservatoryState::Available(result) => ActivityDto::Ok {
            events: result.events.into_iter().map(activity_event_dto).collect(),
            truncated: result.truncated,
        },
        ObservatoryState::Blocked {
            code,
            detail,
            recovery,
        } => ActivityDto::Blocked {
            failure: failure_dto(&code, detail, recovery),
        },
        ObservatoryState::Unavailable {
            code,
            detail,
            recovery,
        } => ActivityDto::Unavailable {
            failure: unavailable_failure(code, detail, recovery),
        },
    }
}

fn activity_event_dto(event: ActivityEventSummary) -> ActivityEventDto {
    ActivityEventDto {
        sequence: event.sequence,
        project_id: bounded_detail(event.project_id.as_str().to_owned()),
        caller_id: bounded_detail(event.caller_id.as_str().to_owned()),
        action: bounded_detail(event.action),
        decision: bounded_detail(event.decision),
        outcome: bounded_detail(event.outcome),
        occurred_at_ms: event.occurred_at_ms,
        project_root: event
            .project_root
            .map(|root| bounded_utf8(root, MAX_PROJECT_PATH_BYTES)),
    }
}

fn daemon_logs_dto(state: ObservatoryState<DaemonLogsResult>) -> DaemonLogsDto {
    match state {
        ObservatoryState::Available(result) => DaemonLogsDto::Ok {
            entries: result
                .entries
                .into_iter()
                .map(|entry| DaemonLogEntryDto {
                    timestamp_ms: entry.timestamp_ms,
                    severity: match entry.severity {
                        LogSeverity::Info => "info".to_owned(),
                        LogSeverity::Warn => "warn".to_owned(),
                        LogSeverity::Error => "error".to_owned(),
                    },
                    message: bounded_detail(entry.message),
                })
                .collect(),
        },
        ObservatoryState::Blocked {
            code,
            detail,
            recovery,
        } => DaemonLogsDto::Blocked {
            failure: failure_dto(&code, detail, recovery),
        },
        ObservatoryState::Unavailable {
            code,
            detail,
            recovery,
        } => DaemonLogsDto::Unavailable {
            failure: unavailable_failure(code, detail, recovery),
        },
    }
}

fn daemon_stats_dto(state: ObservatoryState<DaemonStatsResult>) -> DaemonStatsDto {
    match state {
        ObservatoryState::Available(result) => DaemonStatsDto::Ok {
            days: result.days.into_iter().map(activity_day_dto).collect(),
            projects: result.projects.into_iter().map(project_usage_dto).collect(),
        },
        ObservatoryState::Blocked {
            code,
            detail,
            recovery,
        } => DaemonStatsDto::Blocked {
            failure: failure_dto(&code, detail, recovery),
        },
        ObservatoryState::Unavailable {
            code,
            detail,
            recovery,
        } => DaemonStatsDto::Unavailable {
            failure: unavailable_failure(code, detail, recovery),
        },
    }
}

const fn activity_day_dto(day: ActivityDaySummary) -> ActivityDayDto {
    ActivityDayDto {
        day_start_ms: day.day_start_ms,
        events: day.events,
    }
}

fn project_usage_dto(project: ProjectUsageSummary) -> ProjectUsageDto {
    ProjectUsageDto {
        project_id: project.project_id,
        events: project.events,
        last_event_ms: project.last_event_ms,
        root: project
            .root
            .map(|root| bounded_utf8(root, MAX_PROJECT_PATH_BYTES)),
    }
}

fn callers_dto(state: ObservatoryState<CallerListResult>) -> CallersDto {
    match state {
        ObservatoryState::Available(result) => CallersDto::Ok {
            callers: result.callers.iter().map(caller_dto).collect(),
        },
        ObservatoryState::Blocked {
            code,
            detail,
            recovery,
        } => CallersDto::Blocked {
            failure: failure_dto(&code, detail, recovery),
        },
        ObservatoryState::Unavailable {
            code,
            detail,
            recovery,
        } => CallersDto::Unavailable {
            failure: unavailable_failure(code, detail, recovery),
        },
    }
}

fn caller_dto(caller: &CallerSummary) -> CallerDto {
    CallerDto {
        caller_id: bounded_detail(caller.caller_id.as_str().to_owned()),
        registered_at_ms: caller.registered_at_ms,
        revoked_at_ms: caller.revoked_at_ms,
        kind: caller.kind.clone(),
    }
}

/// A loading daemon cannot say so: it binds its socket, then blocks on
/// hashing and mapping the artifact before it ever accepts a request. So an
/// unreachable model surface plus a still-running child this GUI spawned
/// means "loading", not "gone" — that live handle is the only honest signal
/// available while the load runs (#34).
fn mark_model_loading(status: ModelStatusDto, child_slot: &mut Option<Child>) -> ModelStatusDto {
    match status {
        ModelStatusDto::Ok {
            loaded: None,
            registered,
            load_failure: None,
            ..
        } => ModelStatusDto::Ok {
            loaded: None,
            registered,
            load_failure: None,
            loading: daemon_child_running(child_slot),
        },
        other => other,
    }
}

/// Whether the daemon child this GUI spawned is still running. `try_wait`
/// collects it when it has exited, so a stale handle never reports live.
fn daemon_child_running(child_slot: &mut Option<Child>) -> bool {
    child_slot
        .as_mut()
        .is_some_and(|child| matches!(child.try_wait(), Ok(None)))
}

fn model_status_dto(state: ObservatoryState<ModelStatusResult>) -> ModelStatusDto {
    match state {
        ObservatoryState::Available(result) => ModelStatusDto::Ok {
            loaded: result.loaded.as_ref().map(model_summary_dto),
            registered: result.registered.iter().map(model_summary_dto).collect(),
            load_failure: result.load_failure.map(bounded_detail),
            // A daemon that answered is past its load, whatever it reports.
            loading: false,
        },
        ObservatoryState::Blocked {
            code,
            detail,
            recovery,
        } => ModelStatusDto::Blocked {
            failure: failure_dto(&code, detail, recovery),
        },
        ObservatoryState::Unavailable {
            code,
            detail,
            recovery,
        } => ModelStatusDto::Unavailable {
            failure: unavailable_failure(code, detail, recovery),
        },
    }
}

fn model_preset_dto(
    preset: &model_presets::ModelPreset,
    host_total_bytes: Option<u64>,
) -> ModelPresetDto {
    ModelPresetDto {
        id: preset.id.to_owned(),
        label: preset.label.to_owned(),
        model: preset.model.to_owned(),
        file_name: preset.file_name.to_owned(),
        url: preset.url.to_owned(),
        expected_size_bytes: preset.expected_size_bytes,
        sha256: preset.sha256.to_owned(),
        license_id: preset.license_id.to_owned(),
        license_url: preset.license_url.to_owned(),
        license_notice_text: preset.license_notice_text.to_owned(),
        calibrated: preset.calibrated(),
        fits_host: host_total_bytes.is_none_or(|total| preset.fits_host(total)),
        params_label: preset.params_label.to_owned(),
        quant_label: preset.quant_label.to_owned(),
    }
}

fn model_summary_dto(summary: &ModelSummary) -> ModelSummaryDto {
    ModelSummaryDto {
        model_id: bounded_detail(summary.model_id().to_owned()),
        size_bytes: summary.size_bytes,
    }
}

/// Reads the durable registered-model catalog straight from the store, for
/// when the daemon is not serving: nothing can be confirmed loaded without a
/// live daemon read, but the registered models stay reachable.
async fn registered_model_catalog() -> Option<Vec<ModelSummaryDto>> {
    let state_path = user_data_dir().ok()?.join("state.sqlite3");
    registered_model_catalog_in(state_path).await
}

async fn registered_model_catalog_in(state_path: PathBuf) -> Option<Vec<ModelSummaryDto>> {
    let store = Store::open(state_path).ok()?;
    let catalog = store.list_models().await.ok()?;
    let shutdown = store.shutdown().await;
    let summaries = catalog
        .iter()
        .map(|model| ModelSummary::new(model.key.id(), model.size_bytes))
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    shutdown.ok()?;
    Some(summaries.iter().map(model_summary_dto).collect())
}

fn model_infer_dto(state: ObservatoryState<ModelGenerationResult>) -> ModelInferDto {
    match state {
        ObservatoryState::Available(result) => ModelInferDto::Ok {
            model: bounded_detail(result.model.clone()),
            // The protocol already bounds generation text at 512 KiB; the
            // 4 KiB detail bound would destroy legitimate output.
            text: result.text().to_owned(),
            finish_reason: finish_reason_label(result.finish_reason).to_owned(),
            usage: ModelUsageDto {
                input_tokens: result.usage.input_tokens,
                sampled_output_tokens: result.usage.sampled_output_tokens,
                emitted_output_tokens: result.usage.emitted_output_tokens,
            },
        },
        ObservatoryState::Blocked {
            code,
            detail,
            recovery,
        } => ModelInferDto::Blocked {
            failure: failure_dto(&code, detail, recovery),
        },
        ObservatoryState::Unavailable {
            code,
            detail,
            recovery,
        } => ModelInferDto::Unavailable {
            failure: unavailable_failure(code, detail, recovery),
        },
    }
}

const fn finish_reason_label(reason: ModelFinishReason) -> &'static str {
    match reason {
        ModelFinishReason::Stop => "stop",
        ModelFinishReason::Length => "length",
    }
}

fn connectors_dto(state: ObservatoryState<ConnectorListResult>) -> ConnectorsDto {
    match state {
        ObservatoryState::Available(result) => ConnectorsDto::Ok {
            connectors: result
                .connectors
                .into_iter()
                .map(connector_summary_dto)
                .collect(),
        },
        ObservatoryState::Blocked {
            code,
            detail,
            recovery,
        } => ConnectorsDto::Blocked {
            failure: failure_dto(&code, detail, recovery),
        },
        ObservatoryState::Unavailable {
            code,
            detail,
            recovery,
        } => ConnectorsDto::Unavailable {
            failure: unavailable_failure(code, detail, recovery),
        },
    }
}

fn connector_configure_dto(
    state: ObservatoryState<ConnectorConfigureResult>,
) -> ConnectorConfigureDto {
    match state {
        ObservatoryState::Available(result) => ConnectorConfigureDto::Ok {
            connector: connector_summary_dto(result.connector),
        },
        ObservatoryState::Blocked {
            code,
            detail,
            recovery,
        } => ConnectorConfigureDto::Blocked {
            failure: failure_dto(&code, detail, recovery),
        },
        ObservatoryState::Unavailable {
            code,
            detail,
            recovery,
        } => ConnectorConfigureDto::Unavailable {
            failure: unavailable_failure(code, detail, recovery),
        },
    }
}

fn connector_test_dto(state: ObservatoryState<ConnectorTestResult>) -> ConnectorTestDto {
    match state {
        ObservatoryState::Available(result) => ConnectorTestDto::Ok {
            connector_id: bounded_detail(result.connector_id),
            result: connector_test_disposition_label(result.status).to_owned(),
            detail: bounded_detail(result.detail),
        },
        ObservatoryState::Blocked {
            code,
            detail,
            recovery,
        } => ConnectorTestDto::Blocked {
            failure: failure_dto(&code, detail, recovery),
        },
        ObservatoryState::Unavailable {
            code,
            detail,
            recovery,
        } => ConnectorTestDto::Unavailable {
            failure: unavailable_failure(code, detail, recovery),
        },
    }
}

fn connector_summary_dto(summary: ConnectorSummary) -> ConnectorSummaryDto {
    ConnectorSummaryDto {
        connector_id: bounded_detail(summary.connector_id),
        enabled: summary.enabled,
        base_url: summary.base_url.map(bounded_detail),
        credential_present: summary.credential_present,
        last_test_status: summary.last_test_status.map(bounded_detail),
        last_test_at_ms: summary.last_test_at_ms,
    }
}

const fn connector_test_disposition_label(status: ConnectorTestDisposition) -> &'static str {
    match status {
        ConnectorTestDisposition::Passed => "passed",
        ConnectorTestDisposition::Failed => "failed",
    }
}

const fn clamp_model_output_tokens(requested: Option<u32>) -> u32 {
    match requested {
        None | Some(0) => MODEL_INFER_DEFAULT_OUTPUT_TOKENS,
        Some(tokens) if tokens > MAX_MODEL_OUTPUT_TOKENS => MAX_MODEL_OUTPUT_TOKENS,
        Some(tokens) => tokens,
    }
}

fn flow_graph_data(source: &str) -> FlowGraphDto {
    // FlowDefinition::parse_toml enforces MAX_FLOW_DOCUMENT_BYTES on the way in.
    match FlowDefinition::parse_toml(source) {
        Ok(definition) => FlowGraphDto::Ok {
            definition: definition_json(&definition),
        },
        Err(error) => FlowGraphDto::Invalid {
            failure: flow_conversion_failure(error.to_string()),
        },
    }
}

fn flow_compose_data(definition: &str) -> FlowComposeDto {
    if definition.len() > MAX_FLOW_DOCUMENT_BYTES {
        return FlowComposeDto::Invalid {
            failure: flow_conversion_failure(format!(
                "The flow definition exceeds the {MAX_FLOW_DOCUMENT_BYTES}-byte limit."
            )),
        };
    }
    let parsed = match serde_json::from_str::<FlowDefinition>(definition) {
        Ok(parsed) => parsed,
        Err(error) => {
            return FlowComposeDto::Invalid {
                failure: flow_conversion_failure(error.to_string()),
            };
        }
    };
    // to_normalized_toml re-validates the complete definition first.
    let source = match parsed.to_normalized_toml() {
        Ok(source) => source,
        Err(error) => {
            return FlowComposeDto::Invalid {
                failure: flow_conversion_failure(error.to_string()),
            };
        }
    };
    if source.len() > MAX_FLOW_DOCUMENT_BYTES {
        return FlowComposeDto::Invalid {
            failure: flow_conversion_failure(format!(
                "The normalized flow document exceeds the {MAX_FLOW_DOCUMENT_BYTES}-byte limit."
            )),
        };
    }
    FlowComposeDto::Ok { source }
}

fn flow_conversion_failure(detail: String) -> FailureDto {
    unavailable_failure(
        None,
        detail,
        Some("Fix the flow definition and retry the conversion.".to_owned()),
    )
}

/// Builds the exact serde JSON layout of [`FlowDefinition`], with defaults
/// materialized, so the value deserializes back into an equal definition.
fn definition_json(definition: &FlowDefinition) -> serde_json::Value {
    let outcome = definition.outcome();
    serde_json::json!({
        "schema_version": definition.schema_version(),
        "id": definition.id(),
        "name": definition.name(),
        "description": definition.description(),
        "revision": definition.revision(),
        "steps": definition
            .steps()
            .iter()
            .map(|step| step_json(definition.schema_version(), step))
            .collect::<Vec<_>>(),
        "outcome": {
            "solved": outcome.solved(),
            "changed": outcome.changed(),
            "verified": outcome.verified(),
            "unresolved": outcome.unresolved(),
            "blocked": outcome.blocked(),
        },
    })
}

fn step_json(schema_version: u16, step: &FlowStep) -> serde_json::Value {
    let retry = step.retry();
    let mut value = serde_json::json!({
        "id": step.id(),
        "description": step.description(),
        "depends_on": step.dependencies(),
        "condition": condition_json(step.condition()),
        "retry": {
            "max_attempts": retry.max_attempts(),
            "initial_backoff_ms": retry.initial_backoff_ms(),
            "max_backoff_ms": retry.max_backoff_ms(),
        },
        "approval": approval_label(step.approval()),
        "timeout_seconds": step.timeout_seconds(),
        "effect": effect_label(step.effect()),
        "action": action_json(step.action()),
    });
    if let Some(key) = step.idempotency_key() {
        value["idempotency_key"] = serde_json::Value::String(key.to_owned());
    }
    // Schema version 1 derives semantics and rejects an explicit field;
    // version 2 always carries one.
    if schema_version >= 2 {
        value["semantic"] =
            serde_json::Value::String(semantic_role_label(step.semantic_role()).to_owned());
    }
    value
}

fn condition_json(condition: &StepCondition) -> serde_json::Value {
    match condition {
        StepCondition::Always => serde_json::json!({ "kind": "always" }),
        StepCondition::Succeeded { step } => {
            serde_json::json!({ "kind": "succeeded", "step": step })
        }
        StepCondition::Failed { step } => serde_json::json!({ "kind": "failed", "step": step }),
    }
}

fn action_json(action: &StepAction) -> serde_json::Value {
    match action {
        StepAction::Command {
            program,
            args,
            working_directory,
        } => serde_json::json!({
            "type": "command",
            "program": program,
            "args": args,
            "working_directory": working_directory,
        }),
        StepAction::Connector {
            connector,
            capability,
            resource,
        } => serde_json::json!({
            "type": "connector",
            "connector": connector,
            "capability": capability,
            "resource": { "kind": resource.kind(), "id": resource.id() },
        }),
    }
}

fn failure_dto(code: &FailureCode, detail: String, recovery: Option<String>) -> FailureDto {
    FailureDto {
        kind: failure_kind(code),
        code: Some(failure_code(code).to_owned()),
        detail: bounded_detail(detail),
        recovery: recovery.map(bounded_detail),
    }
}

fn unavailable_failure(
    code: Option<String>,
    detail: impl Into<String>,
    recovery: Option<String>,
) -> FailureDto {
    FailureDto {
        kind: FailureKindDto::Unavailable,
        code,
        detail: bounded_detail(detail.into()),
        recovery: recovery.map(bounded_detail),
    }
}

fn settings_data_dir() -> DesktopResult<PathBuf> {
    user_data_dir().map_err(|error| {
        DesktopErrorDto::unavailable(
            "PAM could not resolve its local data directory.",
            Some(error.to_string()),
        )
    })
}

/// The durable state path and the GUI caller identity the daemon-scope grant
/// commands write as.
fn daemon_access_identity() -> DesktopResult<(PathBuf, CallerId)> {
    let state_path = settings_data_dir()?.join("state.sqlite3");
    let caller = caller_id(CallerKind::Gui)
        .map_err(|error| DesktopErrorDto::unavailable(error.to_string(), None))?;
    Ok((state_path, caller))
}

fn settings_snapshot() -> DesktopResult<settings::AppSettingsSnapshot> {
    let data_dir = settings_data_dir()?;
    let home = settings::resolve_home()
        .map_err(|failure| DesktopErrorDto::unavailable(failure.detail, failure.recovery))?;
    Ok(settings::snapshot(&data_dir, &home))
}

fn app_settings_dto(snapshot: &settings::AppSettingsSnapshot) -> AppSettingsDto {
    AppSettingsDto {
        models_dir: snapshot.models_dir.to_string_lossy().into_owned(),
        models_dir_is_default: snapshot.models_dir_is_default,
        data_dir: snapshot.data_dir.to_string_lossy().into_owned(),
        flows_dir: snapshot.flows_dir.to_string_lossy().into_owned(),
        logs_dir: snapshot.logs_dir.to_string_lossy().into_owned(),
        logs_size_bytes: snapshot.logs_size_bytes,
    }
}

const fn failure_kind(code: &FailureCode) -> FailureKindDto {
    match code {
        FailureCode::Forbidden | FailureCode::ApprovalRequired => FailureKindDto::Blocked,
        _ => FailureKindDto::Unavailable,
    }
}

const fn failure_code(code: &FailureCode) -> &'static str {
    match code {
        FailureCode::Unauthenticated => "unauthenticated",
        FailureCode::Forbidden => "forbidden",
        FailureCode::ApprovalRequired => "approval_required",
        FailureCode::ApprovalDenied => "approval_denied",
        FailureCode::ApprovalExpired => "approval_expired",
        FailureCode::UnsupportedProtocolVersion => "unsupported_protocol_version",
        FailureCode::InvalidRequest => "invalid_request",
        FailureCode::FrameTooLarge => "frame_too_large",
        FailureCode::NotFound => "not_found",
        FailureCode::Pending => "pending",
        FailureCode::IdempotencyConflict => "idempotency_conflict",
        FailureCode::Cancelled => "cancelled",
        FailureCode::LeaseConflict => "lease_conflict",
        FailureCode::Busy => "busy",
        FailureCode::Internal => "internal",
    }
}

fn evidence_data(
    handle: EvidenceHandleDto,
    state: EvidenceState,
) -> DesktopResult<EvidenceDataDto> {
    match state {
        EvidenceState::Available(preview) => Ok(evidence_preview_dto(handle, preview)),
        EvidenceState::Failed { detail, .. } => Err(DesktopErrorDto::unavailable(detail, None)),
    }
}

fn evidence_preview_dto(handle: EvidenceHandleDto, preview: EvidencePreview) -> EvidenceDataDto {
    EvidenceDataDto {
        handle,
        digest: bounded_detail(preview.digest),
        size_bytes: preview.size_bytes,
        media_type: bounded_detail(preview.media_type),
        body: preview.body.map(bounded_detail),
        truncated: preview.truncated,
        truth: format!("{:?}", preview.truth).to_ascii_lowercase(),
    }
}

fn workspace_mut(state: &mut DesktopState) -> DesktopResult<&mut FlowWorkspaceState> {
    state.flow_workspace.as_mut().ok_or_else(|| {
        DesktopErrorDto::stale("Load the flow workspace before using a flow handle.")
    })
}

fn flow_library_unavailable() -> DesktopErrorDto {
    DesktopErrorDto::unavailable(
        "PAM could not resolve the global flow-definition library.",
        Some("Verify the operating system user data directory, then retry Flows.".to_owned()),
    )
}

/// Copies definitions from a legacy project-local `.pam/flows` catalog into
/// the global flow library, skipping any definition ID the library already
/// has. Migration is best-effort: an unreadable or missing legacy catalog is
/// silently treated as nothing to migrate, since it is not this project's
/// fault that the global library load failed. Legacy files are never
/// modified or removed.
fn migrate_legacy_flows(
    global: &mut FlowEditorModel,
    legacy_root: &Path,
) -> Result<Vec<String>, FlowEditorError> {
    let Ok(legacy) = FlowEditorModel::open(legacy_root) else {
        return Ok(Vec::new());
    };
    let existing_ids: HashSet<&str> = global
        .entries()
        .iter()
        .map(|entry| entry.identity().id())
        .collect();
    let sources: Vec<String> = legacy
        .entries()
        .iter()
        .filter(|entry| !existing_ids.contains(entry.identity().id()))
        .map(|entry| entry.source().to_owned())
        .collect();
    drop(existing_ids);
    let mut migrated = Vec::with_capacity(sources.len());
    for source in sources {
        let document = global.new_document(source)?;
        let interaction = document.prepare_save()?;
        let mut document = document;
        let saved = document.commit_save(interaction)?;
        migrated.push(saved.identity().id().to_owned());
    }
    if !migrated.is_empty() {
        global.reload()?;
    }
    Ok(migrated)
}

fn identity_dto(identity: &FlowIdentity) -> FlowIdentityDto {
    FlowIdentityDto {
        file_name: identity.file_name().to_owned(),
        id: identity.id().to_owned(),
        revision: identity.revision(),
        digest: identity.digest().to_string(),
    }
}

fn dry_run_dto(plan: &FlowDryRunPlan) -> FlowDryRunDto {
    FlowDryRunDto {
        daemon_definition_eligible: plan.daemon_definition_eligible(),
        steps: plan
            .steps()
            .iter()
            .map(|step| {
                let retry = step.retry();
                FlowDryRunStepDto {
                    index: step.index(),
                    id: step.id().to_owned(),
                    semantic_role: semantic_role_label(step.semantic_role()).to_owned(),
                    condition: condition_label(step.condition()),
                    approval: approval_label(step.approval()).to_owned(),
                    effect: effect_label(step.effect()).to_owned(),
                    max_attempts: retry.max_attempts(),
                    initial_backoff_ms: retry.initial_backoff_ms(),
                    max_backoff_ms: retry.max_backoff_ms(),
                    action: action_label(step.action()),
                    daemon_authority: daemon_authority_label(step.daemon_authority()),
                }
            })
            .collect(),
    }
}

fn condition_label(condition: &DryRunCondition) -> String {
    match condition {
        DryRunCondition::Always => "always".to_owned(),
        DryRunCondition::Succeeded { step_id } => format!("succeeded:{step_id}"),
        DryRunCondition::Failed { step_id } => format!("failed:{step_id}"),
    }
}

const fn semantic_role_label(role: pam_flow::StepSemanticRole) -> &'static str {
    match role {
        pam_flow::StepSemanticRole::Observe => "observe",
        pam_flow::StepSemanticRole::Verify => "verify",
        pam_flow::StepSemanticRole::Change => "change",
    }
}

const fn approval_label(approval: pam_flow::ApprovalMode) -> &'static str {
    match approval {
        pam_flow::ApprovalMode::None => "none",
        pam_flow::ApprovalMode::Required => "required",
    }
}

const fn effect_label(effect: pam_flow::EffectKind) -> &'static str {
    match effect {
        pam_flow::EffectKind::ReadOnly => "read_only",
        pam_flow::EffectKind::Stateful => "stateful",
    }
}

fn action_label(action: &ActionAuthority) -> String {
    match action {
        ActionAuthority::Command {
            program,
            arguments,
            working_directory,
        } => bounded_detail(format!(
            "command:{program} {} @ {working_directory}",
            arguments.join(" ")
        )),
        ActionAuthority::Connector {
            connector,
            capability,
            resource_kind,
            resource_id,
        } => bounded_detail(format!(
            "connector:{connector}.{capability} {resource_kind}:{resource_id}"
        )),
    }
}

fn daemon_authority_label(authority: DaemonAuthority) -> String {
    match authority {
        DaemonAuthority::EligibleAfterRuntimeChecks => "eligible_after_runtime_checks".to_owned(),
        DaemonAuthority::Unsupported(reason) => format!(
            "unsupported:{}",
            match reason {
                crate::flow_editor::UnsupportedDaemonAuthority::Connector => "connector",
                crate::flow_editor::UnsupportedDaemonAuthority::StatefulEffect => {
                    "stateful_effect"
                }
                crate::flow_editor::UnsupportedDaemonAuthority::Approval => "approval",
                crate::flow_editor::UnsupportedDaemonAuthority::Program => "program",
                crate::flow_editor::UnsupportedDaemonAuthority::WorkingDirectory => {
                    "working_directory"
                }
                crate::flow_editor::UnsupportedDaemonAuthority::GitArguments => "git_arguments",
                crate::flow_editor::UnsupportedDaemonAuthority::SemanticRole => "semantic_role",
            }
        ),
    }
}

fn diff_dto(diff: &FlowVersionDiff) -> FlowVersionDiffDto {
    FlowVersionDiffDto {
        changed: diff.changed(),
        truncated: diff.truncated(),
        lines: diff
            .lines()
            .iter()
            .map(|line| FlowVersionDiffLineDto {
                kind: match line.kind() {
                    FlowVersionDiffLineKind::Context => "context",
                    FlowVersionDiffLineKind::Removed => "removed",
                    FlowVersionDiffLineKind::Added => "added",
                }
                .to_owned(),
                text: line.text().to_owned(),
            })
            .collect(),
    }
}

fn flow_error(error: &FlowEditorError) -> DesktopErrorDto {
    let kind = match error {
        FlowEditorError::NotFound(_) => DesktopErrorKind::NotFound,
        FlowEditorError::StaleSaveInteraction | FlowEditorError::SaveConflict => {
            DesktopErrorKind::Conflict
        }
        FlowEditorError::InvalidSelector
        | FlowEditorError::DocumentTooLarge { .. }
        | FlowEditorError::NormalizedDocumentTooLarge { .. }
        | FlowEditorError::InvalidToml(_)
        | FlowEditorError::InvalidDefinition { .. }
        | FlowEditorError::IdentityChanged { .. }
        | FlowEditorError::RevisionNotAdvanced { .. } => DesktopErrorKind::InvalidInput,
        _ => DesktopErrorKind::Unavailable,
    };
    DesktopErrorDto::new(kind, error.to_string(), None)
}

fn post_save_reload_error(error: &FlowEditorError) -> DesktopErrorDto {
    DesktopErrorDto::new(
        DesktopErrorKind::Unavailable,
        format!("The flow was saved, but PAM could not refresh the flow workspace: {error}"),
        Some("Reload the flow workspace before opening or saving another definition.".to_owned()),
    )
}

fn bounded_detail(value: String) -> String {
    bounded_utf8(value, MAX_DETAILS_BYTES)
}

fn bounded_path(path: &Path) -> String {
    bounded_utf8(path.to_string_lossy().into_owned(), MAX_PROJECT_PATH_BYTES)
}

fn bounded_utf8(mut value: String, maximum: usize) -> String {
    if value.len() <= maximum {
        return value;
    }
    let mut end = maximum.saturating_sub(3);
    while !value.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    value.truncate(end);
    value.push_str("...");
    value
}

/// Collects the spawned daemon child after a stop request. The graceful
/// shutdown gets a bounded grace period; whatever is still running after that
/// is killed, and the child is always waited on — dropping the handle without
/// a wait would leave a zombie once the process exits, and an untracked live
/// daemon would block a later start on the ownership lock.
/// Whether a stop whose protocol request failed still counts as a stop.
///
/// It does exactly when this process owned the daemon's child, because that
/// child has just been reaped: the daemon really is gone, and reporting an
/// error would leave the owner staring at a Stop button that never works. A
/// daemon this process did not spawn is not ours to kill, so there the failed
/// request is the answer.
fn stop_outcome(requested: Result<(), String>, owned_child: bool) -> DesktopResult<()> {
    match requested {
        Ok(()) => Ok(()),
        Err(_) if owned_child => Ok(()),
        Err(detail) => Err(DesktopErrorDto::unavailable(detail, None)),
    }
}

fn reap_daemon_child(child_slot: &mut Option<Child>) {
    let Some(mut child) = child_slot.take() else {
        return;
    };
    let deadline = std::time::Instant::now() + DAEMON_STOP_GRACE;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(DAEMON_STOP_POLL);
            }
            Ok(None) | Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return;
            }
        }
    }
}

/// Replaces the tracked daemon child, always collecting the previous one: an
/// already-exited child is reaped and a still-running one is killed first, so
/// a replaced daemon never outlives its handle as a zombie.
fn replace_daemon_child(child_slot: &mut Option<Child>, child: Child) {
    if let Some(mut previous) = child_slot.replace(child) {
        if !matches!(previous.try_wait(), Ok(Some(_))) {
            let _ = previous.kill();
        }
        let _ = previous.wait();
    }
}

#[cfg(test)]
pub(crate) fn failure_kind_for_test(code: &FailureCode) -> FailureKindDto {
    failure_kind(code)
}

#[cfg(test)]
pub(crate) fn bounded_detail_for_test(value: String) -> String {
    bounded_detail(value)
}

#[cfg(test)]
pub(crate) fn post_save_reload_error_for_test(error: &FlowEditorError) -> DesktopErrorDto {
    post_save_reload_error(error)
}

#[cfg(test)]
pub(crate) fn current_dto_for_test(current: CurrentState) -> CurrentDto {
    let mut state = test_state();
    current_dto(&mut state, current)
}

#[cfg(test)]
pub(crate) fn access_dto_for_test(access: AccessConfigState) -> AccessConfigDto {
    access_dto(access)
}

#[cfg(test)]
pub(crate) fn activity_dto_for_test(state: ObservatoryState<ActivityResult>) -> ActivityDto {
    activity_dto(state)
}

#[cfg(test)]
pub(crate) fn callers_dto_for_test(state: ObservatoryState<CallerListResult>) -> CallersDto {
    callers_dto(state)
}

#[cfg(test)]
pub(crate) fn model_status_dto_for_test(
    state: ObservatoryState<ModelStatusResult>,
) -> ModelStatusDto {
    model_status_dto(state)
}

#[cfg(test)]
pub(crate) async fn registered_model_catalog_in_for_test(
    state_path: PathBuf,
) -> Option<Vec<ModelSummaryDto>> {
    registered_model_catalog_in(state_path).await
}

#[cfg(test)]
pub(crate) async fn wait_for_daemon_serving_for_test<H, HF, M, MF>(
    child: &mut Child,
    model: Option<&str>,
    budget: Duration,
    health: H,
    model_loaded: M,
    progress: &StartupProgressCell,
    model_bytes: Option<u64>,
) -> DesktopResult<DaemonStartup>
where
    H: FnMut() -> HF,
    HF: Future<Output = HealthState>,
    M: FnMut() -> MF,
    MF: Future<Output = bool>,
{
    wait_for_daemon_serving(
        child,
        model,
        budget,
        health,
        model_loaded,
        progress,
        model_bytes,
    )
    .await
}

#[cfg(test)]
pub(crate) fn mark_model_loading_for_test(
    status: ModelStatusDto,
    child_slot: &mut Option<Child>,
) -> ModelStatusDto {
    mark_model_loading(status, child_slot)
}

#[cfg(test)]
pub(crate) fn startup_budget_for_bytes_for_test(model_bytes: Option<u64>) -> Duration {
    startup_budget_for_bytes(model_bytes)
}

#[cfg(test)]
pub(crate) fn model_infer_dto_for_test(
    state: ObservatoryState<ModelGenerationResult>,
) -> ModelInferDto {
    model_infer_dto(state)
}

#[cfg(test)]
pub(crate) fn stop_outcome_for_test(
    requested: Result<(), String>,
    owned_child: bool,
) -> DesktopResult<()> {
    stop_outcome(requested, owned_child)
}

#[cfg(test)]
pub(crate) fn reap_daemon_child_for_test(child_slot: &mut Option<Child>) {
    reap_daemon_child(child_slot);
}

#[cfg(test)]
pub(crate) fn replace_daemon_child_for_test(child_slot: &mut Option<Child>, child: Child) {
    replace_daemon_child(child_slot, child);
}

#[cfg(test)]
pub(crate) fn connectors_dto_for_test(
    state: ObservatoryState<ConnectorListResult>,
) -> ConnectorsDto {
    connectors_dto(state)
}

#[cfg(test)]
pub(crate) fn connector_configure_dto_for_test(
    state: ObservatoryState<ConnectorConfigureResult>,
) -> ConnectorConfigureDto {
    connector_configure_dto(state)
}

#[cfg(test)]
pub(crate) fn connector_test_dto_for_test(
    state: ObservatoryState<ConnectorTestResult>,
) -> ConnectorTestDto {
    connector_test_dto(state)
}

#[cfg(test)]
pub(crate) const fn clamp_model_output_tokens_for_test(requested: Option<u32>) -> u32 {
    clamp_model_output_tokens(requested)
}

#[cfg(test)]
pub(crate) fn flow_graph_data_for_test(source: &str) -> FlowGraphDto {
    flow_graph_data(source)
}

#[cfg(test)]
pub(crate) fn flow_compose_data_for_test(definition: &str) -> FlowComposeDto {
    flow_compose_data(definition)
}

#[cfg(test)]
pub(crate) fn evidence_dto_for_test(
    handle: EvidenceHandleDto,
    preview: EvidencePreview,
) -> EvidenceDataDto {
    evidence_preview_dto(handle, preview)
}

#[cfg(test)]
pub(crate) async fn approval_current_for_test(
    current: CurrentState,
    calls: &std::sync::atomic::AtomicUsize,
) -> CurrentState {
    approval_surfaces_with(current, || async {
        calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        (
            HealthState::Offline,
            AccessConfigState::Unavailable {
                code: None,
                detail: "test access".to_owned(),
                recovery: None,
            },
        )
    })
    .await
    .current
}

#[cfg(test)]
pub(crate) fn gui_registration_current_for_test(detail: String) -> CurrentDto {
    let mut state = test_state();
    current_dto(&mut state, gui_registration_required_bundle(detail).current)
}

#[cfg(test)]
pub(crate) async fn approval_failure_retains_handle_for_test(
    core: &DesktopCore,
    fence: CommandFence,
    approval: ApprovalHandle,
    pending: PendingApproval,
) -> (DesktopErrorDto, bool, bool) {
    let retry_fence = CommandFence::new(
        fence.project_handle.clone(),
        fence.generation.clone(),
        OperationId::new(),
    );
    core.inner
        .lock()
        .await
        .approvals
        .insert(approval.clone(), pending);
    let error = core
        .decide_approval_with(
            fence,
            approval.clone(),
            ApprovalDecisionDto::Approve,
            |_, _| async {
                Err::<ApprovalDecisionView, _>(ApprovalDecisionFailure {
                    detail: "The approval response was not observed.".to_owned(),
                    recovery: Some("Retry the exact decision.".to_owned()),
                })
            },
        )
        .await
        .expect_err("the injected transport failure must be returned");
    let retained = core.inner.lock().await.approvals.contains_key(&approval);
    let retry_authorized = core.begin(&retry_fence).await.is_ok()
        && core.inner.lock().await.approvals.contains_key(&approval);
    (error, retained, retry_authorized)
}

#[cfg(test)]
pub(crate) fn registration_contract_for_test(
    executable: &Path,
    root: &Path,
) -> (
    PathBuf,
    Vec<std::ffi::OsString>,
    Option<PathBuf>,
    bool,
    Duration,
) {
    let command = gui_registration_command(executable, root);
    (
        command.as_std().get_program().into(),
        command.as_std().get_args().map(Into::into).collect(),
        command.as_std().get_current_dir().map(PathBuf::from),
        command.get_kill_on_drop(),
        GUI_REGISTRATION_TIMEOUT,
    )
}

#[cfg(test)]
pub(crate) fn active_core_for_test(
    project: &ProjectHandle,
    generation: GenerationId,
) -> DesktopCore {
    active_core_at_for_test(project, generation, Path::new("/bounded/test/project"))
}

#[cfg(test)]
pub(crate) fn active_core_at_for_test(
    project: &ProjectHandle,
    generation: GenerationId,
    root: &Path,
) -> DesktopCore {
    let catalog = CatalogProject {
        handle: project.clone(),
        name: "project".to_owned(),
        root: root.to_path_buf(),
    };
    let mut state = test_state();
    state.catalog.insert(project.clone(), catalog.clone());
    state.active = Some(ActiveProject {
        catalog,
        project_id: ProjectId::new("internal-project-authority"),
        generation,
    });
    DesktopCore {
        inner: Arc::new(Mutex::new(state)),
        command_gate: Arc::new(Mutex::new(())),
        downloads: ModelDownloadManager::new(),
        imports: ModelImportManager::new(),
        startup_progress: StartupProgressCell::default(),
    }
}

#[cfg(test)]
pub(crate) async fn manage_skill_library_without_io_for_test(
    core: &DesktopCore,
    request: SkillLibraryRequest,
    switch_after_work: Option<(ProjectHandle, GenerationId)>,
) -> DesktopResult<SkillLibraryDto> {
    let inner = Arc::clone(&core.inner);
    core.manage_skill_library_with(request, move |_, _| async move {
        if let Some((project, generation)) = switch_after_work {
            let mut state = inner.lock().await;
            let catalog = CatalogProject {
                handle: project,
                name: "other".to_owned(),
                root: PathBuf::from("/bounded/test/other"),
            };
            state.active = Some(ActiveProject {
                catalog,
                project_id: ProjectId::new("other-internal-authority"),
                generation,
            });
            state.used_operations.clear();
        }
        Ok(crate::skill_library::empty_load_for_test())
    })
    .await
}

#[cfg(test)]
pub(crate) async fn reserve_for_test(
    core: &DesktopCore,
    fence: &CommandFence,
) -> DesktopResult<()> {
    core.begin(fence).await.map(drop)
}

#[cfg(test)]
pub(crate) fn command_gate_for_test(core: &DesktopCore) -> Arc<Mutex<()>> {
    Arc::clone(&core.command_gate)
}

#[cfg(test)]
pub(crate) async fn reserve_daemon_for_test(
    core: &DesktopCore,
    fence: &CommandFence,
) -> DesktopResult<()> {
    core.begin_daemon(fence).await
}

#[cfg(test)]
pub(crate) async fn flow_workspace_at_for_test(
    core: &DesktopCore,
    fence: CommandFence,
    root: PathBuf,
) -> DesktopResult<FlowWorkspaceDto> {
    core.flow_workspace_at(fence, root).await
}

#[cfg(test)]
pub(crate) async fn bootstrap_with_catalog_for_test(
    core: &DesktopCore,
    operation: OperationId,
    catalog: CatalogDto,
) -> DesktopResult<BootstrapDto> {
    core.bootstrap_with_catalog(operation, catalog).await
}

#[cfg(test)]
pub(crate) fn daemon_start_cwd_for_test(fallback: &Path) -> PathBuf {
    daemon_start_cwd(fallback)
}

#[cfg(test)]
pub(crate) async fn switch_authority_for_test(
    core: &DesktopCore,
    project: ProjectHandle,
    generation: GenerationId,
) {
    let mut state = core.inner.lock().await;
    let catalog = CatalogProject {
        handle: project,
        name: "other".to_owned(),
        root: PathBuf::from("/bounded/test/other"),
    };
    state.active = Some(ActiveProject {
        catalog,
        project_id: ProjectId::new("other-internal-authority"),
        generation,
    });
    state.used_operations.clear();
}

#[cfg(test)]
fn test_state() -> DesktopState {
    DesktopState {
        startup_root: PathBuf::from("/bounded/test"),
        daemon_executable: PathBuf::from("pam"),
        catalog: HashMap::new(),
        active: None,
        activation_operation: None,
        used_operations: VecDeque::new(),
        daemon_operations: VecDeque::new(),
        approvals: HashMap::new(),
        evidence: HashMap::new(),
        flow_workspace: None,
        daemon_child: None,
        catalog_warning: None,
    }
}
