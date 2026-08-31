#![forbid(unsafe_code)]

mod access_config;
mod control_center;
mod current;
mod daemon_access;
mod desktop;
mod flow_editor;
mod flow_runs;
mod model_discovery;
mod model_download;
mod model_import;
mod model_presets;
mod observatory;
mod settings;
mod skill_audit;
mod skill_inventory;
mod skill_library;
mod store_writes;

#[cfg(test)]
mod access_config_test;
#[cfg(test)]
mod control_center_test;
#[cfg(test)]
mod current_test;
#[cfg(test)]
mod daemon_access_test;
#[cfg(test)]
mod desktop_test;
#[cfg(test)]
mod flow_editor_test;
#[cfg(test)]
mod flow_runs_test;
#[cfg(test)]
mod model_discovery_test;
#[cfg(test)]
mod model_download_test;
#[cfg(test)]
mod model_import_test;
#[cfg(test)]
mod model_presets_test;
#[cfg(test)]
mod observatory_test;
#[cfg(test)]
mod settings_test;
#[cfg(test)]
mod skill_audit_test;
#[cfg(test)]
mod skill_inventory_test;
#[cfg(test)]
mod skill_library_test;
#[cfg(test)]
mod store_writes_test;

pub use desktop::{
    AccessConfigDto, ActivityDayDto, ActivityDto, ActivityEventDto, AppSettingsDto,
    ApprovalDecisionDispositionDto, ApprovalDecisionDto, ApprovalDecisionResponseDto,
    ApprovalHandle, BootstrapDto, CallerDto, CallersDto, CatalogDto, CommandFence,
    ConnectorConfigureDto, ConnectorConfigureParams, ConnectorSummaryDto, ConnectorTestDto,
    ConnectorsDto, CurrentDto, DaemonLogEntryDto, DaemonLogsDto, DaemonStartupProgressDto,
    DaemonStatsDto, DesktopCore, DesktopErrorDto, DesktopErrorKind, DesktopResult, EvidenceDataDto,
    EvidenceDto, EvidenceHandleDto, FailureDto, FailureKindDto, FlowComposeDto, FlowDefinitionDto,
    FlowDefinitionHandle, FlowDocumentDataDto, FlowDocumentDto, FlowDocumentHandle, FlowDryRunDto,
    FlowDryRunStepDto, FlowGraphDto, FlowIdentityDto, FlowReviewDataDto, FlowReviewDto,
    FlowSaveDataDto, FlowSaveDto, FlowVersionDiffDto, FlowVersionDiffLineDto, FlowWorkspaceDataDto,
    FlowWorkspaceDto, GenerationId, HealthDto, HostMemoryDto, ModelDownloadDto,
    ModelDownloadStatusDto, ModelDownloadStatusKindDto, ModelImportDto, ModelImportStageDto,
    ModelImportStatusDto, ModelImportStatusKindDto, ModelInferDto, ModelInspectDto,
    ModelLicenseDiscoveryDto, ModelMessageDto, ModelPresetDto, ModelPresetsDto, ModelRoleDto,
    ModelStatusDto, ModelSummaryDto, ModelUnregisterDto, ModelUsageDto, OperationId, OutcomeDto,
    OutcomeSectionDto, ProjectHandle, ProjectSummaryDto, ProjectUsageDto, RequestSummaryDto,
    ResetDto, ResetItemDto, ResetResultDto, RunDto, SnapshotDataDto, SnapshotDto, SnapshotFence,
    StartupPhaseDto, TimelineFactDto, TimelineKindDto,
};
pub use pam_protocol::ResetTier;

// Re-exported so the desktop shell can accept the debug-redacted credential
// action without depending on pam_protocol directly.
pub use pam_protocol::{ConnectorCredentialAction, ConnectorSecret};

pub use daemon_access::{DaemonAccessDto, DaemonCapabilityDto};

pub use flow_runs::{
    FlowRunCancelDataDto, FlowRunCancelDto, FlowRunDataDto, FlowRunDto, FlowRunHistoryDataDto,
    FlowRunHistoryDto, FlowRunHistoryEntryDto, FlowRunProgressDataDto, FlowRunProgressDto,
};

pub use model_import::ModelImportParams;

pub use skill_inventory::{
    CursorGlobalRulesStatusDto, SkillArtifactDto, SkillInventoryDataDto, SkillInventoryDriftDto,
    SkillInventoryDto,
};

pub use skill_library::{
    SKILL_LIBRARY_DTO_SCHEMA_VERSION, SkillLibraryAgentDto, SkillLibraryCleanupDto,
    SkillLibraryDataDto, SkillLibraryDispositionDto, SkillLibraryDriftConflictDto,
    SkillLibraryDriftDto, SkillLibraryDriftStateDto, SkillLibraryDto, SkillLibraryEntryDto,
    SkillLibraryFileMetadataDto, SkillLibraryInstallationDto, SkillLibraryKeyDto,
    SkillLibraryMaterializationActionDto, SkillLibraryOutcomeDto, SkillLibraryPlanItemDto,
    SkillLibraryRequest, SkillLibraryVersionDto,
};

pub use skill_audit::{
    SkillAuditArtifactDto, SkillAuditDataDto, SkillAuditDto, SkillAuditEvaluationDto,
    SkillAuditEvaluatorDto, SkillAuditFailureDto, SkillAuditFootprintDto,
    SkillAuditMultiArtifactFindingDto, SkillAuditOriginSessionDto, SkillAuditSaturationGradeDto,
    SkillAuditScopeTotalDto, SkillAuditStaleCandidateDto, SkillAuditVerdictDto,
};

pub use flow_editor::{
    ActionAuthority, DaemonAuthority, DryRunCondition, DryRunStep, FlowCatalogEntry,
    FlowDryRunPlan, FlowEditorDocument, FlowEditorError, FlowEditorModel, FlowEditorValidation,
    FlowIdentity, FlowSaveInteraction, FlowSaveResult, FlowVersionDiff, FlowVersionDiffLine,
    FlowVersionDiffLineKind, MAX_FLOW_CATALOG_BYTES, MAX_FLOW_CATALOG_ENTRIES,
    MAX_VERSION_DIFF_LINES, UnsupportedDaemonAuthority,
};
