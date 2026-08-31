export type ViewId = "overview" | "models" | "flows" | "skills" | "access" | "activity" | "settings";
export type ApprovalDecision = "approve" | "deny";
export type BridgeMode = "native" | "fixture";

export interface CommandFence {
  projectHandle: string;
  generation: string;
  operationId: string;
}

export interface FencedResponse<T> {
  fence: CommandFence;
  data: T;
}

export interface ProjectSummaryDto {
  handle: string;
  name: string;
  location: string;
}

export interface CatalogDto {
  projects: ProjectSummaryDto[];
  warning: string | null;
}

export type HealthDto =
  | { status: "healthy"; daemonVersion: string; queueDepth: number }
  | { status: "offline" }
  | { status: "degraded"; detail: string; recovery: string | null };

export interface FailureDto {
  kind: "blocked" | "unavailable";
  code: string | null;
  detail: string;
  recovery: string | null;
}

export type AccessConfigDto =
  | {
      status: "available";
      truth: string;
      platformRootsEnabled: boolean;
      systemProxyDiscoveryEnabled: boolean;
      proxyEnvironment: string;
      noProxy: string;
      pac: string;
    }
  | { status: "blocked"; failure: FailureDto; approvalId: string | null; expiresAtMs: number | null }
  | { status: "unavailable"; failure: FailureDto };

export interface RequestSummaryDto {
  requestId: string;
  operationKind: string;
  state: string;
  queueSequence: number;
  acceptedAtMs: number;
  completedAtMs: number | null;
}

export interface TimelineFactDto {
  kind: "request" | "evidence" | "change" | "verification" | "failure";
  label: string;
  summary: string;
  verified: boolean;
  evidence: string[];
}

export interface OutcomeSectionDto {
  label: string;
  summary: string;
  satisfied: boolean;
}

export interface OutcomeDto {
  heading: string;
  solved: boolean;
  sections: OutcomeSectionDto[];
  evidence: string[];
  evidenceTruncated: boolean;
}

export interface RunDto {
  request: RequestSummaryDto;
  timeline: TimelineFactDto[];
  outcome: OutcomeDto | null;
  detailError: string | null;
}

export type CurrentDto =
  | { status: "available"; queued: RequestSummaryDto[]; truncated: boolean; run: RunDto | null }
  | { status: "approval_required"; approval: string; expiresAtMs: number }
  | { status: "blocked"; failure: FailureDto }
  | { status: "unavailable"; failure: FailureDto };

export interface SnapshotDataDto {
  project: ProjectSummaryDto;
  health: HealthDto;
  current: CurrentDto;
  access: AccessConfigDto;
  catalogWarning: string | null;
}

export type SnapshotDto = FencedResponse<SnapshotDataDto>;

// The bootstrap result: the discovered catalog plus an activated project
// snapshot, or no snapshot at all in global-only mode (empty catalog).
export interface BootstrapResponse {
  catalog: CatalogDto;
  snapshot: SnapshotDto | null;
}

export interface ApprovalDecisionResponseDto {
  disposition: "approved" | "denied" | "expired";
  snapshot: SnapshotDto;
}

export interface EvidenceDataDto {
  handle: string;
  digest: string;
  sizeBytes: number;
  mediaType: string;
  body: string | null;
  truncated: boolean;
  truth: string;
}

export type EvidenceDto = FencedResponse<EvidenceDataDto>;

export interface FlowIdentityDto {
  fileName: string;
  id: string;
  revision: number;
  digest: string;
}

export interface FlowDefinitionDto {
  handle: string;
  identity: FlowIdentityDto;
}

export interface FlowWorkspaceDataDto {
  definitions: FlowDefinitionDto[];
  /** Definition IDs copied into the global library from a legacy project
   * `.pam/flows` catalog during this load. Empty once migrated. */
  migrated: string[];
}

export type FlowWorkspaceDto = FencedResponse<FlowWorkspaceDataDto>;

export interface FlowDocumentDataDto {
  handle: string;
  identity: FlowIdentityDto | null;
  source: string;
}

