import type {
  AccessConfigDto,
  ActivityDto,
  ActivityEventDto,
  ApprovalDecision,
  AppSettingsDto,
  CallerDto,
  CallersDto,
  CatalogDto,
  CommandFence,
  ConnectorConfigureDto,
  ConnectorSummaryDto,
  ConnectorTestDto,
  ConnectorsDto,
  DaemonAccessDto,
  DaemonCapabilityDto,
  ActivityDayDto,
  DaemonLogEntryDto,
  DaemonLogsDto,
  DaemonStartupProgressDto,
  DaemonStatsDto,
  EvidenceDataDto,
  ProjectUsageDto,
  FlowDefinitionJson,
  FlowDocumentDataDto,
  FlowReviewDataDto,
  FlowRunDataDto,
  FlowRunHistoryDataDto,
  FlowRunProgressDataDto,
  FlowSaveDataDto,
  FlowWorkspaceDataDto,
  ChatMessageDto,
  HealthDto,
  HostMemoryDto,
  ModelDownloadDto,
  ModelDownloadSource,
  ModelDownloadStatusDto,
  ModelFailureDto,
  ModelImportDto,
  ModelImportStatusDto,
  ModelInspectDto,
  ModelInferDto,
  ModelLicenseDiscoveryDto,
  ModelPresetDto,
  ModelPresetsDto,
  ModelStatusDto,
  ModelSummaryDto,
  ModelDeleteWeightsDto,
  ModelHealthDto,
  ModelLoadDto,
  ModelSweepDto,
  ModelUnloadDto,
  ModelUnregisterDto,
  ModelVerifyDto,
  PamBridge,
  ProjectSummaryDto,
  ResetDto,
  ResetItemDto,
  ResetResultDto,
  SnapshotDataDto,
  SkillAuditDataDto,
  SkillInventoryDataDto,
  SkillLibraryActionResultDto,
  SkillLibraryAgentDto,
  SkillLibraryDriftStateDto,
  SkillLibraryEntryDto,
  SkillLibraryKeyDto,
  SkillLibraryMaterializationActionDto,
  SkillLibraryVersionDto,
} from "./domain";

const projects: ProjectSummaryDto[] = [
  { handle: "11111111-1111-4111-8111-111111111111", name: "payments-api", location: "/work/payments-api" },
  { handle: "22222222-2222-4222-8222-222222222222", name: "ledger-web", location: "/work/ledger-web" },
  { handle: "33333333-3333-4333-8333-333333333333", name: "docs", location: "/work/docs" },
];

// Events carry the daemon's project ID and the root it remembers for it;
// the catalog is matched by root, not by the GUI-local handle. Daemon-only
// projects are never in the catalog, so the feed and the usage panel must
// fall back from the catalog name to the remembered root's basename, and
// only to a truncated ID when the daemon never learned one.
const DAEMON_ONLY_WITH_ROOT = "66666666-6666-4666-8666-666666666666";
const DAEMON_ONLY_ROOTLESS = "77777777-7777-4777-8777-777777777777";

// The daemon's own project IDs, deliberately unequal to the GUI-local catalog
// handles above: an audit event only ever carries these, so the feed can only
// reach a catalog name through the root both sides agree on.
const PAYMENTS_PROJECT_ID = "d951014b-0a3c-4f2e-9a17-2b6c8e5f1d40";
const LEDGER_PROJECT_ID = "4c2b9f70-6d18-4a55-9c31-7e0a2f8b6c94";

const activityEvents: ActivityEventDto[] = [
  { sequence: 4, projectId: PAYMENTS_PROJECT_ID, callerId: "gui:pam-desktop", action: "project.current", decision: "allowed", outcome: "served", occurredAtMs: 1_777_001_520_000, projectRoot: "/work/payments-api" },
  { sequence: 3, projectId: PAYMENTS_PROJECT_ID, callerId: "cli:release-agent", action: "flow.save", decision: "approval_required", outcome: null, occurredAtMs: 1_777_001_460_000, projectRoot: "/work/payments-api" },
  { sequence: 2, projectId: LEDGER_PROJECT_ID, callerId: "cli:release-agent", action: "project.refresh", decision: "allowed", outcome: "served", occurredAtMs: 1_777_001_400_000, projectRoot: "/work/ledger-web" },
  { sequence: 1, projectId: "daemon", callerId: "gui:pam-desktop", action: "daemon.status", decision: "allowed", outcome: "served", occurredAtMs: 1_777_001_340_000, projectRoot: null },
  { sequence: 6, projectId: DAEMON_ONLY_WITH_ROOT, callerId: "cli:daemon-only-rooted", action: "agent.sync", decision: "denied", outcome: null, occurredAtMs: 1_777_001_320_000, projectRoot: "/work/scratch-agent" },
  { sequence: 5, projectId: DAEMON_ONLY_ROOTLESS, callerId: "cli:daemon-only-rootless", action: "agent.deploy", decision: "denied", outcome: null, occurredAtMs: 1_777_001_310_000, projectRoot: null },
  // Production caller IDs are UUIDs, unlike the pretty legacy IDs above; the
  // matching registeredCallers row carries a kind so the UI can label these.
  // Distinct action names from the rows above, so feed assertions elsewhere
  // that match on action text stay unambiguous.
  { sequence: 7, projectId: PAYMENTS_PROJECT_ID, callerId: "8f14e45f-ceea-467e-adc9-15794b520d1d", action: "model.infer", decision: "allowed", outcome: "served", occurredAtMs: 1_777_001_530_000, projectRoot: "/work/payments-api" },
  { sequence: 8, projectId: PAYMENTS_PROJECT_ID, callerId: "3f79bb7b-4a57-4b14-9b3a-9bb6b3b6c56a", action: "skill.audit", decision: "allowed", outcome: "served", occurredAtMs: 1_777_001_540_000, projectRoot: "/work/payments-api" },
];

const daemonLogEntries: DaemonLogEntryDto[] = [
  { timestampMs: 1_777_001_300_000, severity: "info", message: "PAM daemon ready (version fixture-0.1.0, protocol 8)." },
  { timestampMs: 1_777_001_405_000, severity: "warn", message: "rejected malformed client frame: expected identity and body frames, received 3" },
  { timestampMs: 1_777_001_500_000, severity: "error", message: "queued operation failed: store row for request gui-flow-7 vanished mid-lease" },
  { timestampMs: 1_777_001_521_000, severity: "info", message: "request handler completed project.current in 12 ms" },
];

const DAY_MS = 86_400_000;
// Anchored to the wall clock so the overview heatmap demo fills to today. The
// span must cover HEATMAP_DAYS or the demo grid renders empty leading columns
// that look like a rendering fault rather than a short fixture; fixtures.test
// pins the two together. Not imported from the view: bridge.ts imports this
// module, so reaching back into a view would close an import cycle.
const DAEMON_STAT_DAYS = 364;
const statsAnchorDay = Math.floor(Date.now() / DAY_MS) * DAY_MS;
const daemonStatDays: ActivityDayDto[] = Array.from({ length: DAEMON_STAT_DAYS }, (_, index) => ({
  dayStartMs: statsAnchorDay - (DAEMON_STAT_DAYS - 1 - index) * DAY_MS,
  events: (index * 7) % 11 === 0 ? 0 : ((index * 13) % 23) + 1,
})).filter((day) => day.events > 0);

// "docs" is deliberately absent: the catalog still lists it, so the Projects
// panel renders it as a known project with zero usage.
const projectUsage: ProjectUsageDto[] = [
  { projectId: "11111111-1111-4111-8111-111111111111", events: 128, lastEventMs: 1_777_001_520_000, root: null },
  { projectId: "22222222-2222-4222-8222-222222222222", events: 54, lastEventMs: 1_777_001_400_000, root: null },
  { projectId: DAEMON_ONLY_WITH_ROOT, events: 12, lastEventMs: 1_777_001_320_000, root: "/work/scratch-agent" },
  { projectId: DAEMON_ONLY_ROOTLESS, events: 5, lastEventMs: 1_777_001_310_000, root: null },
];

const registeredCallers: CallerDto[] = [
  // Legacy rows registered before `kind` existed: still render with no badge.
  { callerId: "gui:pam-desktop", registeredAtMs: 1_776_900_000_000, revokedAtMs: null, kind: null },
  { callerId: "cli:release-agent", registeredAtMs: 1_776_500_000_000, revokedAtMs: null, kind: null },
  { callerId: "cli:retired-agent", registeredAtMs: 1_775_000_000_000, revokedAtMs: 1_776_800_000_000, kind: null },
  // Production-shaped UUID callers with a declared kind.
  { callerId: "8f14e45f-ceea-467e-adc9-15794b520d1d", registeredAtMs: 1_776_950_000_000, revokedAtMs: null, kind: "gui" },
  { callerId: "3f79bb7b-4a57-4b14-9b3a-9bb6b3b6c56a", registeredAtMs: 1_776_600_000_000, revokedAtMs: null, kind: "cli" },
];

const evidenceHandles = [
  "44444444-4444-4444-8444-444444444444",
  "55555555-5555-4555-8555-555555555555",
];

const flowSource = `schema_version = 2
id = "after-merge-checks"
name = "After merge checks"
description = "Observe the merged revision and verify the worktree."
revision = 4

[outcome]
solved = "Whether every declared check completed successfully."
changed = "This read-only flow does not change project state."
verified = "Whether the tracked worktree matches the index."
unresolved = "Which check still needs investigation."
blocked = "Which policy or workspace boundary stopped the flow."

[[steps]]
id = "observe-revision"
description = "Record the checked-out revision as evidence."
depends_on = []
condition = { kind = "always" }
approval = "none"
timeout_seconds = 30
effect = "read_only"
semantic = "observe"
action = { type = "command", program = "git", args = ["rev-parse", "--verify", "HEAD"], working_directory = "." }

[[steps]]
id = "verify-worktree"
description = "Verify tracked files match the index."
depends_on = ["observe-revision"]
condition = { kind = "succeeded", step = "observe-revision" }
approval = "none"
timeout_seconds = 30
effect = "read_only"
semantic = "verify"
action = { type = "command", program = "git", args = ["diff", "--quiet"], working_directory = "." }
`;

// The fixture flowSource above, expressed as the exact serde JSON of
// pam_flow::FlowDefinition. Exported so tests can assert against the graph.
export const afterMergeDefinition: FlowDefinitionJson = {
  schema_version: 2,
  id: "after-merge-checks",
  name: "After merge checks",
  description: "Observe the merged revision and verify the worktree.",
  revision: 4,
  steps: [
    {
      id: "observe-revision",
      description: "Record the checked-out revision as evidence.",
      depends_on: [],
      condition: { kind: "always" },
      retry: { max_attempts: 1, initial_backoff_ms: 0, max_backoff_ms: 0 },
      approval: "none",
      idempotency_key: null,
      timeout_seconds: 30,
      effect: "read_only",
      semantic: "observe",
      action: { type: "command", program: "git", args: ["rev-parse", "--verify", "HEAD"], working_directory: "." },
    },
    {
      id: "verify-worktree",
      description: "Verify tracked files match the index.",
      depends_on: ["observe-revision"],
      condition: { kind: "succeeded", step: "observe-revision" },
      retry: { max_attempts: 1, initial_backoff_ms: 0, max_backoff_ms: 0 },
      approval: "none",
      idempotency_key: null,
      timeout_seconds: 30,
      effect: "read_only",
      semantic: "verify",
      action: { type: "command", program: "git", args: ["diff", "--quiet"], working_directory: "." },
    },
  ],
  outcome: {
    solved: "Whether every declared check completed successfully.",
    changed: "This read-only flow does not change project state.",
    verified: "Whether the tracked worktree matches the index.",
    unresolved: "Which check still needs investigation.",
    blocked: "Which policy or workspace boundary stopped the flow.",
  },
};

