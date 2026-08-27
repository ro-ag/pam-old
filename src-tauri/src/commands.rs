use std::sync::Arc;

use pam_gui::{
    AccessConfigDto, ActivityDto, AppSettingsDto, ApprovalDecisionDto, ApprovalDecisionResponseDto,
    ApprovalHandle, BootstrapDto, CallersDto, CatalogDto, CommandFence, ConnectorConfigureDto,
    ConnectorConfigureParams, ConnectorCredentialAction, ConnectorTestDto, ConnectorsDto,
    DaemonAccessDto, DaemonLogsDto, DaemonStatsDto, DesktopCore, DesktopErrorDto, DesktopErrorKind,
    EvidenceDto, EvidenceHandleDto, FlowComposeDto, FlowDefinitionHandle, FlowDocumentDto,
    FlowDocumentHandle, FlowGraphDto, FlowReviewDto, FlowSaveDto, FlowWorkspaceDto, GenerationId,
    HealthDto, HostMemoryDto, ModelDownloadDto, ModelDownloadStatusDto, ModelImportDto,
    ModelImportParams, ModelImportStatusDto, ModelInferDto, ModelInspectDto,
    ModelLicenseDiscoveryDto, ModelMessageDto, ModelPresetsDto, ModelStatusDto, OperationId,
    ProjectHandle, SkillAuditDto, SkillInventoryDto, SkillLibraryDto, SkillLibraryRequest,
    SnapshotDto,
};
use serde::{Deserialize, Deserializer, de::Error as _};
use tauri::State;

trait CanonicalHandle: Sized {
    fn parse(value: String) -> Result<Self, DesktopErrorDto>;
}

macro_rules! canonical_handle {
    ($($handle:ty),+ $(,)?) => {
        $(
            impl CanonicalHandle for $handle {
                fn parse(value: String) -> Result<Self, DesktopErrorDto> {
                    <$handle>::parse(value)
                }
            }
        )+
    };
}

canonical_handle!(
    ProjectHandle,
    GenerationId,
    OperationId,
    ApprovalHandle,
    EvidenceHandleDto,
    FlowDefinitionHandle,
    FlowDocumentHandle,
);

fn canonical_uuid<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: CanonicalHandle,
{
    T::parse(String::deserialize(deserializer)?).map_err(D::Error::custom)
}

pub(crate) struct DesktopState {
    core: Arc<DesktopCore>,
}

