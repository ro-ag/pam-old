import { invoke } from "@tauri-apps/api/core";
import type {
  AccessConfigDto,
  ActivityDto,
  AppSettingsDto,
  BootstrapResponse,
  ApprovalDecisionResponseDto,
  CallersDto,
  CatalogDto,
  CommandFence,
  ConnectorConfigureDto,
  ConnectorConfigureParams,
  ConnectorTestDto,
  ConnectorsDto,
  DaemonAccessDto,
  DaemonLogsDto,
  DaemonStartupProgressDto,
  DaemonStatsDto,
  EvidenceDto,
  FlowComposeDto,
  FlowDefinitionJson,
  FlowDocumentDto,
  FlowGraphDto,
  FlowReviewDto,
  FlowSaveDto,
  FlowWorkspaceDto,
  ChatMessageDto,
  HealthDto,
  HostMemoryDto,
  ModelDownloadDto,
  ModelDownloadStatusDto,
  ModelImportDto,
  ModelImportParams,
  ModelImportStatusDto,
  ModelInspectDto,
  ModelLicenseDiscoveryDto,
  ModelInferDto,
  ModelPresetsDto,
  ModelStatusDto,
  PamBridge,
  SkillAuditDto,
  SkillInventoryDto,
  SkillLibraryDto,
  SnapshotDto,
} from "./domain";
import { fixtureBridge, type FixtureScenario } from "./fixtures";

type Invoke = <T>(command: string, args?: Record<string, unknown>) => Promise<T>;

const flatFence = ({ projectHandle, generation, operationId }: CommandFence) => ({
  projectHandle,
  generation,
  operationId,
});

const request = (payload: Record<string, unknown>) => ({ request: payload });

export function nextOperationId(): string {
  if (globalThis.crypto?.randomUUID) return globalThis.crypto.randomUUID();
  const bytes = new Uint8Array(16);
  globalThis.crypto?.getRandomValues?.(bytes);
  if (!bytes.some(Boolean)) {
    for (let index = 0; index < bytes.length; index += 1) bytes[index] = Math.floor(Math.random() * 256);
  }
  bytes[6] = (bytes[6] & 0x0f) | 0x40;
  bytes[8] = (bytes[8] & 0x3f) | 0x80;
  const hex = Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}

export function sameFence(left: CommandFence | null | undefined, right: CommandFence | null | undefined): boolean {
  return Boolean(
    left &&
      right &&
      left.projectHandle === right.projectHandle &&
      left.generation === right.generation &&
      left.operationId === right.operationId,
  );
}

// Snapshot commands rotate the generation server-side and return the successor
// fence; a response answers a request when it echoes its handle and operation.
export function answersFence(requestFence: CommandFence | null | undefined, responseFence: CommandFence): boolean {
  return Boolean(
    requestFence &&
      requestFence.projectHandle === responseFence.projectHandle &&
      requestFence.operationId === responseFence.operationId,
  );
}

export function withOperation(fence: CommandFence): CommandFence {
  return { ...fence, operationId: nextOperationId() };
}

// The reserved authority literal for daemon-scoped commands. Daemon-global
// loaders always mint this fence; project-scoped commands reject it.
export const DAEMON_AUTHORITY = "daemon";

export function withDaemonOperation(): CommandFence {
  return { projectHandle: DAEMON_AUTHORITY, generation: DAEMON_AUTHORITY, operationId: nextOperationId() };
}