export type FlowDocumentDto = FencedResponse<FlowDocumentDataDto>;

export interface FlowDryRunStepDto {
  index: number;
  id: string;
  semanticRole: string;
  condition: string;
  approval: string;
  effect: string;
  maxAttempts: number;
  initialBackoffMs: number;
  maxBackoffMs: number;
  action: string;
  daemonAuthority: string;
}

export interface FlowDryRunDto {
  daemonDefinitionEligible: boolean;
  steps: FlowDryRunStepDto[];
}

export interface FlowVersionDiffLineDto {
  kind: string;
  text: string;
}

export interface FlowVersionDiffDto {
  changed: boolean;
  truncated: boolean;
  lines: FlowVersionDiffLineDto[];
}

export interface FlowReviewDataDto {
  document: string;
  identity: FlowIdentityDto;
  normalizedToml: string;
  dryRun: FlowDryRunDto;
  diff: FlowVersionDiffDto;
}

export type FlowReviewDto = FencedResponse<FlowReviewDataDto>;

export interface FlowSaveDataDto {
  document: string;
  identity: FlowIdentityDto;
  created: boolean;
  durabilityConfirmed: boolean;
  cleanupComplete: boolean;
}

export type FlowSaveDto = FencedResponse<FlowSaveDataDto>;

export interface FlowRunDataDto {
  runId: string;
  definitionId: string;
  /** The project this run is bound to. A run has a project; the global flow
   * library it came from does not. */
  projectLabel: string;
  retryCommand: string;
}

export type FlowRunDto = FencedResponse<FlowRunDataDto>;

export interface FlowRunProgressDataDto {
  runId: string;
  /** The replay cursor to send as `after` on the next poll. */
  cursor: number;
  facts: TimelineFactDto[];
  truncated: boolean;
  terminal: boolean;
  outcome: OutcomeDto | null;
  detailError: string | null;
}

export type FlowRunProgressDto = FencedResponse<FlowRunProgressDataDto>;

export interface FlowRunCancelDataDto {
  runId: string;
  disposition: string;
}

export type FlowRunCancelDto = FencedResponse<FlowRunCancelDataDto>;

export interface FlowRunHistoryEntryDto {
  runId: string;
  /** The catalog definition this run still matches, or null once the
   * definition has been edited away. */
  definitionId: string | null;
  projectLabel: string;
  state: string;
  outcome: string | null;
  startedAtMs: number;
  completedAtMs: number | null;
}

export interface FlowRunHistoryDataDto {
  runs: FlowRunHistoryEntryDto[];
  truncated: boolean;
}

export type FlowRunHistoryDto = FencedResponse<FlowRunHistoryDataDto>;

// Mirrors the serde JSON of pam_flow::FlowDefinition exactly (snake_case fields,
// tagged enums via "kind"/"type", snake_case variant values).
export type FlowEffectJson = "read_only" | "stateful";
export type FlowSemanticJson = "observe" | "verify" | "change";
export type FlowApprovalJson = "none" | "required";

export type FlowConditionJson =
  | { kind: "always" }
  | { kind: "succeeded"; step: string }
  | { kind: "failed"; step: string };

export interface FlowRetryJson {
  max_attempts: number;
  initial_backoff_ms: number;
  max_backoff_ms: number;
}

export type FlowActionJson =
  | { type: "command"; program: string; args: string[]; working_directory: string }
  | { type: "connector"; connector: string; capability: string; resource: { kind: string; id: string } };

export interface FlowStepJson {
  id: string;
  description: string;
  depends_on: string[];
  condition: FlowConditionJson;
  retry: FlowRetryJson;
  approval: FlowApprovalJson;
  idempotency_key: string | null;
  timeout_seconds: number;
  effect: FlowEffectJson;
  semantic: FlowSemanticJson | null;
  action: FlowActionJson;
}

export interface FlowOutcomeJson {
  solved: string;
  changed: string;
  verified: string;
  unresolved: string;
  blocked: string;
}

export interface FlowDefinitionJson {
  schema_version: number;
  id: string;
  name: string;
  description: string;
  revision: number;
  steps: FlowStepJson[];
  outcome: FlowOutcomeJson;
}