impl DesktopState {
    pub(crate) fn new(core: DesktopCore) -> Self {
        Self {
            core: Arc::new(core),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ActivateProjectRequest {
    #[serde(deserialize_with = "canonical_uuid")]
    project_handle: ProjectHandle,
    #[serde(deserialize_with = "canonical_uuid")]
    operation_id: OperationId,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct BootstrapRequest {
    #[serde(deserialize_with = "canonical_uuid")]
    operation_id: OperationId,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct StartDaemonRequest {
    #[serde(deserialize_with = "canonical_uuid")]
    project_handle: ProjectHandle,
    #[serde(deserialize_with = "canonical_uuid")]
    generation: GenerationId,
    #[serde(deserialize_with = "canonical_uuid")]
    operation_id: OperationId,
    #[serde(default)]
    model: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct FencedRequest {
    #[serde(deserialize_with = "canonical_uuid")]
    project_handle: ProjectHandle,
    #[serde(deserialize_with = "canonical_uuid")]
    generation: GenerationId,
    #[serde(deserialize_with = "canonical_uuid")]
    operation_id: OperationId,
}

impl FencedRequest {
    fn into_fence(self) -> CommandFence {
        CommandFence::new(self.project_handle, self.generation, self.operation_id)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ApprovalRequest {
    #[serde(deserialize_with = "canonical_uuid")]
    project_handle: ProjectHandle,
    #[serde(deserialize_with = "canonical_uuid")]
    generation: GenerationId,
    #[serde(deserialize_with = "canonical_uuid")]
    operation_id: OperationId,
    #[serde(deserialize_with = "canonical_uuid")]
    approval_handle: ApprovalHandle,
    decision: ApprovalDecisionDto,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct EvidenceRequest {
    #[serde(deserialize_with = "canonical_uuid")]
    project_handle: ProjectHandle,
    #[serde(deserialize_with = "canonical_uuid")]
    generation: GenerationId,
    #[serde(deserialize_with = "canonical_uuid")]
    operation_id: OperationId,
    #[serde(deserialize_with = "canonical_uuid")]
    evidence_handle: EvidenceHandleDto,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct OpenFlowRequest {
    #[serde(deserialize_with = "canonical_uuid")]
    project_handle: ProjectHandle,
    #[serde(deserialize_with = "canonical_uuid")]
    generation: GenerationId,
    #[serde(deserialize_with = "canonical_uuid")]
    operation_id: OperationId,
    #[serde(deserialize_with = "canonical_uuid")]
    flow_handle: FlowDefinitionHandle,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ReviewFlowRequest {
    #[serde(deserialize_with = "canonical_uuid")]
    project_handle: ProjectHandle,
    #[serde(deserialize_with = "canonical_uuid")]
    generation: GenerationId,
    #[serde(deserialize_with = "canonical_uuid")]
    operation_id: OperationId,
    #[serde(deserialize_with = "canonical_uuid")]
    document_handle: FlowDocumentHandle,
    source: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ActivityRequest {
    #[serde(deserialize_with = "canonical_uuid")]
    project_handle: ProjectHandle,
    #[serde(deserialize_with = "canonical_uuid")]
    generation: GenerationId,
    #[serde(deserialize_with = "canonical_uuid")]
    operation_id: OperationId,
    #[serde(default)]
    limit: Option<u32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ModelInferRequest {
    #[serde(deserialize_with = "canonical_uuid")]
    project_handle: ProjectHandle,
    #[serde(deserialize_with = "canonical_uuid")]
    generation: GenerationId,
    #[serde(deserialize_with = "canonical_uuid")]
    operation_id: OperationId,
    model: String,
    messages: Vec<ModelMessageDto>,
    #[serde(default)]
    max_output_tokens: Option<u32>,
}

// The license notice text is user-supplied consent material: this request
// struct must never derive Debug or log its fields wholesale.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ModelImportRequest {
    #[serde(deserialize_with = "canonical_uuid")]
    project_handle: ProjectHandle,
    #[serde(deserialize_with = "canonical_uuid")]
    generation: GenerationId,
    #[serde(deserialize_with = "canonical_uuid")]
    operation_id: OperationId,
    model: String,
    path: String,
    license_id: String,
    license_url: String,
    license_notice_text: String,
    #[serde(default)]
    allow_small: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ModelInspectRequest {
    #[serde(deserialize_with = "canonical_uuid")]
    project_handle: ProjectHandle,
    #[serde(deserialize_with = "canonical_uuid")]
    generation: GenerationId,
    #[serde(deserialize_with = "canonical_uuid")]
    operation_id: OperationId,
    path: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ModelLicenseDiscoverRequest {
    #[serde(deserialize_with = "canonical_uuid")]
    project_handle: ProjectHandle,
    #[serde(deserialize_with = "canonical_uuid")]
    generation: GenerationId,
    #[serde(deserialize_with = "canonical_uuid")]
    operation_id: OperationId,
    query: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ModelDownloadRequest {
    #[serde(deserialize_with = "canonical_uuid")]
    project_handle: ProjectHandle,
    #[serde(deserialize_with = "canonical_uuid")]
    generation: GenerationId,
    #[serde(deserialize_with = "canonical_uuid")]
    operation_id: OperationId,
    preset_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SettingsUpdateRequest {
    #[serde(deserialize_with = "canonical_uuid")]
    project_handle: ProjectHandle,
    #[serde(deserialize_with = "canonical_uuid")]
    generation: GenerationId,
    #[serde(deserialize_with = "canonical_uuid")]
    operation_id: OperationId,
    #[serde(default)]
    models_dir: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RevealPathRequest {
    #[serde(deserialize_with = "canonical_uuid")]
    project_handle: ProjectHandle,
    #[serde(deserialize_with = "canonical_uuid")]
    generation: GenerationId,
    #[serde(deserialize_with = "canonical_uuid")]
    operation_id: OperationId,
    path: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct FlowGraphRequest {
    #[serde(deserialize_with = "canonical_uuid")]
    project_handle: ProjectHandle,
    #[serde(deserialize_with = "canonical_uuid")]
    generation: GenerationId,
    #[serde(deserialize_with = "canonical_uuid")]
    operation_id: OperationId,
    source: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct FlowComposeRequest {
    #[serde(deserialize_with = "canonical_uuid")]
    project_handle: ProjectHandle,
    #[serde(deserialize_with = "canonical_uuid")]
    generation: GenerationId,
    #[serde(deserialize_with = "canonical_uuid")]
    operation_id: OperationId,
    definition: String,
}

// The optional credential secret is a debug-redacted pass-through value: this
// request struct must never derive Debug or log its fields.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ConnectorConfigureRequest {
    #[serde(deserialize_with = "canonical_uuid")]
    project_handle: ProjectHandle,
    #[serde(deserialize_with = "canonical_uuid")]
    generation: GenerationId,
    #[serde(deserialize_with = "canonical_uuid")]
    operation_id: OperationId,
    connector: String,
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    base_url: Option<String>,
    #[serde(default)]
    credential: Option<ConnectorCredentialAction>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ConnectorTestRequest {
    #[serde(deserialize_with = "canonical_uuid")]
    project_handle: ProjectHandle,
    #[serde(deserialize_with = "canonical_uuid")]
    generation: GenerationId,
    #[serde(deserialize_with = "canonical_uuid")]
    operation_id: OperationId,
    connector: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DaemonAccessRequest {
    #[serde(deserialize_with = "canonical_uuid")]
    project_handle: ProjectHandle,
    #[serde(deserialize_with = "canonical_uuid")]
    generation: GenerationId,
    #[serde(deserialize_with = "canonical_uuid")]
    operation_id: OperationId,
    capability: String,
    granted: bool,
}

fn fence(
    project_handle: ProjectHandle,
    generation: GenerationId,
    operation_id: OperationId,
) -> CommandFence {
    CommandFence::new(project_handle, generation, operation_id)
}

#[tauri::command]
pub(crate) async fn bootstrap(
    state: State<'_, DesktopState>,
    request: BootstrapRequest,
) -> Result<BootstrapDto, DesktopErrorDto> {
    state.core.bootstrap(request.operation_id).await
}

#[tauri::command]
pub(crate) async fn catalog(state: State<'_, DesktopState>) -> Result<CatalogDto, DesktopErrorDto> {
    Ok(state.core.catalog().await)
}

#[tauri::command]
pub(crate) async fn activate_project(
    state: State<'_, DesktopState>,
    request: ActivateProjectRequest,
) -> Result<SnapshotDto, DesktopErrorDto> {
    state
        .core
        .activate(request.project_handle, request.operation_id)
        .await
}

#[tauri::command]
pub(crate) async fn refresh_project(
    state: State<'_, DesktopState>,
    request: FencedRequest,
) -> Result<SnapshotDto, DesktopErrorDto> {
    state.core.refresh(request.into_fence()).await
}

#[tauri::command]
pub(crate) async fn start_daemon(
    state: State<'_, DesktopState>,
    request: StartDaemonRequest,
) -> Result<Option<SnapshotDto>, DesktopErrorDto> {
    let fence = CommandFence::new(
        request.project_handle,
        request.generation,
        request.operation_id,
    );
    state.core.start_daemon(fence, request.model).await
}

#[tauri::command]
pub(crate) async fn stop_daemon(
    state: State<'_, DesktopState>,
    request: FencedRequest,
) -> Result<Option<SnapshotDto>, DesktopErrorDto> {
    state.core.stop_daemon(request.into_fence()).await
}

#[tauri::command]
pub(crate) async fn daemon_health(
    state: State<'_, DesktopState>,
    request: FencedRequest,
) -> Result<HealthDto, DesktopErrorDto> {
    state.core.daemon_health(request.into_fence()).await
}

#[tauri::command]
pub(crate) async fn register_gui_caller(
    state: State<'_, DesktopState>,
    request: FencedRequest,
) -> Result<SnapshotDto, DesktopErrorDto> {
    state.core.register_gui_caller(request.into_fence()).await
}

#[tauri::command]
pub(crate) async fn decide_approval(
    state: State<'_, DesktopState>,
    request: ApprovalRequest,
) -> Result<ApprovalDecisionResponseDto, DesktopErrorDto> {
    state
        .core
        .decide_approval(
            fence(
                request.project_handle,
                request.generation,
                request.operation_id,
            ),
            request.approval_handle,
            request.decision,
        )
        .await
}

#[tauri::command]
pub(crate) async fn load_evidence(
    state: State<'_, DesktopState>,
    request: EvidenceRequest,
) -> Result<EvidenceDto, DesktopErrorDto> {
    state
        .core
        .load_evidence(
            fence(
                request.project_handle,
                request.generation,
                request.operation_id,
            ),
            request.evidence_handle,
        )
        .await
}

#[tauri::command]
pub(crate) async fn load_flow_workspace(
    state: State<'_, DesktopState>,
    request: FencedRequest,
) -> Result<FlowWorkspaceDto, DesktopErrorDto> {
    state.core.flow_workspace(request.into_fence()).await
}

#[tauri::command]
pub(crate) async fn daemon_activity(
    state: State<'_, DesktopState>,
    request: ActivityRequest,
) -> Result<ActivityDto, DesktopErrorDto> {
    state
        .core
        .daemon_activity(
            fence(
                request.project_handle,
                request.generation,
                request.operation_id,
            ),
            request.limit,
        )
        .await
}

#[tauri::command]
pub(crate) async fn daemon_logs(
    state: State<'_, DesktopState>,
    request: ActivityRequest,
) -> Result<DaemonLogsDto, DesktopErrorDto> {
    state
        .core
        .daemon_logs(
            fence(
                request.project_handle,
                request.generation,
                request.operation_id,
            ),
            request.limit,
        )
        .await
}

#[tauri::command]
pub(crate) async fn daemon_stats(
    state: State<'_, DesktopState>,
    request: ActivityRequest,
) -> Result<DaemonStatsDto, DesktopErrorDto> {
    state
        .core
        .daemon_stats(
            fence(
                request.project_handle,
                request.generation,
                request.operation_id,
            ),
            request.limit,
        )
        .await
}

#[tauri::command]
pub(crate) async fn caller_registry(
    state: State<'_, DesktopState>,
    request: FencedRequest,
) -> Result<CallersDto, DesktopErrorDto> {
    state.core.caller_registry(request.into_fence()).await
}

#[tauri::command]
pub(crate) async fn daemon_access(
    state: State<'_, DesktopState>,
    request: FencedRequest,
) -> Result<DaemonAccessDto, DesktopErrorDto> {
    state.core.daemon_access(request.into_fence()).await
}

#[tauri::command]
pub(crate) async fn daemon_access_config(
    state: State<'_, DesktopState>,
    request: FencedRequest,
) -> Result<AccessConfigDto, DesktopErrorDto> {
    state.core.daemon_access_config(request.into_fence()).await
}

#[tauri::command]
pub(crate) async fn set_daemon_access(
    state: State<'_, DesktopState>,
    request: DaemonAccessRequest,
) -> Result<DaemonAccessDto, DesktopErrorDto> {
    state
        .core
        .set_daemon_access(
            fence(
                request.project_handle,
                request.generation,
                request.operation_id,
            ),
            request.capability,
            request.granted,
        )
        .await
}

#[tauri::command]
pub(crate) async fn model_status(
    state: State<'_, DesktopState>,
    request: FencedRequest,
) -> Result<ModelStatusDto, DesktopErrorDto> {
    state.core.model_status(request.into_fence()).await
}

#[tauri::command]
pub(crate) async fn model_infer(
    state: State<'_, DesktopState>,
    request: ModelInferRequest,
) -> Result<ModelInferDto, DesktopErrorDto> {
    state
        .core
        .model_infer(
            fence(
                request.project_handle,
                request.generation,
                request.operation_id,
            ),
            request.model,
            request.messages,
            request.max_output_tokens,
        )
        .await
}

#[tauri::command]
pub(crate) async fn model_import(
    state: State<'_, DesktopState>,
    request: ModelImportRequest,
) -> Result<ModelImportDto, DesktopErrorDto> {
    state
        .core
        .model_import(
            fence(
                request.project_handle,
                request.generation,
                request.operation_id,
            ),
            ModelImportParams {
                model: request.model,
                path: request.path.into(),
                license_id: request.license_id,
                license_url: request.license_url,
                license_notice_text: request.license_notice_text,
                allow_small: request.allow_small,
            },
        )
        .await
}

#[tauri::command]
pub(crate) async fn model_import_status(
    state: State<'_, DesktopState>,
    request: FencedRequest,
) -> Result<ModelImportStatusDto, DesktopErrorDto> {
    state.core.model_import_status(request.into_fence()).await
}

#[tauri::command]
pub(crate) async fn model_inspect(
    state: State<'_, DesktopState>,
    request: ModelInspectRequest,
) -> Result<ModelInspectDto, DesktopErrorDto> {
    state
        .core
        .model_inspect(
            fence(
                request.project_handle,
                request.generation,
                request.operation_id,
            ),
            request.path,
        )
        .await
}

#[tauri::command]
pub(crate) async fn model_license_discover(
    state: State<'_, DesktopState>,
    request: ModelLicenseDiscoverRequest,
) -> Result<ModelLicenseDiscoveryDto, DesktopErrorDto> {
    state
        .core
        .model_license_discover(
            fence(
                request.project_handle,
                request.generation,
                request.operation_id,
            ),
            request.query,
        )
        .await
}

#[tauri::command]
pub(crate) async fn model_presets(
    state: State<'_, DesktopState>,
    request: FencedRequest,
) -> Result<ModelPresetsDto, DesktopErrorDto> {
    state.core.model_presets(request.into_fence()).await
}

#[tauri::command]
pub(crate) async fn model_download(
    state: State<'_, DesktopState>,
    request: ModelDownloadRequest,
) -> Result<ModelDownloadDto, DesktopErrorDto> {
    state
        .core
        .model_download(
            fence(
                request.project_handle,
                request.generation,
                request.operation_id,
            ),
            request.preset_id,
        )
        .await
}

#[tauri::command]
pub(crate) async fn model_download_status(
    state: State<'_, DesktopState>,
    request: FencedRequest,
) -> Result<ModelDownloadStatusDto, DesktopErrorDto> {
    state.core.model_download_status(request.into_fence()).await
}

#[tauri::command]
pub(crate) async fn model_download_cancel(
    state: State<'_, DesktopState>,
    request: FencedRequest,
) -> Result<ModelDownloadDto, DesktopErrorDto> {
    state.core.model_download_cancel(request.into_fence()).await
}

#[tauri::command]
pub(crate) async fn host_memory(
    state: State<'_, DesktopState>,
    request: FencedRequest,
) -> Result<HostMemoryDto, DesktopErrorDto> {
    state.core.host_memory(request.into_fence()).await
}

#[tauri::command]
pub(crate) async fn app_settings(
    state: State<'_, DesktopState>,
    request: FencedRequest,
) -> Result<AppSettingsDto, DesktopErrorDto> {
    state.core.app_settings(request.into_fence()).await
}

#[tauri::command]
pub(crate) async fn settings_update(
    state: State<'_, DesktopState>,
    request: SettingsUpdateRequest,
) -> Result<AppSettingsDto, DesktopErrorDto> {
    state
        .core
        .settings_update(
            fence(
                request.project_handle,
                request.generation,
                request.operation_id,
            ),
            request.models_dir,
        )
        .await
}

#[tauri::command]
pub(crate) async fn logs_delete(
    state: State<'_, DesktopState>,
    request: FencedRequest,
) -> Result<AppSettingsDto, DesktopErrorDto> {
    state.core.logs_delete(request.into_fence()).await
}

/// Validates `request.path` against PAM's own Settings locations, then opens
/// it in the system file manager. Deliberately not the opener plugin's own
/// `reveal_item_in_dir` IPC command: the frontend never gets a permission to
/// reveal an arbitrary path, only this validated one.
#[tauri::command]
pub(crate) async fn reveal_path(
    state: State<'_, DesktopState>,
    request: RevealPathRequest,
) -> Result<(), DesktopErrorDto> {
    state
        .core
        .reveal_path(
            fence(
                request.project_handle,
                request.generation,
                request.operation_id,
            ),
            request.path.clone(),
        )
        .await?;
    tauri_plugin_opener::reveal_item_in_dir(&request.path).map_err(|error| DesktopErrorDto {
        kind: DesktopErrorKind::Unavailable,
        message: error.to_string(),
        recovery: None,
    })
}

#[tauri::command]
pub(crate) async fn connector_registry(
    state: State<'_, DesktopState>,
    request: FencedRequest,
) -> Result<ConnectorsDto, DesktopErrorDto> {
    state.core.connector_registry(request.into_fence()).await
}

#[tauri::command]
pub(crate) async fn connector_configure(
    state: State<'_, DesktopState>,
    request: ConnectorConfigureRequest,
) -> Result<ConnectorConfigureDto, DesktopErrorDto> {
    state
        .core
        .connector_configure(
            fence(
                request.project_handle,
                request.generation,
                request.operation_id,
            ),
            ConnectorConfigureParams {
                connector: request.connector,
                enabled: request.enabled,
                base_url: request.base_url,
                credential: request.credential,
            },
        )
        .await
}

#[tauri::command]
pub(crate) async fn connector_test(
    state: State<'_, DesktopState>,
    request: ConnectorTestRequest,
) -> Result<ConnectorTestDto, DesktopErrorDto> {
    state
        .core
        .connector_test(
            fence(
                request.project_handle,
                request.generation,
                request.operation_id,
            ),
            request.connector,
        )
        .await
}

#[tauri::command]
pub(crate) async fn flow_graph(
    state: State<'_, DesktopState>,
    request: FlowGraphRequest,
) -> Result<FlowGraphDto, DesktopErrorDto> {
    state
        .core
        .flow_graph(
            fence(
                request.project_handle,
                request.generation,
                request.operation_id,
            ),
            request.source,
        )
        .await
}

#[tauri::command]
pub(crate) async fn flow_compose(
    state: State<'_, DesktopState>,
    request: FlowComposeRequest,
) -> Result<FlowComposeDto, DesktopErrorDto> {
    state
        .core
        .flow_compose(
            fence(
                request.project_handle,
                request.generation,
                request.operation_id,
            ),
            request.definition,
        )
        .await
}

#[tauri::command]
pub(crate) async fn load_skill_inventory(
    state: State<'_, DesktopState>,
    request: FencedRequest,
) -> Result<SkillInventoryDto, DesktopErrorDto> {
    state.core.skill_inventory(request.into_fence()).await
}

#[tauri::command]
pub(crate) async fn manage_skill_library(
    state: State<'_, DesktopState>,
    request: SkillLibraryRequest,
) -> Result<SkillLibraryDto, DesktopErrorDto> {
    state.core.manage_skill_library(request).await
}

#[tauri::command]
pub(crate) async fn load_skill_audit(
    state: State<'_, DesktopState>,
    request: FencedRequest,
) -> Result<SkillAuditDto, DesktopErrorDto> {
    state.core.load_skill_audit(request.into_fence()).await
}

#[tauri::command]
pub(crate) async fn run_skill_audit(
    state: State<'_, DesktopState>,
    request: FencedRequest,
) -> Result<SkillAuditDto, DesktopErrorDto> {
    state.core.run_skill_audit(request.into_fence()).await
}

#[tauri::command]
pub(crate) async fn open_flow(
    state: State<'_, DesktopState>,
    request: OpenFlowRequest,
) -> Result<FlowDocumentDto, DesktopErrorDto> {
    state
        .core
        .open_flow(
            fence(
                request.project_handle,
                request.generation,
                request.operation_id,
            ),
            request.flow_handle,
        )
        .await
}

#[tauri::command]
pub(crate) async fn validate_flow(
    state: State<'_, DesktopState>,
    request: ReviewFlowRequest,
) -> Result<FlowReviewDto, DesktopErrorDto> {
    state
        .core
        .validate_flow(
            fence(
                request.project_handle,
                request.generation,
                request.operation_id,
            ),
            request.document_handle,
            request.source,
        )
        .await
}

#[tauri::command]
pub(crate) async fn save_flow(
    state: State<'_, DesktopState>,
    request: ReviewFlowRequest,
) -> Result<FlowSaveDto, DesktopErrorDto> {
    state
        .core
        .save_flow(
            fence(
                request.project_handle,
                request.generation,
                request.operation_id,
            ),
            request.document_handle,
            request.source,
        )
        .await
}
