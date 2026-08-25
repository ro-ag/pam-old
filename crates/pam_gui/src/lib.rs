#![forbid(unsafe_code)]

mod access_config;
mod control_center;
mod current;
mod desktop;
mod flow_editor;
mod model_download;
mod model_import;
mod model_presets;
mod observatory;
mod skill_audit;
mod skill_inventory;
mod skill_library;

#[cfg(test)]
mod access_config_test;
#[cfg(test)]
mod control_center_test;
#[cfg(test)]
mod current_test;
#[cfg(test)]
mod desktop_test;
#[cfg(test)]
mod flow_editor_test;
#[cfg(test)]
mod model_download_test;
#[cfg(test)]
mod model_import_test;
#[cfg(test)]
mod model_presets_test;
#[cfg(test)]
mod observatory_test;
#[cfg(test)]
mod skill_audit_test;
#[cfg(test)]
mod skill_inventory_test;
#[cfg(test)]
mod skill_library_test;

pub use desktop::{
    AccessConfigDto, ActivityDayDto, ActivityDto, ActivityEventDto, ApprovalDecisionDispositionDto,
    ApprovalDecisionDto, ApprovalDecisionResponseDto, ApprovalHandle, BootstrapDto, CallerDto,
    CallersDto, CatalogDto, CommandFence, ConnectorConfigureDto, ConnectorConfigureParams,
    ConnectorSummaryDto, ConnectorTestDto, ConnectorsDto, CurrentDto, DaemonLogEntryDto,
    DaemonLogsDto, DaemonStatsDto, DesktopCore, DesktopErrorDto, DesktopErrorKind, DesktopResult,
    EvidenceDataDto, EvidenceDto, EvidenceHandleDto, FailureDto, FailureKindDto, FlowComposeDto,
    FlowDefinitionDto, FlowDefinitionHandle, FlowDocumentDataDto, FlowDocumentDto,
    FlowDocumentHandle, FlowDryRunDto, FlowDryRunStepDto, FlowGraphDto, FlowIdentityDto,
    FlowReviewDataDto, FlowReviewDto, FlowSaveDataDto, FlowSaveDto, FlowVersionDiffDto,
    FlowVersionDiffLineDto, FlowWorkspaceDataDto, FlowWorkspaceDto, GenerationId, HealthDto,
    HostMemoryDto, ModelDownloadDto, ModelDownloadStatusDto, ModelDownloadStatusKindDto,
    ModelImportDto, ModelInferDto, ModelMessageDto, ModelPresetDto, ModelPresetsDto, ModelRoleDto,
    ModelStatusDto, ModelSummaryDto, ModelUsageDto, OperationId, OutcomeDto, OutcomeSectionDto,
    ProjectHandle, ProjectSummaryDto, ProjectUsageDto, RequestSummaryDto, RunDto, SnapshotDataDto,
    SnapshotDto, SnapshotFence, TimelineFactDto, TimelineKindDto,
};

// Re-exported so the desktop shell can accept the debug-redacted credential
// action without depending on pam_protocol directly.
pub use pam_protocol::{ConnectorCredentialAction, ConnectorSecret};

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