export type FlowGraphDto =
  | { status: "ok"; definition: FlowDefinitionJson }
  | { status: "invalid"; failure: { detail: string } };

export type FlowComposeDto =
  | { status: "ok"; source: string }
  | { status: "invalid"; failure: { detail: string } };

export interface SkillArtifactDto {
  id: string;
  name: string;
  logicalPath: string;
  kind: string;
  scope: string;
  origin: string;
  loadSemantics: string;
  contentHash: string;
  firstSeenAtMs: number;
  lastChangedAtMs: number;
}

export interface SkillInventoryDriftDto {
  added: number;
  changed: number;
  removed: number;
  resurrected: number;
}

export interface SkillInventoryDataDto {
  artifacts: SkillArtifactDto[];
  total: number;
  truncated: boolean;
  drift: SkillInventoryDriftDto;
  cursorGlobalRulesStatus: "not_locally_discoverable" | "explicitly_configured";
}

export type SkillInventoryDto = FencedResponse<SkillInventoryDataDto>;

export type SkillLibraryAgentDto = "claude" | "codex" | "cursor";
export type SkillLibraryDispositionDto = "inserted" | "already_present";
export type SkillLibraryMaterializationActionDto = "no_op" | "create" | "replace";
export type SkillLibraryCleanupDto =
  | "removed"
  | "missing"
  | "preserved_modified"
  | "preserved_symlink"
  | "preserved_unowned";

export type SkillLibraryInstallationDto =
  | { kind: "local" }
  | { kind: "git"; commit: string };

export type SkillLibraryDriftStateDto =
  | { state: "clean" }
  | { state: "missing" }
  | { state: "modified"; actualDigest: string }
  | {
      state: "conflict";
      reason:
        | "disabled"
        | "unowned"
        | "unsafe_root"
        | "unsafe_path"
        | "symlink"
        | "non_regular"
        | "unreadable"
        | "too_large"
        | "plan_mismatch";
    };

export interface SkillLibraryVersionDto {
  version: string;
  installation: SkillLibraryInstallationDto | null;
  enabledAgents: SkillLibraryAgentDto[];
  managedAgents: SkillLibraryAgentDto[];
}

export interface SkillLibraryEntryDto {
  entryId: string;
  versions: SkillLibraryVersionDto[];
}

export interface SkillLibraryKeyDto {
  entryId: string;
  version: string;
  agent: SkillLibraryAgentDto;
}

export interface SkillLibraryFileMetadataDto {
  byteLen: number;
  digest: string;
}

export interface SkillLibraryPlanItemDto {
  key: SkillLibraryKeyDto;
  action: SkillLibraryMaterializationActionDto;
  existing: SkillLibraryFileMetadataDto | null;
  backupPlanned: boolean;
}

export interface SkillLibraryOutcomeDto {
  key: SkillLibraryKeyDto;
  action: SkillLibraryMaterializationActionDto;
  backup: SkillLibraryFileMetadataDto | null;
  ownershipRecorded: boolean;
}

export interface SkillLibraryDriftDto {
  key: SkillLibraryKeyDto;
  expectedDigest: string;
  state: SkillLibraryDriftStateDto;
}

export type SkillLibraryActionRequest =
  | { action: "load" }
  | { action: "adopt"; entryId: string; artifactId: string }
  | { action: "install_local"; entryId: string; sourcePath: string }
  | { action: "install_git"; entryId: string; url: string; artifactPath: string }
  | ({ action: "enable" | "disable" | "preview_materialization" | "apply_materialization" | "inspect_drift" | "preview_resync" | "apply_resync" } & SkillLibraryKeyDto);