export function createTauriBridge(invokeCommand: Invoke = invoke): PamBridge {
  return {
    mode: "native",
    bootstrap: () => invokeCommand<BootstrapResponse>("bootstrap", request({ operationId: nextOperationId() })),
    catalog: () => invokeCommand<CatalogDto>("catalog"),
    daemonHealth: (fence) =>
      invokeCommand<HealthDto>("daemon_health", request(flatFence(fence))),
    daemonActivity: (fence, limit) =>
      invokeCommand<ActivityDto>("daemon_activity", request({
        ...flatFence(fence),
        limit: limit ?? null,
      })),
    daemonLogs: (fence, limit) =>
      invokeCommand<DaemonLogsDto>("daemon_logs", request({
        ...flatFence(fence),
        limit: limit ?? null,
      })),
    daemonStats: (fence, days) =>
      invokeCommand<DaemonStatsDto>("daemon_stats", request({
        ...flatFence(fence),
        limit: days ?? null,
      })),
    callerRegistry: (fence) =>
      invokeCommand<CallersDto>("caller_registry", request(flatFence(fence))),
    daemonAccess: (fence) =>
      invokeCommand<DaemonAccessDto>("daemon_access", request(flatFence(fence))),
    daemonAccessConfig: (fence) =>
      invokeCommand<AccessConfigDto>("daemon_access_config", request(flatFence(fence))),
    setDaemonAccess: (fence, capability, granted) =>
      invokeCommand<DaemonAccessDto>("set_daemon_access", request({
        ...flatFence(fence),
        capability,
        granted,
      })),
    connectorRegistry: (fence) =>
      invokeCommand<ConnectorsDto>("connector_registry", request(flatFence(fence))),
    connectorConfigure: (fence, params: ConnectorConfigureParams) =>
      invokeCommand<ConnectorConfigureDto>("connector_configure", request({
        ...flatFence(fence),
        ...params,
      })),
    connectorTest: (fence, connector) =>
      invokeCommand<ConnectorTestDto>("connector_test", request({
        ...flatFence(fence),
        connector,
      })),
    modelStatus: (fence) =>
      invokeCommand<ModelStatusDto>("model_status", request(flatFence(fence))),
    modelInfer: (fence, model, messages: ChatMessageDto[], maxOutputTokens) =>
      invokeCommand<ModelInferDto>("model_infer", request({
        ...flatFence(fence),
        model,
        messages,
        ...(maxOutputTokens === undefined ? {} : { maxOutputTokens }),
      })),
    modelImport: (fence, params: ModelImportParams) =>
      invokeCommand<ModelImportDto>("model_import", request({
        ...flatFence(fence),
        model: params.model,
        path: params.path,
        licenseId: params.licenseId,
        licenseUrl: params.licenseUrl,
        licenseNoticeText: params.licenseNoticeText,
        allowSmall: params.allowSmall,
      })),
    modelImportStatus: (fence) =>
      invokeCommand<ModelImportStatusDto>("model_import_status", request(flatFence(fence))),
    modelInspect: (fence, path) =>
      invokeCommand<ModelInspectDto>("model_inspect", request({
        ...flatFence(fence),
        path,
      })),
    modelLicenseDiscover: (fence, query) =>
      invokeCommand<ModelLicenseDiscoveryDto>("model_license_discover", request({
        ...flatFence(fence),
        query,
      })),
    modelPresets: (fence) =>
      invokeCommand<ModelPresetsDto>("model_presets", request(flatFence(fence))),
    modelDownload: (fence, presetId) =>
      invokeCommand<ModelDownloadDto>("model_download", request({
        ...flatFence(fence),
        presetId,
      })),
    modelDownloadStatus: (fence) =>
      invokeCommand<ModelDownloadStatusDto>("model_download_status", request(flatFence(fence))),
    modelDownloadCancel: (fence) =>
      invokeCommand<ModelDownloadDto>("model_download_cancel", request(flatFence(fence))),
    hostMemory: (fence) =>
      invokeCommand<HostMemoryDto>("host_memory", request(flatFence(fence))),
    appSettings: (fence) =>
      invokeCommand<AppSettingsDto>("app_settings", request(flatFence(fence))),
    settingsUpdate: (fence, modelsDir) =>
      invokeCommand<AppSettingsDto>("settings_update", request({
        ...flatFence(fence),
        modelsDir,
      })),
    logsDelete: (fence) =>
      invokeCommand<AppSettingsDto>("logs_delete", request(flatFence(fence))),
    revealPath: (fence, path) =>
      invokeCommand<void>("reveal_path", request({ ...flatFence(fence), path })),
    activateProject: (projectHandle, operationId) =>
      invokeCommand<SnapshotDto>(
        "activate_project",
        request({ projectHandle, operationId }),
      ),
    refreshProject: (fence) =>
      invokeCommand<SnapshotDto>("refresh_project", request(flatFence(fence))),
    startDaemon: (fence, model) =>
      invokeCommand<SnapshotDto | null>("start_daemon", request({
        ...flatFence(fence),
        ...(model === undefined ? {} : { model }),
      })),
    // Never behind the command gate: `start_daemon` holds it for the whole
    // load, and there is no event bus in this codebase, so the GUI polls.
    daemonStartupProgress: (fence) =>
      invokeCommand<DaemonStartupProgressDto>("daemon_startup_progress", request(flatFence(fence))),
    stopDaemon: (fence) =>
      invokeCommand<SnapshotDto | null>("stop_daemon", request(flatFence(fence))),
    registerGuiCaller: (fence) =>
      invokeCommand<SnapshotDto>("register_gui_caller", request(flatFence(fence))),
    decideApproval: (fence, approvalHandle, decision) =>
      invokeCommand<ApprovalDecisionResponseDto>("decide_approval", request({
        ...flatFence(fence),
        approvalHandle,
        decision,
      })),
    loadEvidence: (fence, evidenceHandle) =>
      invokeCommand<EvidenceDto>("load_evidence", request({
        ...flatFence(fence),
        evidenceHandle,
      })),
    loadFlowWorkspace: (fence) =>
      invokeCommand<FlowWorkspaceDto>("load_flow_workspace", request(flatFence(fence))),
    loadSkillInventory: (fence) =>
      invokeCommand<SkillInventoryDto>("load_skill_inventory", request(flatFence(fence))),
    manageSkillLibrary: (fence, action) =>
      invokeCommand<SkillLibraryDto>("manage_skill_library", request({
        ...flatFence(fence),
        ...action,
      })),
    loadSkillAudit: (fence) =>
      invokeCommand<SkillAuditDto>("load_skill_audit", request(flatFence(fence))),
    runSkillAudit: (fence) =>
      invokeCommand<SkillAuditDto>("run_skill_audit", request(flatFence(fence))),
    openFlow: (fence, flowHandle) =>
      invokeCommand<FlowDocumentDto>(
        "open_flow",
        request({ ...flatFence(fence), flowHandle }),
      ),
    flowGraph: (fence, source) =>
      invokeCommand<FlowGraphDto>("flow_graph", request({ ...flatFence(fence), source })),
    flowCompose: (fence, definition: FlowDefinitionJson) =>
      // The desktop command takes the definition as JSON text, not an object.
      invokeCommand<FlowComposeDto>("flow_compose", request({ ...flatFence(fence), definition: JSON.stringify(definition) })),
    validateFlow: (fence, documentHandle, source) =>
      invokeCommand<FlowReviewDto>("validate_flow", request({
        ...flatFence(fence),
        documentHandle,
        source,
      })),
    saveFlow: (fence, documentHandle, source) =>
      invokeCommand<FlowSaveDto>("save_flow", request({
        ...flatFence(fence),
        documentHandle,
        source,
      })),
  };
}

export function createFixtureBridge(scenario: FixtureScenario = "solved"): PamBridge {
  return fixtureBridge(scenario);
}
