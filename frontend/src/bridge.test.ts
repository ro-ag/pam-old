import { describe, expect, it } from "vitest";
import { createTauriBridge, sameFence, withDaemonOperation } from "./bridge";
import type { CommandFence } from "./domain";
import { afterMergeDefinition } from "./fixtures";

const fence: CommandFence = {
  projectHandle: "project:opaque",
  generation: "11111111-1111-4111-8111-111111111111",
  operationId: "22222222-2222-4222-8222-222222222222",
};

describe("Tauri bridge ABI", () => {
  it("wraps typed payloads once under request and uses exact camelCase keys", async () => {
    const calls: Array<[string, Record<string, unknown> | undefined]> = [];
    const invoke = async <T,>(command: string, args?: Record<string, unknown>) => {
      calls.push([command, args]);
      return { fence, data: {} } as T;
    };
    const bridge = createTauriBridge(invoke);

    await bridge.refreshProject(fence);
    await bridge.decideApproval(fence, "approval:opaque", "deny");
    await bridge.loadEvidence(fence, "evidence://bounded/1");
    await bridge.validateFlow(fence, "document:opaque", "schema_version = 2");
    await bridge.manageSkillLibrary(fence, {
      action: "install_git",
      entryId: "review-changes",
      url: "https://example.com/team/skills.git",
      artifactPath: "skills/review/SKILL.md",
    });

    expect(calls[0]).toEqual(["refresh_project", {
      request: { projectHandle: fence.projectHandle, generation: fence.generation, operationId: fence.operationId },
    }]);
    expect(calls[1]).toEqual(["decide_approval", {
      request: { ...fence, approvalHandle: "approval:opaque", decision: "deny" },
    }]);
    expect(calls[2]).toEqual(["load_evidence", {
      request: { ...fence, evidenceHandle: "evidence://bounded/1" },
    }]);
    expect(calls[3]).toEqual(["validate_flow", {
      request: { ...fence, documentHandle: "document:opaque", source: "schema_version = 2" },
    }]);
    expect(calls[4]).toEqual(["manage_skill_library", {
      request: {
        ...fence,
        action: "install_git",
        entryId: "review-changes",
        url: "https://example.com/team/skills.git",
        artifactPath: "skills/review/SKILL.md",
      },
    }]);

    await bridge.flowGraph(fence, "schema_version = 2");
    await bridge.flowCompose(fence, afterMergeDefinition);
    expect(calls[5]).toEqual(["flow_graph", {
      request: { ...fence, source: "schema_version = 2" },
    }]);
    expect(calls[6]).toEqual(["flow_compose", {
      request: { ...fence, definition: JSON.stringify(afterMergeDefinition) },
    }]);
  });

  it("supplies bootstrap with only a canonical operation id", async () => {
    const calls: Array<[string, Record<string, unknown> | undefined]> = [];
    const invoke = async <T,>(command: string, args?: Record<string, unknown>) => {
      calls.push([command, args]);
      return { fence, data: {} } as T;
    };
    const bridge = createTauriBridge(invoke);
    await bridge.bootstrap();

    expect(calls).toHaveLength(1);
    const [command, args] = calls[0];
    expect(command).toBe("bootstrap");
    expect(args).toEqual({ request: { operationId: expect.stringMatching(/^[0-9a-f-]{36}$/) } });
  });

  it("keeps lifecycle and flow commands narrow", async () => {
    const calls: Array<[string, Record<string, unknown> | undefined]> = [];
    const invoke = async <T,>(command: string, args?: Record<string, unknown>) => {
      calls.push([command, args]);
      return { fence, data: {} } as T;
    };
    const bridge = createTauriBridge(invoke);

    await bridge.catalog();
    await bridge.activateProject(fence.projectHandle, fence.operationId);
    await bridge.startDaemon(fence);
    await bridge.stopDaemon(fence);
    await bridge.registerGuiCaller(fence);
    await bridge.loadFlowWorkspace(fence);
    await bridge.loadSkillInventory(fence);
    await bridge.manageSkillLibrary(fence, { action: "load" });
    await bridge.loadSkillAudit(fence);
    await bridge.runSkillAudit(fence);
    await bridge.openFlow(fence, "55555555-5555-4555-8555-555555555555");
    await bridge.saveFlow(fence, "66666666-6666-4666-8666-666666666666", "schema_version = 2");

    expect(calls.map(([command]) => command)).toEqual([
      "catalog", "activate_project", "start_daemon", "stop_daemon", "register_gui_caller", "load_flow_workspace", "load_skill_inventory", "manage_skill_library", "load_skill_audit", "run_skill_audit", "open_flow", "save_flow",
    ]);
    expect(calls[0][1]).toBeUndefined();
    expect(calls[1][1]).toEqual({ request: { projectHandle: fence.projectHandle, operationId: fence.operationId } });
    expect(calls[4][1]).toEqual({ request: fence });
    expect(calls[5][1]).toEqual({ request: fence });
    expect(calls[6][1]).toEqual({ request: fence });
    expect(calls[7][1]).toEqual({ request: { ...fence, action: "load" } });
    expect(calls[8][1]).toEqual({ request: fence });
    expect(calls[9][1]).toEqual({ request: fence });
    expect(calls[10][1]).toEqual({ request: { ...fence, flowHandle: "55555555-5555-4555-8555-555555555555" } });
    expect(calls[11][1]).toEqual({ request: { ...fence, documentHandle: "66666666-6666-4666-8666-666666666666", source: "schema_version = 2" } });
  });

  it("keeps the daemon observatory reads narrow with an optional bounded limit", async () => {
    const calls: Array<[string, Record<string, unknown> | undefined]> = [];
    const invoke = async <T,>(command: string, args?: Record<string, unknown>) => {
      calls.push([command, args]);
      return { status: "ok" } as T;
    };
    const bridge = createTauriBridge(invoke);

    await bridge.daemonActivity(fence, 25);
    await bridge.daemonActivity(fence);
    await bridge.callerRegistry(fence);
    await bridge.daemonHealth(fence);

    expect(calls).toEqual([
      ["daemon_activity", { request: { ...fence, limit: 25 } }],
      ["daemon_activity", { request: { ...fence, limit: null } }],
      ["caller_registry", { request: fence }],
      ["daemon_health", { request: fence }],
    ]);
  });

  it("mints the exact daemon authority fence with a fresh operation per call", () => {
    const first = withDaemonOperation();
    const second = withDaemonOperation();

    expect(first).toMatchObject({ projectHandle: "daemon", generation: "daemon" });
    expect(second).toMatchObject({ projectHandle: "daemon", generation: "daemon" });
    expect(first.operationId).toMatch(/^[0-9a-f-]{36}$/);
    expect(first.operationId).not.toBe(second.operationId);
  });

  it("keeps the connector commands on the fixed request ABI with credential passthrough", async () => {
    const calls: Array<[string, Record<string, unknown> | undefined]> = [];
    const invoke = async <T,>(command: string, args?: Record<string, unknown>) => {
      calls.push([command, args]);
      return { status: "ok" } as T;
    };
    const bridge = createTauriBridge(invoke);

    await bridge.connectorRegistry(fence);
    await bridge.connectorConfigure(fence, { connector: "github-actions", enabled: true, baseUrl: "https://api.github.com" });
    await bridge.connectorConfigure(fence, { connector: "github-actions", credential: { action: "set", secret: "exact-secret-bytes" } });
    await bridge.connectorConfigure(fence, { connector: "github-actions", credential: { action: "clear" } });
    await bridge.connectorTest(fence, "github-actions");

    expect(calls).toEqual([
      ["connector_registry", { request: fence }],
      ["connector_configure", { request: { ...fence, connector: "github-actions", enabled: true, baseUrl: "https://api.github.com" } }],
      ["connector_configure", { request: { ...fence, connector: "github-actions", credential: { action: "set", secret: "exact-secret-bytes" } } }],
      ["connector_configure", { request: { ...fence, connector: "github-actions", credential: { action: "clear" } } }],
      ["connector_test", { request: { ...fence, connector: "github-actions" } }],
    ]);
  });

  it("keeps the local-model commands on the fixed request ABI", async () => {
    const calls: Array<[string, Record<string, unknown> | undefined]> = [];
    const invoke = async <T,>(command: string, args?: Record<string, unknown>) => {
      calls.push([command, args]);
      return { status: "ok" } as T;
    };
    const bridge = createTauriBridge(invoke);
    const messages = [
      { role: "system" as const, content: "You are a bounded reviewer." },
      { role: "user" as const, content: "hello" },
    ];

    await bridge.modelStatus(fence);
    await bridge.modelInfer(fence, "qwen/qwen3-14b-instruct-q4", messages, 512);
    await bridge.modelInfer(fence, "qwen/qwen3-14b-instruct-q4", messages);
    await bridge.modelImportStatus(fence);

    expect(calls).toEqual([
      ["model_status", { request: fence }],
      ["model_infer", { request: { ...fence, model: "qwen/qwen3-14b-instruct-q4", messages, maxOutputTokens: 512 } }],
      ["model_infer", { request: { ...fence, model: "qwen/qwen3-14b-instruct-q4", messages } }],
      ["model_import_status", { request: fence }],
    ]);
  });

  it("keeps the Settings commands on the fixed request ABI", async () => {
    const calls: Array<[string, Record<string, unknown> | undefined]> = [];
    const invoke = async <T,>(command: string, args?: Record<string, unknown>) => {
      calls.push([command, args]);
      return {} as T;
    };
    const bridge = createTauriBridge(invoke);

    await bridge.appSettings(fence);
    await bridge.settingsUpdate(fence, "/Volumes/external/models");
    await bridge.settingsUpdate(fence, null);
    await bridge.logsDelete(fence);
    await bridge.revealPath(fence, "/Users/example/llm");

    expect(calls).toEqual([
      ["app_settings", { request: fence }],
      ["settings_update", { request: { ...fence, modelsDir: "/Volumes/external/models" } }],
      ["settings_update", { request: { ...fence, modelsDir: null } }],
      ["logs_delete", { request: fence }],
      ["reveal_path", { request: { ...fence, path: "/Users/example/llm" } }],
    ]);
  });

  it("compares all three fence fields", () => {
    expect(sameFence(fence, { ...fence })).toBe(true);
    expect(sameFence(fence, { ...fence, generation: "33333333-3333-4333-8333-333333333333" })).toBe(false);
    expect(sameFence(fence, { ...fence, operationId: "44444444-4444-4444-8444-444444444444" })).toBe(false);
    expect(sameFence(fence, { ...fence, projectHandle: "project:other" })).toBe(false);
  });
});