export type SkillLibraryActionResultDto =
  | { schemaVersion: 1; action: "load"; entries: SkillLibraryEntryDto[] }
  | { schemaVersion: 1; action: "adopt"; entryId: string; version: string; artifactId: string; disposition: SkillLibraryDispositionDto }
  | { schemaVersion: 1; action: "install_local" | "install_git"; entryId: string; version: string; disposition: SkillLibraryDispositionDto }
  | { schemaVersion: 1; action: "enable"; key: SkillLibraryKeyDto; enabled: boolean; changed: boolean }
  | { schemaVersion: 1; action: "disable"; key: SkillLibraryKeyDto; stateChanged: boolean; cleanup: SkillLibraryCleanupDto }
  | { schemaVersion: 1; action: "preview_materialization" | "preview_resync"; items: SkillLibraryPlanItemDto[] }
  | { schemaVersion: 1; action: "apply_materialization" | "apply_resync"; outcomes: SkillLibraryOutcomeDto[] }
  | { schemaVersion: 1; action: "inspect_drift"; inspection: SkillLibraryDriftDto };

export type SkillLibraryDto = FencedResponse<SkillLibraryActionResultDto>;

export interface SkillAuditOriginSessionDto {
  origin: string;
  artifactCount: number;
  rawBytes: number;
  estimatedTokens: number;
}

export interface SkillAuditScopeTotalDto {
  scope: string;
  artifactCount: number;
  rawBytes: number;
  estimatedTokens: number;
}

export interface SkillAuditArtifactDto {
  rank: number;
  id: string;
  name: string;
  logicalPath: string;
  kind: string;
  scope: string;
  origin: string;
  loadSemantics: string;
  contentHash: string;
  rawBytes: number;
  estimatedTokens: number;
}

export interface SkillAuditFootprintDto {
  estimator: string;
  alwaysLoadedArtifactCount: number;
  allSessionRawBytes: number;
  allSessionEstimatedTokens: number;
  originSessions: SkillAuditOriginSessionDto[];
  scopeTotals: SkillAuditScopeTotalDto[];
  rankedArtifacts: SkillAuditArtifactDto[];
  rankedArtifactsTotal: number;
  rankedArtifactsTruncated: boolean;
}

export interface SkillAuditMultiArtifactFindingDto {
  artifactIds: string[];
  summary: string;
}

export interface SkillAuditStaleCandidateDto {
  artifactId: string;
  reason: string;
}

export interface SkillAuditVerdictDto {
  overlaps: SkillAuditMultiArtifactFindingDto[];
  conflicts: SkillAuditMultiArtifactFindingDto[];
  staleCandidates: SkillAuditStaleCandidateDto[];
  saturationGrade: "healthy" | "elevated" | "high" | "critical";
  overallSummary: string;
}

export type SkillAuditEvaluatorDto = "claude" | "codex" | "cursor_agent";
export type SkillAuditFailureDto = "invalid_corpus" | "prompt_too_large" | "invocation_failed" | "invalid_verdict";

export type SkillAuditEvaluationDto =
  | { status: "no_evaluator" }
  | { status: "failed"; evaluator: SkillAuditEvaluatorDto; failure: SkillAuditFailureDto }
  | { status: "evaluated"; evaluator: SkillAuditEvaluatorDto; verdict: SkillAuditVerdictDto };

export interface SkillAuditDataDto {
  observedAtMs: number;
  footprint: SkillAuditFootprintDto;
  evaluation: SkillAuditEvaluationDto;
}

export type SkillAuditDto = FencedResponse<SkillAuditDataDto | null>;

export interface BridgeFailureDto {
  code: string;
  detail: string;
  recovery: string | null;
}

export interface ActivityEventDto {
  sequence: number;
  projectId: string | null;
  callerId: string;
  action: string;
  decision: string;
  outcome: string | null;
  occurredAtMs: number;
  projectRoot: string | null;
}

export type ActivityDto =
  | { status: "ok"; events: ActivityEventDto[]; truncated: boolean }
  | { status: "blocked" | "unavailable"; failure: BridgeFailureDto };

export interface DaemonLogEntryDto {
  timestampMs: number;
  severity: string;
  message: string;
}

export type DaemonLogsDto =
  | { status: "ok"; entries: DaemonLogEntryDto[] }
  | { status: "blocked" | "unavailable"; failure: BridgeFailureDto };

export interface ActivityDayDto {
  dayStartMs: number;
  events: number;
}