const tomlString = (value: string) => `"${value.replace(/\\/g, "\\\\").replace(/"/g, '\\"')}"`;
const tomlArray = (values: string[]) => `[${values.map(tomlString).join(", ")}]`;

// A deterministic serializer for FlowDefinitionJson, mirroring the inline TOML
// style pam_flow writes. This is not a parser: the fixture graphs only sources
// it has produced (or its own seed document) via a lookup.
function composeFlowToml(definition: FlowDefinitionJson): string {
  const lines = [
    `schema_version = ${definition.schema_version}`,
    `id = ${tomlString(definition.id)}`,
    `name = ${tomlString(definition.name)}`,
    `description = ${tomlString(definition.description)}`,
    `revision = ${definition.revision}`,
    "",
    "[outcome]",
    `solved = ${tomlString(definition.outcome.solved)}`,
    `changed = ${tomlString(definition.outcome.changed)}`,
    `verified = ${tomlString(definition.outcome.verified)}`,
    `unresolved = ${tomlString(definition.outcome.unresolved)}`,
    `blocked = ${tomlString(definition.outcome.blocked)}`,
  ];
  for (const step of definition.steps) {
    const condition = step.condition.kind === "always"
      ? '{ kind = "always" }'
      : `{ kind = ${tomlString(step.condition.kind)}, step = ${tomlString(step.condition.step)} }`;
    const action = step.action.type === "command"
      ? `{ type = "command", program = ${tomlString(step.action.program)}, args = ${tomlArray(step.action.args)}, working_directory = ${tomlString(step.action.working_directory)} }`
      : `{ type = "connector", connector = ${tomlString(step.action.connector)}, capability = ${tomlString(step.action.capability)}, resource = { kind = ${tomlString(step.action.resource.kind)}, id = ${tomlString(step.action.resource.id)} } }`;
    lines.push(
      "",
      "[[steps]]",
      `id = ${tomlString(step.id)}`,
      `description = ${tomlString(step.description)}`,
      `depends_on = ${tomlArray(step.depends_on)}`,
      `condition = ${condition}`,
      `retry = { max_attempts = ${step.retry.max_attempts}, initial_backoff_ms = ${step.retry.initial_backoff_ms}, max_backoff_ms = ${step.retry.max_backoff_ms} }`,
      `approval = ${tomlString(step.approval)}`,
    );
    if (step.idempotency_key !== null) lines.push(`idempotency_key = ${tomlString(step.idempotency_key)}`);
    lines.push(`timeout_seconds = ${step.timeout_seconds}`, `effect = ${tomlString(step.effect)}`);
    if (step.semantic !== null) lines.push(`semantic = ${tomlString(step.semantic)}`);
    lines.push(`action = ${action}`);
  }
  return `${lines.join("\n")}\n`;
}

const normalizeFlowSource = (source: string) => `${source.trimEnd()}\n`;

const definitionHandle = "66666666-6666-4666-8666-666666666666";
const secondDefinitionHandle = "77777777-7777-4777-8777-777777777777";
const documentHandle = "88888888-8888-4888-8888-888888888888";

export const fixtureScenarios = [
  "loading",
  "global-only",
  "offline",
  "missing-credential",
  "approval",
  "queued",
  "empty",
  "active",
  "solved",
  "unresolved",
  "blocked",
  "current-blocked",
  "cancelled",
  "access-available",
  "access-blocked",
  "evidence-loading",
  "evidence-available",
  "evidence-failed",
  "evidence-binary",
  "evidence-truncated",
  "startup-error",
  "skill-audit-empty",
  "skill-audit-no-evaluator",
  "skill-audit-failed",
  "skill-audit-load-error",
  "model-infer-blocked",
  "model-none",
  "model-on-deck",
  "model-loading",
  "model-download-fail",
  "model-unregister-blocked",
  "model-load-blocked",
  "model-health-rotted",
  "model-verify-blocked",
  "connector-unconfigured",
  "connector-blocked",
  "reset-blocked",
] as const;

export type FixtureScenario = typeof fixtureScenarios[number];

export function fixtureScenario(value: string | null | undefined): FixtureScenario {
  return fixtureScenarios.find((scenario) => scenario === value) ?? "solved";
}

const FIXTURE_MODELS_DIR = "/Users/example/llm";
// Provenance decides which row may offer Delete weights: the 14B was
// downloaded by PAM into its models directory, the 4B was imported in place
// from somewhere the user owns and is never PAM's to delete.
const fixtureModelPaths: Record<string, { path: string; source: string }> = {
  "qwen/qwen3-14b-instruct-q4": { path: `${FIXTURE_MODELS_DIR}/qwen/qwen3-14b-instruct-q4.gguf`, source: "https" },
  "qwen/qwen3-4b-instruct-q4": { path: "/Users/example/Downloads/qwen3-4b-instruct-q4.gguf", source: "local" },
};

const loadedModel: ModelSummaryDto = { modelId: "qwen/qwen3-14b-instruct-q4", sizeBytes: 19_500_000_000 };
const registeredModels: ModelSummaryDto[] = [
  loadedModel,
  { modelId: "qwen/qwen3-4b-instruct-q4", sizeBytes: 2_800_000_000 },
];

// A real 32 GB Mac (hw.memsize reports binary GiB) — PAM's supported system
// minimum. Tests that need the below-minimum banner override hostMemory.
const FIXTURE_HOST_MEMORY_BYTES = 34_359_738_368;
const FIXTURE_SUPPORTED_MINIMUM_BYTES = 34_359_738_368;

// The largest artifact a 32 GiB Mac can devote to a model: its 24,696,061,952
// byte runtime ceiling less the 1,234,803,098 byte projection contingency.
// The real value comes from Rust; this mirrors it so the fixture picker tiers
// exactly like the shipped one.
const FIXTURE_HOST_MODEL_BUDGET_BYTES = 23_461_258_854;

// The curated catalog, mirrored from `crates/pam_gui/src/model_presets.rs`:
// two coding families tiered by quantization from a 32 GiB Mac to a 128 GiB
// one. Only the three original Qwen quants are calibrated; digests are
// fixture placeholders. `fitsHost` is computed against the fixture host, the
// way the Rust command computes it against the real one.
const modelPresets: ModelPresetDto[] = (
  [
    ["qwen3-coder-30b-q4ks", "Qwen3 Coder 30B — minimum", "qwen", "Qwen3-Coder-30B-A3B-Instruct-GGUF", "Qwen3-Coder-30B-A3B-Instruct", "30B-A3B", "Q4_K_S", 17_456_012_448, true],
    ["qwen3-coder-30b-q4km", "Qwen3 Coder 30B — balanced", "qwen", "Qwen3-Coder-30B-A3B-Instruct-GGUF", "Qwen3-Coder-30B-A3B-Instruct", "30B-A3B", "Q4_K_M", 18_556_689_568, true],
    ["qwen3-coder-30b-q5km", "Qwen3 Coder 30B — refined", "qwen", "Qwen3-Coder-30B-A3B-Instruct-GGUF", "Qwen3-Coder-30B-A3B-Instruct", "30B-A3B", "Q5_K_M", 21_725_584_544, false],
    ["qwen3-coder-30b-q6k", "Qwen3 Coder 30B — high fidelity", "qwen", "Qwen3-Coder-30B-A3B-Instruct-GGUF", "Qwen3-Coder-30B-A3B-Instruct", "30B-A3B", "Q6_K", 25_092_535_456, true],
    ["qwen3-coder-30b-q80", "Qwen3 Coder 30B — maximum fidelity", "qwen", "Qwen3-Coder-30B-A3B-Instruct-GGUF", "Qwen3-Coder-30B-A3B-Instruct", "30B-A3B", "Q8_0", 32_483_935_392, false],
    ["devstral-small-2-24b-q4km", "Devstral Small 2 24B — balanced", "mistral", "Devstral-Small-2-24B-Instruct-2512-GGUF", "Devstral-Small-2-24B-Instruct-2512", "24B", "Q4_K_M", 14_334_446_752, false],
    ["devstral-small-2-24b-q5km", "Devstral Small 2 24B — refined", "mistral", "Devstral-Small-2-24B-Instruct-2512-GGUF", "Devstral-Small-2-24B-Instruct-2512", "24B", "Q5_K_M", 16_764_521_632, false],
    ["devstral-small-2-24b-q6k", "Devstral Small 2 24B — high fidelity", "mistral", "Devstral-Small-2-24B-Instruct-2512-GGUF", "Devstral-Small-2-24B-Instruct-2512", "24B", "Q6_K", 19_346_476_192, false],
    ["devstral-small-2-24b-q80", "Devstral Small 2 24B — maximum fidelity", "mistral", "Devstral-Small-2-24B-Instruct-2512-GGUF", "Devstral-Small-2-24B-Instruct-2512", "24B", "Q8_0", 25_055_317_152, false],
    ["devstral-small-2-24b-bf16", "Devstral Small 2 24B — full precision", "mistral", "Devstral-Small-2-24B-Instruct-2512-GGUF", "Devstral-Small-2-24B-Instruct-2512", "24B", "BF16", 47_154_056_032, false],
    ["gpt-oss-120b-f16", "GPT-OSS 120B — full precision", "openai", "gpt-oss-120b-GGUF", "gpt-oss-120b", "120B", "F16", 65_369_017_728, false],
  ] as const
).map(([id, label, vendor, repo, stem, paramsLabel, quantLabel, expectedSizeBytes, calibrated]) => {
  const fileName = `${stem}-${quantLabel}.gguf`;
  return {
    id,
    label,
    model: `${vendor}/${stem.toLowerCase()}-${quantLabel.toLowerCase()}`,
    fileName,
    url: `https://huggingface.co/unsloth/${repo}/resolve/main/${fileName}`,
    expectedSizeBytes,
    sha256: `sha256:fixture-${id}`,
    licenseId: "Apache-2.0",
    licenseUrl: "https://www.apache.org/licenses/LICENSE-2.0",
    licenseNoticeText: `${fileName} is distributed under the Apache-2.0 license at https://www.apache.org/licenses/LICENSE-2.0.`,
    calibrated,
    fitsHost: expectedSizeBytes <= FIXTURE_HOST_MODEL_BUDGET_BYTES,
    paramsLabel,
    quantLabel,
  };
});

// Settings v1 is global, so these locations never depend on the scenario's
// active project.
const FIXTURE_DEFAULT_MODELS_DIR = "/Users/fixture/llm";
const FIXTURE_DATA_DIR = "/Users/fixture/Library/Application Support/dev.pam.pam";
const FIXTURE_FLOWS_DIR = `${FIXTURE_DATA_DIR}/.pam/flows`;
const FIXTURE_LOGS_DIR = `${FIXTURE_DATA_DIR}/logs`;
const FIXTURE_LOGS_SIZE_BYTES = 842_318;