export interface ProjectUsageDto {
  projectId: string;
  events: number;
  lastEventMs: number;
  root: string | null;
}

export type DaemonStatsDto =
  | { status: "ok"; days: ActivityDayDto[]; projects: ProjectUsageDto[] }
  | { status: "blocked" | "unavailable"; failure: BridgeFailureDto };

export interface ModelSummaryDto {
  modelId: string;
  sizeBytes: number;
}

export interface ModelFailureDto {
  kind: "blocked" | "unavailable";
  code: string | null;
  detail: string;
  recovery: string | null;
}

export type ModelStatusDto =
  | {
      status: "ok";
      loaded: ModelSummaryDto | null;
      registered: ModelSummaryDto[];
      /** Why the running daemon is serving without the model it was started
       * with; null when a model is loaded, none was requested, or the daemon
       * is unreachable. Lasts as long as that daemon does. */
      loadFailure: string | null;
      /** The daemon this GUI started is running but has not answered yet: it
       * is still hashing and mapping its model. A loading daemon cannot say
       * so itself — it only starts accepting once the load is in. */
      loading: boolean;
    }
  | { status: "blocked" | "unavailable"; failure: ModelFailureDto };

export interface ChatMessageDto {
  role: "system" | "user" | "assistant";
  content: string;
}

export interface ModelUsageDto {
  inputTokens: number;
  sampledOutputTokens: number;
  emittedOutputTokens: number;
}

export type ModelInferDto =
  | { status: "ok"; model: string; text: string; finishReason: string; usage: ModelUsageDto }
  | { status: "blocked" | "unavailable"; failure: ModelFailureDto };

export interface ModelImportParams {
  /** Stable model identity in vendor/name form. */
  model: string;
  /** Absolute path to the GGUF file on this machine. */
  path: string;
  licenseId: string;
  licenseUrl: string;
  /** Exact license notice text the user accepted; the backend hashes it. */
  licenseNoticeText: string;
  /** Registers a model below PAM's recommended minimum size anyway. */
  allowSmall: boolean;
}

// Starting an import only acknowledges the background run; completion and
// failure arrive through ModelImportStatusDto polls.
export type ModelImportDto =
  | { status: "ok" }
  | { status: "blocked" | "unavailable"; failure: ModelFailureDto };

// Removing a registration acknowledges exactly the row that left. The GGUF on
// disk is never deleted: unregistering is a registry operation only.
export type ModelUnregisterDto =
  | { status: "ok"; model: string; sizeBytes: number }
  | { status: "blocked" | "unavailable"; failure: ModelFailureDto };

/// One registered model's health against its registration record. `health` is
// never a bare boolean: it names the check that stopped matching, so a row can
// say moved, truncated or drifted rather than just bad.
export interface ModelHealthDto {
  model: string;
  path: string;
  sizeBytes: number;
  health: "ok" | "path_missing" | "size_mismatch" | "digest_mismatch" | "metadata_mismatch" | "unsafe_path" | "unreadable";
  detail: string | null;
  /** `local` for a GGUF PAM verified in place, `https` for one it downloaded. */
  source: string;
  /**
   * True only when PAM downloaded this artifact and it still sits inside the
   * models directory in effect right now. The daemon decides this; the view
   * never re-derives the rule.
   */
  weightsDeletable: boolean;
}

export type ModelVerifyDto =
  | { status: "ok"; models: ModelHealthDto[] }
  | { status: "blocked" | "unavailable"; failure: ModelFailureDto };

export interface DanglingModelDto {
  model: string;
  path: string;
  sizeBytes: number;
}

export interface OrphanWeightsDto {
  path: string;
  sizeBytes: number;
}

// The sweep reports and never acts: a dangling row is cleared by unregistering
// it, and an orphaned file PAM downloaded goes through Delete weights.
export type ModelSweepDto =
  | {
      status: "ok";
      modelsDir: string;
      dangling: DanglingModelDto[];
      orphans: OrphanWeightsDto[];
      totalBytes: number;
    }
  | { status: "blocked" | "unavailable"; failure: ModelFailureDto };

// Deleting weights removes the file and unregisters the model in one
// operation, so the acknowledgement names both the path and the bytes freed.
export type ModelDeleteWeightsDto =
  | { status: "ok"; model: string; path: string; bytesReclaimed: number }
  | { status: "blocked" | "unavailable"; failure: ModelFailureDto };

export interface ModelImportStatusDto {
  status: "idle" | "running" | "complete" | "failed";
  model: string | null;
  /** "hashing" carries live bytes; "registering" is indeterminate. */
  stage: "hashing" | "registering" | null;
  hashedBytes: number;
  totalBytes: number;
  /** Meaningful on "complete": whether the artifact is in PAM's calibrated
   * set. Uncalibrated imports still register, but loading them is untested. */
  calibrated: boolean;
  failure: ModelFailureDto | null;
}

// Hugging Face license discovery for a manual import: an enhancement, so a
// miss is unavailable data and the form falls back to manual entry.
export type ModelLicenseDiscoveryDto =
  | { status: "ok"; repoId: string; licenseId: string }
  | { status: "unavailable"; failure: ModelFailureDto };

export type ModelInspectDto =
  | {
      status: "ok";
      fileName: string;
      sizeBytes: number;
      architecture: string | null;
      modelName: string | null;
      license: string | null;
      belowFloor: boolean;
      floorBytes: number;
    }
  | { status: "blocked" | "unavailable"; failure: ModelFailureDto };

export interface ModelPresetDto {
  id: string;
  label: string;
  model: string;
  fileName: string;
  url: string;
  expectedSizeBytes: number;
  sha256: string;
  licenseId: string;
  licenseUrl: string;
  licenseNoticeText: string;
  /** True when this exact artifact is in PAM's measured, known-good set. */
  calibrated: boolean;
  /**
   * Whether this Mac can run the preset, decided in Rust against the same
   * budget the daemon admits a model with. True when the host memory probe
   * is unavailable.
   */
  fitsHost: boolean;
  paramsLabel: string;
  quantLabel: string;
}

export interface ModelPresetsDto {
  presets: ModelPresetDto[];
  /**
   * The largest artifact this Mac can devote to a model: its runtime ceiling
   * less the projection contingency. Null when the host memory probe failed.
   */
  hostModelBudgetBytes: number | null;
}

export type ModelDownloadDto =
  | { status: "ok" }
  | { status: "blocked" | "unavailable"; failure: ModelFailureDto };

export interface ModelDownloadStatusDto {
  status: "idle" | "running" | "complete" | "failed" | "cancelled";
  presetId: string | null;
  receivedBytes: number;
  totalBytes: number;
  failure: ModelFailureDto | null;
}

/** A daemon start in flight. `modelId` and `phase` are null when no start is
 *  loading a registered model. The verification phase streams the artifact
 *  through a hash, so its resident memory says nothing: it reports
 *  `elapsedSeconds` instead of bytes. */
export interface DaemonStartupProgressDto {
  modelId: string | null;
  phase: "verifying" | "loading" | null;
  loadedBytes: number;
  totalBytes: number;
  elapsedSeconds: number;
}

export interface HostMemoryDto {
  totalBytes: number;
  /** PAM's supported system minimum: local AI needs a 32 GiB machine. */
  supportedMinimumBytes: number;
}

// Settings v1 is global: it never carries a project fence or a failure
// union. `logsDir`/`logsSizeBytes` describe the daemon's real on-disk log
// files; `flowsDir` is the daemon-global flow-definition library the Flows
// view and the CLI open.
export interface AppSettingsDto {
  modelsDir: string;
  modelsDirIsDefault: boolean;
  dataDir: string;
  flowsDir: string;
  logsDir: string;
  logsSizeBytes: number;
}

// Reset is tiered on purpose: every scope is its own capability and its own
// command, so the danger zone can show one blast radius at a time.
export type ResetScope = "access" | "identity" | "history" | "registry" | "factory";

export interface ResetItemDto {
  kind: string;
  count: number;
  bytes: number;
  names: string[];
}