const estimateTokens = (text: string) => Math.max(1, Math.ceil(text.length / 4));

const unavailableFailure = {
  kind: "unavailable" as const,
  code: null,
  detail: "The authenticated daemon is unavailable for this project.",
  recovery: "Start PAM, then retry the authenticated project refresh.",
};

function solvedSnapshot(project: ProjectSummaryDto, daemonRunning: boolean): SnapshotDataDto {
  const data: SnapshotDataDto = {
    project,
    health: daemonRunning
      ? { status: "healthy", daemonVersion: "fixture-0.1.0", queueDepth: 2 }
      : { status: "offline" },
    current: {
      status: "available",
      queued: [
        { requestId: "fixture-request-2", operationKind: "after-merge-checks", state: "queued", queueSequence: 2, acceptedAtMs: 1_777_000_000_000, completedAtMs: null },
        { requestId: "fixture-request-3", operationKind: "staging-smoke", state: "queued", queueSequence: 3, acceptedAtMs: 1_777_000_060_000, completedAtMs: null },
      ],
      truncated: false,
      run: {
        request: { requestId: "fixture-request-1", operationKind: "merge-repair", state: "succeeded", queueSequence: 1, acceptedAtMs: 1_777_000_000_000, completedAtMs: 1_777_001_440_000 },
        detailError: null,
        timeline: [
          { kind: "request", label: "Request received", summary: "Investigate failing merge in PR #1842", verified: false, evidence: [] },
          { kind: "evidence", label: "Evidence found", summary: "CI failure and merge base identified", verified: false, evidence: [evidenceHandles[0]] },
          { kind: "change", label: "Fix applied", summary: "Resolved conflicting idempotency logic", verified: false, evidence: [evidenceHandles[1]] },
          { kind: "verification", label: "Verification passed", summary: "All checks green on PR #1842", verified: true, evidence: evidenceHandles },
        ],
        outcome: {
          heading: "Ready for the next agent",
          solved: true,
          sections: [
            { label: "SOLVED", summary: "The merge conflict was repaired and the original request completed.", satisfied: true },
            { label: "CHANGED", summary: "Conflicting idempotency logic was consolidated in the service layer.", satisfied: true },
            { label: "VERIFIED", summary: "Unit and integration checks completed successfully.", satisfied: true },
            { label: "UNRESOLVED", summary: "No unresolved work was reported.", satisfied: false },
            { label: "BLOCKED", summary: "No blocker was reported.", satisfied: false },
          ],
          evidence: evidenceHandles,
          evidenceTruncated: false,
        },
      },
    },
    access: {
      status: "available",
      truth: "System trust and proxy discovery are available to the active project.",
      platformRootsEnabled: true,
      systemProxyDiscoveryEnabled: true,
      proxyEnvironment: "not configured",
      noProxy: "configured",
      pac: "not detected",
    },
    catalogWarning: null,
  };

  if (!daemonRunning) {
    data.current = { status: "unavailable", failure: unavailableFailure };
    data.access = { status: "unavailable", failure: unavailableFailure };
  }

  return data;
}

// The daemon authority sees only what lives outside any project root.
function skillInventory(empty: boolean, global: boolean): SkillInventoryDataDto {
  const artifacts = empty
    ? []
    : global
    ? [
        {
          id: "artifact:sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
          name: "Global review checklist",
          logicalPath: "~/.claude/skills/global-review/SKILL.md",
          kind: "skill",
          scope: "user",
          origin: "claude_code",
          loadSemantics: "model_selected",
          contentHash: "sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
          firstSeenAtMs: 1_777_000_000_000,
          lastChangedAtMs: 1_777_000_000_000,
        },
      ]
    : [
        {
          id: "artifact:sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
          name: "Review changes",
          logicalPath: ".claude/skills/review/SKILL.md",
          kind: "skill",
          scope: "project",
          origin: "claude_code",
          loadSemantics: "model_selected",
          contentHash: "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
          firstSeenAtMs: 1_777_000_000_000,
          lastChangedAtMs: 1_777_000_000_000,
        },
        {
          id: "artifact:sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
          name: "Project instructions",
          logicalPath: "AGENTS.md",
          kind: "instruction",
          scope: "project",
          origin: "codex",
          loadSemantics: "always",
          contentHash: "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
          firstSeenAtMs: 1_777_000_000_000,
          lastChangedAtMs: 1_777_000_000_000,
        },
      ];
  return {
    artifacts,
    total: artifacts.length,
    truncated: false,
    drift: { added: artifacts.length, changed: 0, removed: 0, resurrected: 0 },
    cursorGlobalRulesStatus: "not_locally_discoverable",
  };
}

function skillAudit(evaluation: SkillAuditDataDto["evaluation"] = {
  status: "evaluated",
  evaluator: "codex",
  verdict: {
    saturationGrade: "elevated",
    overallSummary: "The always-loaded footprint is usable, with one overlapping review pair and one stale candidate to inspect.",
    overlaps: [{
      artifactIds: [
        "artifact:sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "artifact:sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
      ],
      summary: "Two review instructions cover the same change-verification responsibility.",
    }],
    conflicts: [{
      artifactIds: [
        "artifact:sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
        "artifact:sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
      ],
      summary: "The project instructions and review skill disagree about when local checks may be skipped.",
    }],
    staleCandidates: [{
      artifactId: "artifact:sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
      reason: "This review skill references a command no longer present in the project.",
    }],
  },
}): SkillAuditDataDto {
  return {
    observedAtMs: 1_777_001_800_000,
    footprint: {
      estimator: "raw_bytes_div_4_ceil_v1",
      alwaysLoadedArtifactCount: 2,
      allSessionRawBytes: 14_336,
      allSessionEstimatedTokens: 3_584,
      originSessions: [
        { origin: "codex", artifactCount: 1, rawBytes: 8_192, estimatedTokens: 2_048 },
        { origin: "claude_code", artifactCount: 1, rawBytes: 6_144, estimatedTokens: 1_536 },
      ],
      scopeTotals: [
        { scope: "project", artifactCount: 2, rawBytes: 14_336, estimatedTokens: 3_584 },
      ],
      rankedArtifacts: [
        {
          rank: 1,
          id: "artifact:sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
          name: "Project instructions",
          logicalPath: "AGENTS.md",
          kind: "instruction",
          scope: "project",
          origin: "codex",
          loadSemantics: "always",
          contentHash: "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
          rawBytes: 8_192,
          estimatedTokens: 2_048,
        },
        {
          rank: 2,
          id: "artifact:sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
          name: "Review changes",
          logicalPath: ".claude/skills/review/SKILL.md",
          kind: "skill",
          scope: "project",
          origin: "claude_code",
          loadSemantics: "always",
          contentHash: "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
          rawBytes: 6_144,
          estimatedTokens: 1_536,
        },
      ],
      rankedArtifactsTotal: 2,
      rankedArtifactsTruncated: false,
    },
    evaluation,
  };
}

function skillLibraryFixture(): SkillLibraryEntryDto[] {
  return [
    {
      entryId: "release-confidence",
      versions: [{
        version: "sha256:1111111111111111111111111111111111111111111111111111111111111111",
        installation: { kind: "local" },
        enabledAgents: ["codex"],
        managedAgents: ["codex"],
      }],
    },
    {
      entryId: "review-changes",
      versions: [{
        version: "sha256:2222222222222222222222222222222222222222222222222222222222222222",
        installation: {
          kind: "git",
          commit: "0123456789abcdef0123456789abcdef01234567",
        },
        enabledAgents: ["claude", "cursor"],
        managedAgents: ["claude", "cursor"],
      }],
    },
  ];
}

function libraryVersion(
  entries: SkillLibraryEntryDto[],
  key: SkillLibraryKeyDto,
): SkillLibraryVersionDto {
  const version = entries
    .find((entry) => entry.entryId === key.entryId)
    ?.versions.find((candidate) => candidate.version === key.version);
  if (!version) throw new Error("The exact fixture library version is unavailable.");
  return version;
}

function setAgent(values: SkillLibraryAgentDto[], agent: SkillLibraryAgentDto, present: boolean) {
  const index = values.indexOf(agent);
  if (present && index === -1) values.push(agent);
  if (!present && index !== -1) values.splice(index, 1);
  values.sort();
}

function fixturePlanAction(managed: boolean, drift: SkillLibraryDriftStateDto): SkillLibraryMaterializationActionDto {
  if (managed && drift.state === "clean") return "no_op";
  if (drift.state === "missing") return "create";
  return "replace";
}

function snapshot(project: ProjectSummaryDto, daemonRunning: boolean, scenario: FixtureScenario): SnapshotDataDto {
  const data = solvedSnapshot(project, daemonRunning);
  if (!daemonRunning || scenario === "solved" || scenario.startsWith("evidence-")) return data;

  if (["unresolved", "blocked", "cancelled"].includes(scenario) && data.current.status === "available" && data.current.run?.outcome) {
    const outcome = data.current.run.outcome;
    outcome.solved = false;
    outcome.heading = scenario === "unresolved"
      ? "Run needs follow-up"
      : scenario === "blocked"
        ? "Run is blocked"
        : "Run was cancelled";
    outcome.sections = outcome.sections.map((section) => ({
      ...section,
      satisfied: section.label === "CHANGED"
        || (scenario === "unresolved" && section.label === "UNRESOLVED")
        || (scenario === "blocked" && section.label === "BLOCKED"),
      summary: section.label === "UNRESOLVED" && scenario === "unresolved"
        ? "The staging verification still needs investigation."
        : section.label === "BLOCKED" && scenario === "blocked"
          ? "Project policy blocked the declared write effect."
          : section.summary,
    }));
    data.current.run.timeline[data.current.run.timeline.length - 1] = {
      kind: "failure",
      label: scenario === "unresolved" ? "Unresolved" : scenario === "blocked" ? "Blocked" : "Run cancelled",
      summary: outcome.heading,
      verified: false,
      evidence: [],
    };
    return data;
  }

  if (scenario === "missing-credential") {
    const detail = "PAM has no native caller credential for this caller.";
    const recovery = "Use Register GUI caller in PAM.";
    const failure = { kind: "unavailable" as const, code: "gui_registration_required", detail, recovery };
    data.health = { status: "degraded", detail, recovery };
    data.current = { status: "unavailable", failure };
    data.access = { status: "unavailable", failure };
  }
  if (scenario === "offline") {
    // The daemon can be started from a paused fixture (e.g. "Start PAM with
    // this model"); snapshots then report the now-running daemon.
    return solvedSnapshot(project, daemonRunning);
  }
  if (scenario === "approval") {
    data.health = { status: "healthy", daemonVersion: "fixture-0.1.0", queueDepth: 0 };
    data.current = {
      status: "approval_required",
      approval: "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
      expiresAtMs: 2_000_000_000_000,
    };
  }
  if (scenario === "queued") {
    data.current = { status: "available", queued: data.current.status === "available" ? data.current.queued : [], truncated: false, run: null };
  }
  if (scenario === "empty") {
    data.current = { status: "available", queued: [], truncated: false, run: null };
  }
  if (scenario === "current-blocked") {
    data.current = {
      status: "blocked",
      failure: {
        kind: "blocked",
        code: "project_current_blocked",
        detail: "Project policy blocked access to the bounded current state.",
        recovery: "Grant project.current for this GUI caller and project, then retry.",
      },
    };
  }
  if (scenario === "active" && data.current.status === "available" && data.current.run) {
    data.current = {
      ...data.current,
      queued: data.current.queued.slice(0, 1),
      run: {
        ...data.current.run,
        request: { ...data.current.run.request, state: "leased", completedAtMs: null },
        timeline: data.current.run.timeline.slice(0, 2),
        outcome: null,
      },
    };
  }
  if (scenario === "access-blocked") {
    data.access = {
      status: "blocked",
      failure: {
        kind: "blocked",
        code: "Forbidden",
        detail: "Network diagnostics are blocked by the selected project's policy.",
        recovery: "Grant network.diagnostics for this GUI caller and project, then retry.",
      },
      approvalId: null,
      expiresAtMs: null,
    };
  }
  return data;
}