export interface ResetResultDto {
  scope: string;
  dryRun: boolean;
  items: ResetItemDto[];
  totalItems: number;
  totalBytes: number;
}

export type ResetDto =
  | { status: "ok"; result: ResetResultDto; receiptPath: string | null }
  | { status: "blocked"; failure: FailureDto }
  | { status: "unavailable"; failure: FailureDto };

export interface CallerDto {
  callerId: string;
  registeredAtMs: number;
  revokedAtMs: number | null;
  /** Self-declared local caller surface ("cli", "gui", "coding-agent", or
   * "local-application"); null for callers registered before this field existed. */
  kind: string | null;
}

export type CallersDto =
  | { status: "ok"; callers: CallerDto[] }
  | { status: "blocked" | "unavailable"; failure: BridgeFailureDto };

export interface ConnectorSummaryDto {
  connectorId: string;
  enabled: boolean;
  baseUrl: string | null;
  credentialPresent: boolean;
  lastTestStatus: "passed" | "failed" | null;
  lastTestAtMs: number | null;
}

export type ConnectorsDto =
  | { status: "ok"; connectors: ConnectorSummaryDto[] }
  | { status: "blocked" | "unavailable"; failure: ModelFailureDto };

export type ConnectorCredentialAction =
  | { action: "set"; secret: string }
  | { action: "clear" };

export interface ConnectorConfigureParams {
  connector: string;
  enabled?: boolean;
  baseUrl?: string;
  credential?: ConnectorCredentialAction;
}

export type ConnectorConfigureDto =
  | { status: "ok"; connector: ConnectorSummaryDto }
  | { status: "blocked" | "unavailable"; failure: ModelFailureDto };

export type ConnectorTestDto =
  | { status: "ok"; connectorId: string; result: "passed" | "failed"; detail: string }
  | { status: "blocked" | "unavailable"; failure: ModelFailureDto };

export interface DaemonCapabilityDto {
  capability: string;
  name: string;
  summary: string;
  granted: boolean;
}

export interface DaemonAccessDto {
  capabilities: DaemonCapabilityDto[];
}