/** The pasted-URL refusals the desktop gate produces, in its order, so a
 *  fixture-driven UI meets the same wording the native shell shows. Returns
 *  null when the paste passes every check the browser can make; the address
 *  gate is the desktop's alone. */
function refusePastedDownload(
  source: Extract<ModelDownloadSource, { kind: "url" }>,
): ModelFailureDto | null {
  const refuse = (detail: string, recovery: string): ModelFailureDto => ({
    kind: "unavailable",
    code: "invalid_download_url",
    detail,
    recovery,
  });
  const urlRecovery = "Paste the direct https:// URL of the .gguf file itself.";
  if (!source.accepted) {
    return refuse(
      "PAM records the exact license you accept before it downloads anything.",
      "Accept this model's license, then start the download.",
    );
  }
  if (!/^[^/]+\/[^/]+$/.test(source.model.trim())) {
    return refuse(
      "model identity must use the vendor/name form",
      "Name the model as vendor/name, e.g. qwen/qwen3-4b-instruct-q4.",
    );
  }
  const raw = source.url.trim();
  if (raw === "") return refuse("Enter the model's download URL.", urlRecovery);
  let parsed: URL;
  try {
    parsed = new URL(raw);
  } catch {
    return refuse("PAM could not read that as a URL.", urlRecovery);
  }
  if (parsed.protocol !== "https:") {
    return refuse(
      `PAM downloads models over HTTPS only; this URL uses the ${parsed.protocol.replace(":", "")} scheme.`,
      urlRecovery,
    );
  }
  if (parsed.username !== "" || parsed.password !== "") {
    return refuse(
      "PAM refuses a download URL that carries embedded credentials.",
      "Remove the user:password@ part of the URL and paste it again.",
    );
  }
  if (parsed.search !== "" || parsed.hash !== "") {
    return refuse(
      "PAM refuses a download URL with a query string or fragment; provenance records only the plain URL.",
      "Paste the URL with everything from the ? or # onward removed.",
    );
  }
  if (parsed.port !== "" && parsed.port !== "443") {
    return refuse(
      `PAM downloads models from port 443 only; this URL uses port ${parsed.port}.`,
      "Paste a plain https:// URL with no explicit port.",
    );
  }
  if (!/\/[^/]+\.gguf$/i.test(parsed.pathname)) {
    return refuse("That URL does not end in a .gguf file name PAM can save.", urlRecovery);
  }
  if (!/^(sha256:)?[0-9a-f]{64}$/i.test(source.sha256.trim())) {
    return refuse(
      "The expected digest must be a 64-character hex SHA-256.",
      "Copy the SHA-256 the publisher lists for this exact file.",
    );
  }
  if (source.expectedSizeBytes < 24 || source.expectedSizeBytes > 2 ** 40) {
    return refuse(
      "The expected size must be the file's exact length in bytes.",
      "Copy the byte count the publisher lists for this exact file.",
    );
  }
  if (source.licenseNoticeText.trim() === "") {
    return refuse(
      "PAM records the exact license notice you accept, so it cannot be empty.",
      "Paste the license notice text exactly as the publisher states it.",
    );
  }
  if (source.licenseId.trim() === "" || !source.licenseUrl.trim().startsWith("https://")) {
    return refuse(
      "The license identifier and notice URL are required, and the notice URL must be plain HTTPS.",
      "Fill in the SPDX identifier and the https:// URL of the license notice.",
    );
  }
  return null;
}

function clone<T>(value: T): T {
  return structuredClone(value);
}