export interface PamBridge {
  readonly mode: BridgeMode;
  bootstrap(): Promise<BootstrapResponse>;
  catalog(): Promise<CatalogDto>;
  daemonHealth(fence: CommandFence): Promise<HealthDto>;
  daemonActivity(fence: CommandFence, limit?: number): Promise<ActivityDto>;
  daemonLogs(fence: CommandFence, limit?: number): Promise<DaemonLogsDto>;
  daemonStats(fence: CommandFence, days?: number): Promise<DaemonStatsDto>;
  callerRegistry(fence: CommandFence): Promise<CallersDto>;
  daemonAccess(fence: CommandFence): Promise<DaemonAccessDto>;
  daemonAccessConfig(fence: CommandFence): Promise<AccessConfigDto>;
  setDaemonAccess(fence: CommandFence, capability: string, granted: boolean): Promise<DaemonAccessDto>;
  connectorRegistry(fence: CommandFence): Promise<ConnectorsDto>;
  connectorConfigure(fence: CommandFence, params: ConnectorConfigureParams): Promise<ConnectorConfigureDto>;
  connectorTest(fence: CommandFence, connector: string): Promise<ConnectorTestDto>;
  modelStatus(fence: CommandFence): Promise<ModelStatusDto>;
  modelInfer(fence: CommandFence, model: string, messages: ChatMessageDto[], maxOutputTokens?: number): Promise<ModelInferDto>;
  modelImport(fence: CommandFence, params: ModelImportParams): Promise<ModelImportDto>;
  modelUnregister(fence: CommandFence, model: string): Promise<ModelUnregisterDto>;
  /** Registry health: re-reads registered weights. Not the loaded-model check. */
  modelVerify(fence: CommandFence, model?: string): Promise<ModelVerifyDto>;
  modelSweep(fence: CommandFence): Promise<ModelSweepDto>;
  modelDeleteWeights(fence: CommandFence, model: string): Promise<ModelDeleteWeightsDto>;
  modelImportStatus(fence: CommandFence): Promise<ModelImportStatusDto>;
  modelInspect(fence: CommandFence, path: string): Promise<ModelInspectDto>;
  modelLicenseDiscover(fence: CommandFence, query: string): Promise<ModelLicenseDiscoveryDto>;
  modelPresets(fence: CommandFence): Promise<ModelPresetsDto>;
  modelDownload(fence: CommandFence, presetId: string): Promise<ModelDownloadDto>;
  modelDownloadStatus(fence: CommandFence): Promise<ModelDownloadStatusDto>;
  modelDownloadCancel(fence: CommandFence): Promise<ModelDownloadDto>;
  hostMemory(fence: CommandFence): Promise<HostMemoryDto>;
  appSettings(fence: CommandFence): Promise<AppSettingsDto>;
  settingsUpdate(fence: CommandFence, modelsDir: string | null): Promise<AppSettingsDto>;
  logsDelete(fence: CommandFence): Promise<AppSettingsDto>;
  resetAccess(fence: CommandFence, dryRun: boolean): Promise<ResetDto>;
  resetIdentity(fence: CommandFence, dryRun: boolean): Promise<ResetDto>;
  resetHistory(fence: CommandFence, dryRun: boolean): Promise<ResetDto>;
  resetRegistry(fence: CommandFence, dryRun: boolean): Promise<ResetDto>;
  factoryReset(fence: CommandFence, dryRun: boolean, includeWeights: boolean): Promise<ResetDto>;
  revealPath(fence: CommandFence, path: string): Promise<void>;
  activateProject(projectHandle: string, operationId: string): Promise<SnapshotDto>;
  refreshProject(fence: CommandFence): Promise<SnapshotDto>;
  startDaemon(fence: CommandFence, model?: string): Promise<SnapshotDto | null>;
  daemonStartupProgress(fence: CommandFence): Promise<DaemonStartupProgressDto>;
  stopDaemon(fence: CommandFence): Promise<SnapshotDto | null>;
  registerGuiCaller(fence: CommandFence): Promise<SnapshotDto>;
  decideApproval(fence: CommandFence, approvalHandle: string, decision: ApprovalDecision): Promise<ApprovalDecisionResponseDto>;
  loadEvidence(fence: CommandFence, evidenceHandle: string): Promise<EvidenceDto>;
  loadFlowWorkspace(fence: CommandFence): Promise<FlowWorkspaceDto>;
  loadSkillInventory(fence: CommandFence): Promise<SkillInventoryDto>;
  manageSkillLibrary(fence: CommandFence, action: SkillLibraryActionRequest): Promise<SkillLibraryDto>;
  loadSkillAudit(fence: CommandFence): Promise<SkillAuditDto>;
  runSkillAudit(fence: CommandFence): Promise<SkillAuditDto>;
  openFlow(fence: CommandFence, flowHandle: string): Promise<FlowDocumentDto>;
  flowGraph(fence: CommandFence, source: string): Promise<FlowGraphDto>;
  flowCompose(fence: CommandFence, definition: FlowDefinitionJson): Promise<FlowComposeDto>;
  validateFlow(fence: CommandFence, documentHandle: string, source: string): Promise<FlowReviewDto>;
  saveFlow(fence: CommandFence, documentHandle: string, source: string): Promise<FlowSaveDto>;
  flowRun(fence: CommandFence, flowHandle: string): Promise<FlowRunDto>;
  flowRunProgress(fence: CommandFence, run: string, after: number): Promise<FlowRunProgressDto>;
  flowRunCancel(fence: CommandFence, run: string): Promise<FlowRunCancelDto>;
  flowRunHistory(fence: CommandFence): Promise<FlowRunHistoryDto>;
}

export const MAX_EVIDENCE_TEXT = 4_096;
export const MAX_FLOW_SOURCE = 128_000;
export const MAX_CHAT_MESSAGES = 100;
export const CHAT_MAX_OUTPUT_TOKENS = 512;
// The model DTOs do not expose a context size, so the chat drawer sends
// history under this fixed estimated-token budget (reply reserve included).
export const CHAT_CONTEXT_TOKEN_BUDGET = 4_096;