export function fixtureBridge(scenario: FixtureScenario = "solved"): PamBridge {
  const catalogProjects = scenario === "global-only" ? [] : projects;
  let active: ProjectSummaryDto | null = catalogProjects[0] ?? null;
  const requireActive = (): ProjectSummaryDto => {
    if (!active) throw new Error("No fixture project is active for this project-scoped command.");
    return active;
  };
  const isDaemonFence = (fence: CommandFence) => fence.projectHandle === "daemon" && fence.generation === "daemon";
  let generation = "99999999-9999-4999-8999-999999999999";
  let daemonRunning = scenario !== "offline";
  // The registered catalog and the loaded slot depend on the model scenario;
  // startDaemon(model) moves a registered model into the loaded slot.
  let modelCatalog: ModelSummaryDto[] =
    scenario === "model-none" || scenario === "model-download-fail" ? [] : [...registeredModels];
  let modelLoaded: ModelSummaryDto | null =
    scenario === "model-none" || scenario === "model-on-deck" || scenario === "model-download-fail"
      ? null
      : loadedModel;
  // A start in flight publishes its child's resident memory against the
  // registered artifact size; each poll advances it like the desktop's
  // sampler does, first through the flat verification phase and then up the
  // mapping ramp.
  let startupElapsedSeconds = 0;
  // One download at a time, tracked globally like the real daemon; each poll
  // advances it a fixed step so a fixture-driven UI shows real progress.
  let download: {
    downloadId: string;
    downloadKind: "preset" | "url";
    model: string;
    receivedBytes: number;
    totalBytes: number;
    status: "running" | "complete" | "failed" | "cancelled";
    failure: ModelFailureDto | null;
  } | null = null;
  // "model-download-fail" fails the first attempt (mirroring a dropped
  // connection) so a retry can then be driven through to a real completion.
  let downloadAttempts = 0;
  // One import at a time, tracked globally like the real import manager;
  // each poll advances the hash a fixed step, then spends one poll in the
  // indeterminate registering stage before completing.
  let importRun: {
    model: string;
    hashedBytes: number;
    totalBytes: number;
    stage: "hashing" | "registering";
    status: "running" | "complete" | "failed";
    calibrated: boolean;
    failure: ModelFailureDto | null;
  } | null = null;
  let modelsDir = FIXTURE_DEFAULT_MODELS_DIR;
  // The persisted pin a daemon start loads when the GUI names no model.
  let defaultModel: string | null = null;
  let logsSizeBytes = FIXTURE_LOGS_SIZE_BYTES;
  // Reset state the fixture actually clears, so a dry run and the run that
  // follows it report the same counts exactly once.
  const resetState: Record<string, ResetItemDto[]> = {
    access: [
      { kind: "grants", count: 6, bytes: 0, names: [] },
      { kind: "approvals", count: 2, bytes: 0, names: [] },
      { kind: "flow_authorizations", count: 1, bytes: 0, names: [] },
    ],
    identity: [
      { kind: "callers", count: 2, bytes: 0, names: [] },
      { kind: "caller_files", count: 3, bytes: 192, names: [] },
      { kind: "keychain_entries", count: 2, bytes: 0, names: [] },
    ],
    history: [
      { kind: "audit_events", count: 418, bytes: 61_440, names: [] },
      { kind: "evidence", count: 37, bytes: 5_242_880, names: [] },
      { kind: "flow_runs", count: 9, bytes: 131_072, names: [] },
    ],
    registry: [{ kind: "models", count: 2, bytes: 0, names: [] }],
  };
  const factoryExtras: ResetItemDto[] = [
    {
      kind: "flows",
      count: 2,
      bytes: 4_096,
      names: ["release-readiness.toml", "worktree-triage.toml"],
    },
    { kind: "settings", count: 1, bytes: 64, names: [] },
    { kind: "logs", count: 3, bytes: FIXTURE_LOGS_SIZE_BYTES, names: [] },
    { kind: "runtime", count: 2, bytes: 128, names: [] },
    { kind: "state_database", count: 1, bytes: 2_097_152, names: [] },
    { kind: "other_data_files", count: 0, bytes: 0, names: [] },
  ];
  const resetResult = (scope: string, items: ResetItemDto[], dryRun: boolean): ResetResultDto => ({
    scope,
    dryRun,
    items: clone(items),
    totalItems: items.reduce((total, item) => total + item.count, 0),
    totalBytes: items.reduce((total, item) => total + item.bytes, 0),
  });
  const runReset = (scope: keyof typeof resetState, dryRun: boolean): ResetDto => {
    if (scenario === "reset-blocked") {
      return {
        status: "blocked",
        failure: {
          kind: "blocked",
          code: "forbidden",
          detail: "project policy denied this capability",
          recovery: `pam access grant reset.${scope} --daemon --resource reset:${scope}:mode=apply`,
        },
      };
    }
    const items = resetState[scope];
    const result = resetResult(scope, items, dryRun);
    if (!dryRun) {
      resetState[scope] = items.map((item) => ({ ...item, count: 0, bytes: 0, names: [] }));
    }
    return { status: "ok", result, receiptPath: null };
  };
  const appSettingsSnapshot = (): AppSettingsDto => ({
    modelsDir,
    modelsDirIsDefault: modelsDir === FIXTURE_DEFAULT_MODELS_DIR,
    defaultModel,
    dataDir: FIXTURE_DATA_DIR,
    flowsDir: FIXTURE_FLOWS_DIR,
    logsDir: FIXTURE_LOGS_DIR,
    logsSizeBytes,
  });
  let savedSource = flowSource;
  const connectors: ConnectorSummaryDto[] = [
    scenario === "connector-unconfigured" || scenario === "connector-blocked"
      ? { connectorId: "github-actions", enabled: false, baseUrl: null, credentialPresent: false, lastTestStatus: null, lastTestAtMs: null }
      : { connectorId: "github-actions", enabled: true, baseUrl: "https://api.github.com", credentialPresent: true, lastTestStatus: "passed", lastTestAtMs: 1_777_001_100_000 },
  ];
  // Daemon-scope grants are durable owner decisions, so the fixture starts
  // ungranted and only the Access view's own action flips a row.
  const daemonCapabilities: DaemonCapabilityDto[] = [
    { capability: "model.infer", name: "Model inference", summary: "Chat and the Models view model check ask the loaded model to generate.", granted: scenario !== "model-infer-blocked" },
    { capability: "model.register", name: "Model registration", summary: "Models registers an imported or downloaded GGUF in the daemon's registry.", granted: true },
    { capability: "model.load", name: "Model loading", summary: "Models brings a registered model into the running daemon, replacing whatever it was serving.", granted: scenario !== "model-load-blocked" },
    { capability: "model.unload", name: "Model unloading", summary: "Models drops the loaded model and frees its memory; PAM keeps serving.", granted: scenario !== "model-load-blocked" },
    { capability: "model.unregister", name: "Model removal", summary: "Models removes a registered model from the daemon's registry; the weights stay on disk.", granted: scenario !== "model-unregister-blocked" },
    { capability: "model.verify", name: "Model verification", summary: "Models re-reads registered weights and reports what no longer matches the registry.", granted: scenario !== "model-verify-blocked" },
    { capability: "model.sweep", name: "Model directory sweep", summary: "Models reconciles the registry against the models directory and reports what it costs.", granted: scenario !== "model-verify-blocked" },
    { capability: "model.delete-weights", name: "Model weights deletion", summary: "Models deletes a GGUF PAM downloaded into its own models directory and unregisters it.", granted: true },
    { capability: "network.diagnostics", name: "Access boundary read", summary: "Access reads the daemon's observed TLS roots, proxy environment, and PAC state.", granted: true },
    { capability: "connector.configure", name: "Connector configuration", summary: "Access saves a connector's enablement, base URL, and credential.", granted: scenario !== "connector-blocked" },
    { capability: "connector.test", name: "Connector self-test", summary: "Access runs a connector's self-test against its configured host.", granted: scenario !== "connector-blocked" },
  ];
  const flowGraphSources = new Map<string, FlowDefinitionJson>([
    [normalizeFlowSource(flowSource), afterMergeDefinition],
  ]);
  const libraryEntries = skillLibraryFixture();
  const libraryDrift = new Map<string, SkillLibraryDriftStateDto>([
    ["release-confidence:sha256:1111111111111111111111111111111111111111111111111111111111111111:codex", { state: "missing" }],
    ["review-changes:sha256:2222222222222222222222222222222222222222222222222222222222222222:claude", { state: "clean" }],
    ["review-changes:sha256:2222222222222222222222222222222222222222222222222222222222222222:cursor", {
      state: "modified",
      actualDigest: "sha256:3333333333333333333333333333333333333333333333333333333333333333",
    }],
  ]);
  const driftKey = (key: SkillLibraryKeyDto) => `${key.entryId}:${key.version}:${key.agent}`;
  const fenceResponse = <T,>(fence: CommandFence, data: T) => ({ fence: clone(fence), data: clone(data) });
  const currentFence = (operationId: string): CommandFence => ({ projectHandle: requireActive().handle, generation, operationId });
  // Snapshot commands rotate the generation, exactly like the desktop core.
  let generationCounter = 0;
  const rotatedFence = (operationId: string): CommandFence => {
    generationCounter += 1;
    generation = `99999999-9999-4999-8999-${String(generationCounter).padStart(12, "0")}`;
    return currentFence(operationId);
  };
  const identity = { fileName: "after-merge-checks.toml", id: "after-merge-checks", revision: 4, digest: "sha256:fixture-after-merge" };
  const workspace = (): FlowWorkspaceDataDto => ({
    definitions: [
      { handle: definitionHandle, identity },
      { handle: secondDefinitionHandle, identity: { fileName: "release-confidence.toml", id: "release-confidence", revision: 3, digest: "sha256:fixture-release" } },
    ],
    migrated: [],
  });
  const document = (): FlowDocumentDataDto => ({ handle: documentHandle, identity, source: savedSource });
  // The demo run is terminal on its first progress read: the browser fixture
  // has no daemon to make progress, and a deterministic window keeps the
  // visual baseline stable.
  let startedRun: FlowRunDataDto | null = null;
  const runHistory: FlowRunHistoryDataDto = {
    runs: [
      { runId: "flow-run-1c6f", definitionId: "after-merge-checks", projectLabel: "/work/payments-api", state: "succeeded", outcome: "solved", startedAtMs: 1_777_001_200_000, completedAtMs: 1_777_001_260_000 },
      { runId: "flow-run-8ba2", definitionId: "release-confidence", projectLabel: "/work/ledger-web", state: "failed", outcome: "blocked", startedAtMs: 1_777_000_800_000, completedAtMs: 1_777_000_845_000 },
      { runId: "flow-run-04d9", definitionId: null, projectLabel: "/work/payments-api", state: "cancelled", outcome: "cancelled", startedAtMs: 1_777_000_300_000, completedAtMs: 1_777_000_330_000 },
    ],
    truncated: false,
  };
  const runProgress = (runId: string): FlowRunProgressDataDto => ({
    runId,
    cursor: 4,
    facts: [
      { kind: "request", label: "Run started", summary: "PAM began the run.", verified: false, evidence: [] },
      { kind: "evidence", label: "Evidence found", summary: "Step observe-revision recorded 1 evidence item(s).", verified: false, evidence: [evidenceHandles[0]] },
      { kind: "verification", label: "Verification passed", summary: "All checks green on PR #1842", verified: true, evidence: [evidenceHandles[1]] },
    ],
    truncated: false,
    terminal: true,
    outcome: {
      heading: "Ready for the next agent",
      solved: true,
      sections: [
        { label: "SOLVED", summary: "The after-merge checks completed against the bound project.", satisfied: true },
        { label: "CHANGED", summary: "This read-only flow does not change project state.", satisfied: false },
        { label: "VERIFIED", summary: "Every declared check reported green.", satisfied: true },
        { label: "UNRESOLVED", summary: "No unresolved work was reported.", satisfied: false },
        { label: "BLOCKED", summary: "No blocker was reported.", satisfied: false },
      ],
      evidence: evidenceHandles,
      evidenceTruncated: false,
    },
    detailError: null,
  });

  return {
    mode: "fixture",
    async bootstrap() {
      if (scenario === "loading") return new Promise(() => {});
      if (scenario === "startup-error") throw new Error("The PAM daemon fixture is unavailable.");
      return {
        catalog: { projects: clone(catalogProjects), warning: null },
        snapshot: active
          ? fenceResponse(currentFence("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"), snapshot(active, daemonRunning, scenario))
          : null,
      };
    },
    async catalog(): Promise<CatalogDto> {
      return { projects: clone(catalogProjects), warning: null };
    },
    async daemonHealth(_fence): Promise<HealthDto> {
      return daemonRunning
        ? { status: "healthy", daemonVersion: "fixture-0.1.0", queueDepth: 2 }
        : { status: "offline" };
    },
    async daemonActivity(_fence, limit): Promise<ActivityDto> {
      if (!daemonRunning) {
        return {
          status: "unavailable",
          failure: {
            code: "daemon_offline",
            detail: "PAM is paused, so no daemon activity is being recorded.",
            recovery: "Start PAM to resume the activity feed.",
          },
        };
      }
      if (scenario === "empty") return { status: "ok", events: [], truncated: false };
      const bounded = activityEvents.slice(0, limit ?? activityEvents.length);
      return clone({ status: "ok" as const, events: bounded, truncated: bounded.length < activityEvents.length });
    },
    async daemonLogs(_fence, limit): Promise<DaemonLogsDto> {
      if (!daemonRunning) {
        return {
          status: "unavailable",
          failure: {
            code: "daemon_offline",
            detail: "PAM is paused, so no daemon diagnostics are being recorded.",
            recovery: "Start PAM to resume the console.",
          },
        };
      }
      if (scenario === "empty") return { status: "ok", entries: [] };
      const bounded = daemonLogEntries.slice(-(limit ?? daemonLogEntries.length));
      return clone({ status: "ok" as const, entries: bounded });
    },
    async daemonStats(_fence, days): Promise<DaemonStatsDto> {
      if (!daemonRunning) {
        return {
          status: "unavailable",
          failure: {
            code: "daemon_offline",
            detail: "PAM is paused, so activity statistics are unavailable.",
            recovery: "Start PAM to see the activity overview.",
          },
        };
      }
      if (scenario === "empty") return { status: "ok", days: [], projects: [] };
      const window = (days && days > 0 ? days : 182) * DAY_MS;
      return clone({
        status: "ok" as const,
        days: daemonStatDays.filter((day) => day.dayStartMs >= statsAnchorDay - window),
        projects: projectUsage,
      });
    },
    async modelStatus(_fence): Promise<ModelStatusDto> {
      // A daemon mid-load answers nothing, so the desktop reports the phase
      // from the child it spawned.
      if (scenario === "model-loading") {
        return clone({ status: "ok" as const, loaded: null, registered: modelCatalog, loadFailure: null, loading: true, transition: null });
      }
      // The registered catalog is durable store state: the desktop answers it
      // even while the daemon is paused, with nothing confirmable as loaded.
      if (!daemonRunning) {
        return clone({ status: "ok" as const, loaded: null, registered: modelCatalog, loadFailure: null, loading: false, transition: null });
      }
      return clone({ status: "ok" as const, loaded: modelLoaded, registered: modelCatalog, loadFailure: null, loading: false, transition: null });
    },
    async modelInfer(_fence, model, messages: ChatMessageDto[]): Promise<ModelInferDto> {
      if (scenario === "model-infer-blocked") {
        return {
          status: "blocked",
          failure: {
            kind: "blocked",
            code: "model_infer_blocked",
            detail: "Project policy has not granted model.infer to this caller yet.",
            recovery: "pam access grant model.infer for this GUI caller and project, then send again.",
          },
        };
      }
      if (!daemonRunning) {
        return {
          status: "unavailable",
          failure: {
            kind: "unavailable",
            code: "daemon_offline",
            detail: "PAM is paused, so the local model cannot answer.",
            recovery: "Start PAM to chat with the local model.",
          },
        };
      }
      const lastUser = [...messages].reverse().find((message) => message.role === "user");
      const text = `You said: ${lastUser?.content ?? "nothing yet"}. The fixture model heard ${messages.length} message${messages.length === 1 ? "" : "s"}.`;
      const outputTokens = estimateTokens(text);
      return {
        status: "ok",
        model,
        text,
        finishReason: "stop",
        usage: {
          inputTokens: messages.reduce((total, message) => total + estimateTokens(message.content), 0),
          sampledOutputTokens: outputTokens,
          emittedOutputTokens: outputTokens,
        },
      };
    },
    async modelLoad(_fence, model): Promise<ModelLoadDto> {
      if (scenario === "model-load-blocked") {
        return {
          status: "blocked",
          failure: {
            kind: "blocked",
            code: "forbidden",
            detail: "Project policy has not granted model.load to this caller yet.",
            recovery: "Grant the GUI caller the model.load capability in Access, or approve the pending load.",
          },
        };
      }
      if (!daemonRunning) {
        return {
          status: "unavailable",
          failure: {
            kind: "unavailable",
            code: "daemon_offline",
            detail: "PAM is paused, so it cannot load a model.",
            recovery: "Start PAM, then load the model.",
          },
        };
      }
      if (modelLoaded?.modelId === model) {
        return { status: "ok", model, sizeBytes: modelLoaded.sizeBytes, previous: null, alreadyLoaded: true };
      }
      const wanted = modelCatalog.find((entry) => entry.modelId === model);
      if (!wanted) {
        return {
          status: "unavailable",
          failure: { kind: "unavailable", code: "not_found", detail: `model ${model} is not registered`, recovery: null },
        };
      }
      // The daemon swaps old-before-new, so the fixture reports both halves.
      const previous = modelLoaded?.modelId ?? null;
      modelLoaded = wanted;
      return { status: "ok", model, sizeBytes: wanted.sizeBytes, previous, alreadyLoaded: false };
    },
    async modelUnload(_fence): Promise<ModelUnloadDto> {
      if (scenario === "model-load-blocked") {
        return {
          status: "blocked",
          failure: {
            kind: "blocked",
            code: "forbidden",
            detail: "Project policy has not granted model.unload to this caller yet.",
            recovery: "Grant the GUI caller the model.unload capability in Access, or approve the pending unload.",
          },
        };
      }
      if (!daemonRunning || !modelLoaded) {
        return {
          status: "unavailable",
          failure: {
            kind: "unavailable",
            code: "not_found",
            detail: "no model is loaded in this daemon",
            recovery: "load one with `pam model load <vendor/name>` before unloading",
          },
        };
      }
      const dropped = modelLoaded;
      modelLoaded = null;
      return { status: "ok", model: dropped.modelId, sizeBytes: dropped.sizeBytes };
    },
    async modelUnregister(_fence, model): Promise<ModelUnregisterDto> {
      if (scenario === "model-unregister-blocked") {
        return {
          status: "blocked",
          failure: {
            kind: "blocked",
            code: "forbidden",
            detail: "Project policy has not granted model.unregister to this caller yet.",
            recovery: "Grant the GUI caller the model.unregister capability in Access, or approve the pending removal.",
          },
        };
      }
      // The daemon maps its model for its whole life and refuses to drop the
      // registration under it; the fixture answers exactly as it does.
      if (daemonRunning && modelLoaded?.modelId === model) {
        return {
          status: "unavailable",
          failure: {
            kind: "unavailable",
            code: null,
            detail: "the requested model is loaded in this daemon and cannot be unregistered",
            recovery: `run \`pam model unload\` (or Unload in the Models view), then unregister ${model}`,
          },
        };
      }
      const removed = modelCatalog.find((entry) => entry.modelId === model);
      if (!removed) {
        return {
          status: "unavailable",
          failure: {
            kind: "unavailable",
            code: "not_found",
            detail: `model ${model} is not registered`,
            recovery: null,
          },
        };
      }
      modelCatalog = modelCatalog.filter((entry) => entry.modelId !== model);
      return { status: "ok", model: removed.modelId, sizeBytes: removed.sizeBytes };
    },
    async modelVerify(_fence, model): Promise<ModelVerifyDto> {
      if (scenario === "model-verify-blocked") {
        return {
          status: "blocked",
          failure: {
            kind: "blocked",
            code: "forbidden",
            detail: "Project policy has not granted model.verify to this caller yet.",
            recovery: "Grant the GUI caller the model.verify capability in Access, or approve the pending check.",
          },
        };
      }
      const rows = modelCatalog
        .filter((entry) => model === undefined || entry.modelId === model)
        .map((entry): ModelHealthDto => {
          const provenance = fixtureModelPaths[entry.modelId] ?? { path: `${FIXTURE_MODELS_DIR}/${entry.modelId}.gguf`, source: "https" };
          // One scenario shows a registry that has rotted: the imported model's
          // file moved out from under it.
          const rotted = scenario === "model-health-rotted" && entry.modelId === "qwen/qwen3-4b-instruct-q4";
          return {
            model: entry.modelId,
            path: provenance.path,
            sizeBytes: entry.sizeBytes,
            health: rotted ? "path_missing" : "ok",
            detail: rotted ? "model storage is unavailable" : null,
            source: provenance.source,
            weightsDeletable: provenance.source === "https" && !rotted,
          };
        });
      return { status: "ok", models: rows };
    },
    async modelSweep(_fence): Promise<ModelSweepDto> {
      if (scenario === "model-verify-blocked") {
        return {
          status: "blocked",
          failure: {
            kind: "blocked",
            code: "forbidden",
            detail: "Project policy has not granted model.sweep to this caller yet.",
            recovery: "Grant the GUI caller the model.sweep capability in Access, or approve the pending sweep.",
          },
        };
      }
      return {
        status: "ok",
        modelsDir: FIXTURE_MODELS_DIR,
        dangling: scenario === "model-health-rotted"
          ? [{
              model: "qwen/qwen3-4b-instruct-q4",
              path: fixtureModelPaths["qwen/qwen3-4b-instruct-q4"].path,
              sizeBytes: 2_800_000_000,
            }]
          : [],
        orphans: scenario === "model-health-rotted"
          ? [{ path: `${FIXTURE_MODELS_DIR}/qwen/orphaned-q4.gguf`, sizeBytes: 4_100_000_000 }]
          : [],
        totalBytes: 23_600_000_000,
      };
    },
    async modelDeleteWeights(_fence, model): Promise<ModelDeleteWeightsDto> {
      const provenance = fixtureModelPaths[model];
      // PAM refuses any artifact it did not download, in the daemon's own
      // words, and says what the user can do instead.
      if (!provenance || provenance.source !== "https") {
        return {
          status: "unavailable",
          failure: {
            kind: "unavailable",
            code: null,
            detail: `PAM did not download this model, so it will not delete the file at ${provenance?.path ?? model}`,
            recovery: `Run \`pam model unregister ${model} --yes\` to drop the registry entry, then delete ${provenance?.path ?? "the file"} yourself.`,
          },
        };
      }
      if (daemonRunning && modelLoaded?.modelId === model) {
        return {
          status: "unavailable",
          failure: {
            kind: "unavailable",
            code: null,
            detail: "the requested model is loaded in this daemon and its weights cannot be deleted",
            recovery: `run \`pam model unload\` (or Unload in the Models view), then delete the weights for ${model}`,
          },
        };
      }
      const removed = modelCatalog.find((entry) => entry.modelId === model);
      if (!removed) {
        return {
          status: "unavailable",
          failure: { kind: "unavailable", code: "not_found", detail: `model ${model} is not registered`, recovery: null },
        };
      }
      modelCatalog = modelCatalog.filter((entry) => entry.modelId !== model);
      return { status: "ok", model, path: provenance.path, bytesReclaimed: removed.sizeBytes };
    },
    async modelImport(_fence, params): Promise<ModelImportDto> {
      if (!daemonRunning) {
        return {
          status: "unavailable",
          failure: {
            kind: "unavailable",
            code: "daemon_offline",
            detail: "PAM is paused, so the model registry is not accepting imports.",
            recovery: "Start PAM, then import the model again.",
          },
        };
      }
      if (!params.path.startsWith("/") || !params.path.endsWith(".gguf")) {
        return {
          status: "unavailable",
          failure: {
            kind: "unavailable",
            code: "model_invalid",
            detail: "The model path must be an absolute path to a GGUF file.",
            recovery: "Pick the downloaded .gguf file and try again.",
          },
        };
      }
      if (importRun?.status === "running") {
        return {
          status: "unavailable",
          failure: {
            kind: "unavailable",
            code: "import_already_running",
            detail: "A model import is already running.",
            recovery: "Wait for the current import to finish, then retry.",
          },
        };
      }
      importRun = {
        model: params.model,
        hashedBytes: 0,
        totalBytes: 4_600_000_000,
        stage: "hashing",
        status: "running",
        calibrated: true,
        failure: null,
      };
      return { status: "ok" };
    },
    async modelImportStatus(_fence): Promise<ModelImportStatusDto> {
      if (!importRun) {
        return { status: "idle", model: null, stage: null, hashedBytes: 0, totalBytes: 0, calibrated: true, failure: null };
      }
      if (importRun.status === "running") {
        if (importRun.stage === "hashing") {
          importRun.hashedBytes = Math.min(
            importRun.totalBytes,
            importRun.hashedBytes + Math.ceil(importRun.totalBytes * 0.4),
          );
          if (importRun.hashedBytes >= importRun.totalBytes) importRun.stage = "registering";
        } else {
          importRun.status = "complete";
          modelCatalog.push({ modelId: importRun.model, sizeBytes: importRun.totalBytes });
        }
      }
      return clone({
        status: importRun.status,
        model: importRun.model,
        stage: importRun.status === "running" ? importRun.stage : null,
        hashedBytes: importRun.hashedBytes,
        totalBytes: importRun.totalBytes,
        calibrated: importRun.calibrated,
        failure: importRun.failure,
      });
    },
    async modelInspect(_fence, path): Promise<ModelInspectDto> {
      const fileName = path.split("/").pop() ?? path;
      if (path.endsWith("tiny.gguf")) {
        return clone({
          status: "ok" as const,
          fileName,
          sizeBytes: 2_800_000_000,
          architecture: null,
          modelName: null,
          license: null,
          belowFloor: true,
          floorBytes: 17_000_000_000,
        });
      }
      if (path.endsWith("community.gguf")) {
        // Declares no license and answers Hugging Face discovery: the
        // narrated auto-prefill flow's fixture path.
        return clone({
          status: "ok" as const,
          fileName,
          sizeBytes: 17_456_012_448,
          architecture: "qwen3moe",
          modelName: "Qwen3-Coder-30B-A3B-Community",
          license: null,
          belowFloor: false,
          floorBytes: 17_000_000_000,
        });
      }
      if (path.endsWith(".gguf")) {
        return clone({
          status: "ok" as const,
          fileName,
          sizeBytes: 17_456_012_448,
          architecture: "qwen3moe",
          modelName: "Qwen3-Coder-30B-A3B-Instruct",
          // Only the dedicated "licensed" fixture path declares a GGUF
          // license, so tests exercising the missing-license-fields flow on
          // an ordinary path are unaffected by license auto-prefill.
          // Lowercase, as real GGUF metadata usually spells it — the form
          // canonicalizes onto the SPDX id.
          license: path.endsWith("licensed.gguf") ? "apache-2.0" : null,
          belowFloor: false,
          floorBytes: 17_000_000_000,
        });
      }
      return clone({
        status: "unavailable" as const,
        failure: {
          kind: "unavailable",
          code: "model_invalid",
          detail: "Point PAM at a downloaded .gguf file.",
          recovery: null,
        },
      });
    },
    async modelLicenseDiscover(_fence, query): Promise<ModelLicenseDiscoveryDto> {
      // Only the dedicated community fixture model resolves, so ordinary
      // paths keep exercising the manual license flow untouched.
      if (query.includes("Community")) {
        return {
          status: "ok",
          repoId: "the-community/Qwen3-Coder-30B-A3B-Community",
          licenseId: "apache-2.0",
        };
      }
      return {
        status: "unavailable",
        failure: { kind: "unavailable", code: "license_discovery_failed", detail: "No matching Hugging Face model declares a license.", recovery: "Fill in the license details under Advanced manually." },
      };
    },
    async modelPresets(_fence): Promise<ModelPresetsDto> {
      return clone({ presets: modelPresets, hostModelBudgetBytes: FIXTURE_HOST_MODEL_BUDGET_BYTES });
    },
    async modelDownload(_fence, source): Promise<ModelDownloadDto> {
      if (!daemonRunning) {
        return {
          status: "unavailable",
          failure: {
            kind: "unavailable",
            code: "daemon_offline",
            detail: "PAM is paused, so it cannot start a download.",
            recovery: "Start PAM, then try the download again.",
          },
        };
      }
      let started: { downloadId: string; downloadKind: "preset" | "url"; model: string; totalBytes: number };
      if (source.kind === "preset") {
        const preset = modelPresets.find((candidate) => candidate.id === source.presetId);
        if (!preset) {
          return {
            status: "unavailable",
            failure: { kind: "unavailable", code: "unknown_preset", detail: "This preset is not offered by PAM.", recovery: null },
          };
        }
        started = { downloadId: preset.id, downloadKind: "preset", model: preset.model, totalBytes: preset.expectedSizeBytes };
      } else {
        // The same refusals the desktop's pasted-URL gate produces, in the
        // order it produces them, so fixture-driven UI meets real messages.
        const refusal = refusePastedDownload(source);
        if (refusal) {
          return { status: "unavailable", failure: refusal };
        }
        started = {
          downloadId: source.model,
          downloadKind: "url",
          model: source.model,
          totalBytes: source.expectedSizeBytes,
        };
      }
      if (download?.status === "running") {
        return {
          status: "unavailable",
          failure: {
            kind: "unavailable",
            code: "download_already_running",
            detail: "A model download is already running.",
            recovery: "Wait for the current download to finish, then try again.",
          },
        };
      }
      downloadAttempts += 1;
      // Restarting a cancelled download resumes from the kept partial bytes,
      // mirroring the daemon's on-disk resume.
      const resumeBytes =
        download?.status === "cancelled" && download.downloadId === started.downloadId ? download.receivedBytes : 0;
      download = { ...started, receivedBytes: resumeBytes, status: "running", failure: null };
      return { status: "ok" };
    },
    async modelDownloadCancel(_fence): Promise<ModelDownloadDto> {
      if (download?.status !== "running") {
        return {
          status: "unavailable",
          failure: { kind: "unavailable", code: "download_not_running", detail: "No model download is running.", recovery: "Start a download before cancelling one." },
        };
      }
      download.status = "cancelled";
      download.failure = null;
      return { status: "ok" };
    },
    async modelDownloadStatus(_fence): Promise<ModelDownloadStatusDto> {
      if (!download) {
        return { status: "idle", downloadId: null, downloadKind: null, receivedBytes: 0, totalBytes: 0, failure: null };
      }
      if (download.status === "running") {
        download.receivedBytes = Math.min(download.totalBytes, download.receivedBytes + Math.ceil(download.totalBytes * 0.4));
        if (scenario === "model-download-fail" && downloadAttempts === 1) {
          download.status = "failed";
          download.failure = {
            kind: "unavailable",
            code: "connection_reset",
            detail: "The download connection dropped.",
            recovery: "Check the network and retry.",
          };
        } else if (download.receivedBytes >= download.totalBytes) {
          download.status = "complete";
          modelCatalog.push({ modelId: download.model, sizeBytes: download.totalBytes });
        }
      }
      return clone({
        status: download.status,
        downloadId: download.downloadId,
        downloadKind: download.downloadKind,
        receivedBytes: download.receivedBytes,
        totalBytes: download.totalBytes,
        failure: download.failure,
      });
    },
    async hostMemory(_fence): Promise<HostMemoryDto> {
      return { totalBytes: FIXTURE_HOST_MEMORY_BYTES, supportedMinimumBytes: FIXTURE_SUPPORTED_MINIMUM_BYTES };
    },
    async appSettings(_fence): Promise<AppSettingsDto> {
      return clone(appSettingsSnapshot());
    },
    async settingsUpdate(_fence, requestedModelsDir): Promise<AppSettingsDto> {
      if (requestedModelsDir !== null) {
        if (!requestedModelsDir.startsWith("/")) {
          throw new Error("The models directory must be an absolute path with no `..` segments.");
        }
        modelsDir = requestedModelsDir;
      } else {
        modelsDir = FIXTURE_DEFAULT_MODELS_DIR;
      }
      return clone(appSettingsSnapshot());
    },
    async settingsSetDefaultModel(_fence, model): Promise<AppSettingsDto> {
      if (model !== null && !/^[^/]+\/[^/]+$/.test(model)) {
        throw new Error("The default model must be a registered vendor/name pair.");
      }
      defaultModel = model;
      return clone(appSettingsSnapshot());
    },
    async logsDelete(_fence): Promise<AppSettingsDto> {
      logsSizeBytes = 0;
      return clone(appSettingsSnapshot());
    },
    async resetAccess(_fence, dryRun): Promise<ResetDto> {
      return runReset("access", dryRun);
    },
    async resetIdentity(_fence, dryRun): Promise<ResetDto> {
      return runReset("identity", dryRun);
    },
    async resetHistory(_fence, dryRun): Promise<ResetDto> {
      return runReset("history", dryRun);
    },
    async resetRegistry(_fence, dryRun): Promise<ResetDto> {
      return runReset("registry", dryRun);
    },
    async factoryReset(_fence, dryRun, includeWeights): Promise<ResetDto> {
      if (daemonRunning) {
        return {
          status: "unavailable",
          failure: {
            kind: "unavailable",
            code: null,
            detail: "a running daemon still owns PAM's durable state",
            recovery:
              "Stop PAM first -- quit the running `pam daemon`, or press Stop in the PAM control center -- then run the reset again.",
          },
        };
      }
      const items = [
        ...Object.values(resetState).flat(),
        ...factoryExtras,
        ...(includeWeights
          ? [{ kind: "model_weights", count: 2, bytes: 22_300_000_000, names: [] }]
          : []),
      ];
      return {
        status: "ok",
        result: resetResult("factory", items, dryRun),
        receiptPath: dryRun
          ? null
          : "/Users/fixture/Library/Application Support/pam-reset-receipt-1730000000000.txt",
      };
    },
    async revealPath(_fence, path): Promise<void> {
      const known = [modelsDir, FIXTURE_DATA_DIR, FIXTURE_FLOWS_DIR, FIXTURE_LOGS_DIR];
      if (!known.includes(path)) throw new Error("This path is not a PAM Settings location.");
    },
    async callerRegistry(_fence): Promise<CallersDto> {
      if (!daemonRunning) {
        return {
          status: "unavailable",
          failure: {
            code: "daemon_offline",
            detail: "PAM is paused, so the caller registry is not being served.",
            recovery: "Start PAM to read the registered callers.",
          },
        };
      }
      return clone({ status: "ok" as const, callers: registeredCallers });
    },
    async daemonAccessConfig(_fence): Promise<AccessConfigDto> {
      // The observed boundary is daemon truth: no project identity reaches it,
      // so it answers the same way with or without an active project.
      if (!daemonRunning) {
        return clone({
          status: "unavailable" as const,
          failure: { kind: "unavailable" as const, code: "daemon_offline", detail: "PAM is paused, so no access boundary is being reported.", recovery: "Start PAM to read the observed boundary." },
        });
      }
      if (scenario === "access-blocked") {
        return clone({
          status: "blocked" as const,
          failure: { kind: "blocked" as const, code: "Forbidden", detail: "Network diagnostics are blocked by policy for this PAM window.", recovery: "Grant network.diagnostics for this GUI caller, then retry." },
          approvalId: null,
          expiresAtMs: null,
        });
      }
      return clone({
        status: "available" as const,
        truth: "System trust and proxy discovery are available to this PAM window.",
        platformRootsEnabled: true,
        systemProxyDiscoveryEnabled: true,
        proxyEnvironment: "not configured",
        noProxy: "configured",
        pac: "not detected",
      });
    },
    async daemonAccess(_fence): Promise<DaemonAccessDto> {
      return { capabilities: clone(daemonCapabilities) };
    },
    async setDaemonAccess(_fence, capability, granted): Promise<DaemonAccessDto> {
      const row = daemonCapabilities.find((candidate) => candidate.capability === capability);
      if (!row) throw new Error("This is not a daemon-scoped capability the PAM window uses.");
      row.granted = granted;
      return { capabilities: clone(daemonCapabilities) };
    },
    async connectorRegistry(_fence): Promise<ConnectorsDto> {
      if (!daemonRunning) {
        return {
          status: "unavailable",
          failure: {
            kind: "unavailable",
            code: "daemon_offline",
            detail: "PAM is paused, so the connector registry is not being served.",
            recovery: "Start PAM to read the connectors.",
          },
        };
      }
      if (scenario === "empty") return { status: "ok", connectors: [] };
      return clone({ status: "ok" as const, connectors });
    },
    async connectorConfigure(_fence, params): Promise<ConnectorConfigureDto> {
      if (scenario === "connector-blocked") {
        return {
          status: "blocked",
          failure: {
            kind: "blocked",
            code: "connector_configure_blocked",
            detail: "Project policy has not granted connector.configure to this caller yet.",
            recovery: "pam access grant connector.configure for this GUI caller and project, then retry.",
          },
        };
      }
      const summary = connectors.find((candidate) => candidate.connectorId === params.connector);
      if (!summary) {
        return {
          status: "unavailable",
          failure: { kind: "unavailable", code: "unknown_connector", detail: "This connector is not registered with the daemon.", recovery: null },
        };
      }
      if (params.enabled !== undefined) summary.enabled = params.enabled;
      if (params.baseUrl !== undefined) summary.baseUrl = params.baseUrl === "" ? null : params.baseUrl;
      if (params.credential) summary.credentialPresent = params.credential.action === "set";
      return clone({ status: "ok" as const, connector: summary });
    },
    async connectorTest(_fence, connector): Promise<ConnectorTestDto> {
      if (scenario === "connector-blocked") {
        return {
          status: "blocked",
          failure: {
            kind: "blocked",
            code: "connector_test_blocked",
            detail: "Project policy has not granted connector.test to this caller yet.",
            recovery: "pam access grant connector.test for this GUI caller and project, then retry.",
          },
        };
      }
      const summary = connectors.find((candidate) => candidate.connectorId === connector);
      if (!summary) {
        return {
          status: "unavailable",
          failure: { kind: "unavailable", code: "unknown_connector", detail: "This connector is not registered with the daemon.", recovery: null },
        };
      }
      const result = summary.credentialPresent && summary.baseUrl !== null ? "passed" : "failed";
      summary.lastTestStatus = result;
      summary.lastTestAtMs = 1_777_002_000_000;
      return {
        status: "ok",
        connectorId: connector,
        result,
        detail: result === "passed"
          ? "The connector answered the bounded test call."
          : "The test needs a base URL and a stored credential before it can reach out.",
      };
    },
    async activateProject(projectHandle, operationId) {
      const selected = catalogProjects.find((project) => project.handle === projectHandle);
      if (!selected) throw new Error("The selected fixture project is unavailable.");
      active = selected;
      generation = projectHandle === projects[1].handle
        ? "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb"
        : projectHandle === projects[2].handle
          ? "cccccccc-cccc-4ccc-8ccc-cccccccccccc"
          : "99999999-9999-4999-8999-999999999999";
      return fenceResponse(currentFence(operationId), snapshot(active, daemonRunning, scenario));
    },
    async refreshProject(fence) { return fenceResponse(rotatedFence(fence.operationId), snapshot(requireActive(), daemonRunning, scenario)); },
    // Under the daemon authority the lifecycle answers with no snapshot; a
    // project fence still receives a freshly fenced snapshot.
    async startDaemon(fence, model) {
      daemonRunning = true;
      if (model) modelLoaded = modelCatalog.find((entry) => entry.modelId === model) ?? null;
      if (isDaemonFence(fence)) return null;
      return fenceResponse(rotatedFence(fence.operationId), snapshot(requireActive(), daemonRunning, scenario));
    },
    async daemonStartupProgress(_fence): Promise<DaemonStartupProgressDto> {
      if (scenario !== "model-loading") {
        return { modelId: null, phase: null, loadedBytes: 0, totalBytes: 0, elapsedSeconds: 0 };
      }
      startupElapsedSeconds += 20;
      // The first three polls hash the artifact (resident memory flat), then
      // the weights map in and settle short of the artifact size.
      const mapped = Math.min(0.4, Math.max(0, (startupElapsedSeconds - 60) / 400));
      return {
        modelId: loadedModel.modelId,
        phase: mapped > 0 ? "loading" : "verifying",
        loadedBytes: Math.round(loadedModel.sizeBytes * mapped),
        totalBytes: loadedModel.sizeBytes,
        elapsedSeconds: startupElapsedSeconds,
      };
    },
    async stopDaemon(fence) {
      daemonRunning = false;
      if (isDaemonFence(fence)) return null;
      return fenceResponse(rotatedFence(fence.operationId), snapshot(requireActive(), daemonRunning, scenario));
    },
    async registerGuiCaller(fence) { return fenceResponse(rotatedFence(fence.operationId), solvedSnapshot(requireActive(), daemonRunning)); },
    async decideApproval(fence, _approvalHandle: string, decision: ApprovalDecision) {
      const data = solvedSnapshot(requireActive(), daemonRunning);
      if (decision === "deny") {
        data.current = {
          status: "unavailable",
          failure: {
            kind: "unavailable",
            code: "approval_denied",
            detail: "This exact project-current request was denied.",
            recovery: null,
          },
        };
      }
      return { disposition: decision === "approve" ? "approved" : "denied", snapshot: fenceResponse(rotatedFence(fence.operationId), data) };
    },
    async loadEvidence(fence, evidenceHandle) {
      if (scenario === "evidence-loading") return new Promise(() => {});
      if (scenario === "evidence-failed") throw new Error("The bounded evidence preview could not be loaded. Retry from the retained handle.");
      const binary = scenario === "evidence-binary";
      const truncated = scenario === "evidence-truncated";
      const data: EvidenceDataDto = {
        handle: evidenceHandle,
        digest: evidenceHandle === evidenceHandles[0] ? "sha256:fixture-ci" : "sha256:fixture-git",
        sizeBytes: binary ? 32_768 : truncated ? 19_212 : 108,
        mediaType: binary ? "application/octet-stream" : "text/plain",
        body: binary
          ? null
          : truncated
            ? `${"retained evidence line\n".repeat(220)}preview stops at the bounded read limit`
            : evidenceHandle === evidenceHandles[0]
              ? "GitHub Actions · integration-test · exit 1\nNull currency in fixture triggers 500 at CurrencyService.java:142"
              : "2 files changed\nAll checks green\nguard currency before invoking conversion pipeline",
        truncated,
        truth: binary ? "Binary evidence metadata" : evidenceHandle === evidenceHandles[0] ? "CI failure output" : "Verified Git patch",
      };
      return fenceResponse(fence, data);
    },
    async loadFlowWorkspace(fence) { return fenceResponse(fence, workspace()); },
    async loadSkillInventory(fence) {
      return fenceResponse(fence, skillInventory(scenario === "empty", fence.projectHandle === "daemon"));
    },
    async manageSkillLibrary(fence, action) {
      let data: SkillLibraryActionResultDto;
      if (action.action === "load") {
        data = { schemaVersion: 1, action: "load", entries: libraryEntries };
      } else if (action.action === "adopt" || action.action === "install_local" || action.action === "install_git") {
        const version = action.action === "adopt"
          ? "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
          : action.action === "install_local"
            ? "sha256:4444444444444444444444444444444444444444444444444444444444444444"
            : "sha256:5555555555555555555555555555555555555555555555555555555555555555";
        let entry = libraryEntries.find((candidate) => candidate.entryId === action.entryId);
        const alreadyPresent = entry?.versions.some((candidate) => candidate.version === version) ?? false;
        if (!entry) {
          entry = { entryId: action.entryId, versions: [] };
          libraryEntries.push(entry);
          libraryEntries.sort((left, right) => left.entryId.localeCompare(right.entryId));
        }
        if (!alreadyPresent) {
          entry.versions.push({
            version,
            installation: action.action === "install_git"
              ? { kind: "git", commit: "abcdefabcdefabcdefabcdefabcdefabcdefabcd" }
              : action.action === "install_local"
                ? { kind: "local" }
                : null,
            enabledAgents: [],
            managedAgents: [],
          });
          entry.versions.sort((left, right) => left.version.localeCompare(right.version));
        }
        const disposition = alreadyPresent ? "already_present" : "inserted";
        data = action.action === "adopt"
          ? {
              schemaVersion: 1,
              action: "adopt",
              entryId: action.entryId,
              version,
              artifactId: action.artifactId,
              disposition,
            }
          : {
              schemaVersion: 1,
              action: action.action,
              entryId: action.entryId,
              version,
              disposition,
            };
      } else {
        const key: SkillLibraryKeyDto = {
          entryId: action.entryId,
          version: action.version,
          agent: action.agent,
        };
        const version = libraryVersion(libraryEntries, key);
        const enabled = version.enabledAgents.includes(key.agent);
        const managed = version.managedAgents.includes(key.agent);
        const drift = libraryDrift.get(driftKey(key))
          ?? { state: "conflict", reason: enabled ? "unowned" : "disabled" } as const;
        if (action.action === "enable") {
          const changed = !enabled;
          setAgent(version.enabledAgents, key.agent, true);
          data = { schemaVersion: 1, action: "enable", key, enabled: true, changed };
        } else if (action.action === "disable") {
          const stateChanged = enabled;
          const cleanup = !managed
            ? "preserved_unowned"
            : drift.state === "missing"
              ? "missing"
              : drift.state === "modified"
                ? "preserved_modified"
                : drift.state === "conflict" && drift.reason === "symlink"
                  ? "preserved_symlink"
                  : "removed";
          setAgent(version.enabledAgents, key.agent, false);
          if (cleanup === "removed" || cleanup === "missing") setAgent(version.managedAgents, key.agent, false);
          libraryDrift.set(driftKey(key), { state: "conflict", reason: "disabled" });
          data = { schemaVersion: 1, action: "disable", key, stateChanged, cleanup };
        } else if (action.action === "preview_materialization" || action.action === "preview_resync") {
          data = {
            schemaVersion: 1,
            action: action.action,
            items: [{
              key,
              action: fixturePlanAction(managed, drift),
              existing: drift.state === "modified" ? { byteLen: 1_024, digest: drift.actualDigest } : null,
              backupPlanned: drift.state === "modified",
            }],
          };
        } else if (action.action === "apply_materialization" || action.action === "apply_resync") {
          const outcomeAction = fixturePlanAction(managed, drift);
          const ownershipRecorded = action.action === "apply_resync" || outcomeAction !== "no_op" || managed;
          if (ownershipRecorded) setAgent(version.managedAgents, key.agent, true);
          libraryDrift.set(driftKey(key), { state: "clean" });
          data = {
            schemaVersion: 1,
            action: action.action,
            outcomes: [{
              key,
              action: outcomeAction,
              backup: outcomeAction === "replace"
                ? { byteLen: 1_024, digest: "sha256:6666666666666666666666666666666666666666666666666666666666666666" }
                : null,
              ownershipRecorded,
            }],
          };
        } else {
          data = {
            schemaVersion: 1,
            action: "inspect_drift",
            inspection: {
              key,
              expectedDigest: key.version,
              state: drift,
            },
          };
        }
      }
      return fenceResponse(fence, data);
    },
    async loadSkillAudit(fence) {
      if (scenario === "skill-audit-load-error") throw new Error("The latest skill audit could not be loaded.");
      if (scenario === "skill-audit-empty" || scenario === "empty") return fenceResponse(fence, null);
      if (scenario === "skill-audit-no-evaluator") return fenceResponse(fence, skillAudit({ status: "no_evaluator" }));
      if (scenario === "skill-audit-failed") {
        return fenceResponse(fence, skillAudit({ status: "failed", evaluator: "cursor_agent", failure: "invalid_verdict" }));
      }
      return fenceResponse(fence, skillAudit());
    },
    async runSkillAudit(fence) { return fenceResponse(fence, skillAudit()); },
    async openFlow(fence, flowHandle) {
      if (flowHandle !== definitionHandle) throw new Error("This fixture definition has no editable document.");
      return fenceResponse(fence, document());
    },
    async flowGraph(_fence, source) {
      const definition = flowGraphSources.get(normalizeFlowSource(source));
      if (!definition) {
        return {
          status: "invalid",
          failure: { detail: "This source has hand edits the visual editor cannot follow yet. Keep working here, or validate first." },
        };
      }
      return { status: "ok", definition: clone(definition) };
    },
    async flowCompose(_fence, definition) {
      const source = composeFlowToml(definition);
      flowGraphSources.set(normalizeFlowSource(source), clone(definition));
      return { status: "ok", source };
    },
    async validateFlow(fence, _documentHandle, source) {
      if (!source.includes("schema_version = 2") || !source.includes("[[steps]]")) {
        throw new Error("The fixture validator requires schema version 2 and at least one step.");
      }
      const data: FlowReviewDataDto = {
        document: documentHandle,
        identity,
        normalizedToml: `${source.trimEnd()}\n`,
        dryRun: {
          daemonDefinitionEligible: true,
          steps: [
            { index: 0, id: "observe-revision", semanticRole: "observe", condition: "always", approval: "none", effect: "read_only", maxAttempts: 1, initialBackoffMs: 0, maxBackoffMs: 0, action: "git rev-parse --verify HEAD", daemonAuthority: "supported" },
          ],
        },
        diff: {
          changed: source !== savedSource,
          truncated: false,
          lines: source === savedSource
            ? []
            : [
                { kind: "removed", text: `revision = ${identity.revision}` },
                { kind: "added", text: `revision = ${identity.revision + 1}` },
              ],
        },
      };
      return fenceResponse(fence, data);
    },
    async saveFlow(fence, _documentHandle, source) {
      savedSource = `${source.trimEnd()}\n`;
      const data: FlowSaveDataDto = { document: documentHandle, identity, created: false, durabilityConfirmed: true, cleanupComplete: true };
      return fenceResponse(fence, data);
    },
    async flowRun(fence, flowHandle) {
      const definitionId = flowHandle === secondDefinitionHandle ? "release-confidence" : "after-merge-checks";
      const runId = `flow-run-${definitionId}-demo`;
      startedRun = {
        runId,
        definitionId,
        projectLabel: requireActive().location,
        retryCommand: `pam flow run ${definitionId} --run-id ${runId} --idempotency-key flow-run:${runId}`,
      };
      return fenceResponse(fence, startedRun);
    },
    async flowRunProgress(fence, run) {
      return fenceResponse(fence, runProgress(run));
    },
    async flowRunCancel(fence, run) {
      return fenceResponse(fence, { runId: run, disposition: "requested" });
    },
    async flowRunHistory(fence) {
      const started = startedRun;
      if (!started) return fenceResponse(fence, runHistory);
      return fenceResponse(fence, {
        runs: [
          {
            runId: started.runId,
            definitionId: started.definitionId,
            projectLabel: started.projectLabel,
            state: "succeeded",
            outcome: "solved",
            startedAtMs: 1_777_001_400_000,
            completedAtMs: 1_777_001_440_000,
          },
          ...runHistory.runs,
        ],
        truncated: false,
      });
    },
  };
}
