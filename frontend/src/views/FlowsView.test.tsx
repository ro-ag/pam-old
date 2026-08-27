import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { CommandFence, PamBridge } from "../domain";
import { afterMergeDefinition, fixtureBridge } from "../fixtures";
import { FlowsView } from "./FlowsView";

const fence: CommandFence = {
  projectHandle: "project:test",
  generation: "11111111-1111-4111-8111-111111111111",
  operationId: "22222222-2222-4222-8222-222222222222",
};

async function openAfterMerge(bridge: PamBridge = fixtureBridge()) {
  const user = userEvent.setup();
  const onToast = vi.fn();
  render(<FlowsView bridge={bridge} fence={fence} onError={vi.fn()} onToast={onToast} />);
  await screen.findByRole("region", { name: "Flow workspace" });
  await user.click(screen.getByRole("button", { name: /after-merge-checks/ }));
  const stepList = await screen.findByRole("group", { name: "Flow steps" });
  return { user, bridge, onToast, stepList };
}

const sourceTextarea = () => screen.getByRole("textbox", { name: "Flow TOML source" }) as HTMLTextAreaElement;

describe("FlowsView visual editor", () => {
  it("opens a flow in visual mode and renders one node per step", async () => {
    const { stepList } = await openAfterMerge();
    expect(within(stepList).getByRole("button", { name: /observe-revision/ })).toBeInTheDocument();
    expect(within(stepList).getByRole("button", { name: /verify-worktree/ })).toBeInTheDocument();
    expect(document.querySelectorAll(".react-flow__node")).toHaveLength(2);
    expect(screen.queryByRole("textbox", { name: "Flow TOML source" })).not.toBeInTheDocument();
  });

  it("falls back to Source mode with a calm notice when the document cannot be graphed", async () => {
    const bridge = fixtureBridge();
    bridge.flowGraph = vi.fn(async () => ({
      status: "invalid" as const,
      failure: { detail: "This document uses shapes the visual editor cannot follow yet." },
    }));
    const user = userEvent.setup();
    render(<FlowsView bridge={bridge} fence={fence} onError={vi.fn()} onToast={vi.fn()} />);
    await screen.findByRole("region", { name: "Flow workspace" });
    await user.click(screen.getByRole("button", { name: /after-merge-checks/ }));

    expect((await screen.findByRole("textbox", { name: "Flow TOML source" }) as HTMLTextAreaElement).value).toContain("schema_version = 2");
    expect(screen.getByText(/cannot follow yet/)).toBeInTheDocument();
    expect(screen.queryByRole("group", { name: "Flow steps" })).not.toBeInTheDocument();
  });

  it("stays in Source mode when hand edits cannot be graphed back", async () => {
    const { user } = await openAfterMerge();
    await user.click(screen.getByRole("button", { name: "Source" }));
    const source = sourceTextarea();
    expect(source.value).toContain("schema_version = 2");

    fireEvent.change(source, { target: { value: "schema_version = 2\n[[steps]]\nhand edited" } });
    await user.click(screen.getByRole("button", { name: "Visual" }));

    expect(await screen.findByText(/hand edits/)).toBeInTheDocument();
    expect(sourceTextarea()).toBeInTheDocument();
    expect(screen.queryByRole("group", { name: "Flow steps" })).not.toBeInTheDocument();
  });

  it("carries inspector edits into the composed source", async () => {
    const { user, stepList } = await openAfterMerge();
    await user.click(within(stepList).getByRole("button", { name: /observe-revision/ }));
    const description = screen.getByLabelText(/Description/) as HTMLInputElement;
    expect(description.value).toBe("Record the checked-out revision as evidence.");
    fireEvent.change(description, { target: { value: "Observe the head revision." } });

    await user.click(screen.getByRole("button", { name: "Source" }));
    expect(sourceTextarea().value).toContain('description = "Observe the head revision."');
  });

  it("adds and deletes steps while keeping depends_on tidy", async () => {
    const { user, stepList } = await openAfterMerge();
    await user.click(within(stepList).getByRole("button", { name: "Add step" }));
    expect(within(stepList).getByRole("button", { name: /step-3/ })).toBeInTheDocument();

    await user.click(within(stepList).getByRole("button", { name: /observe-revision/ }));
    await user.click(screen.getByRole("button", { name: "Delete step" }));
    expect(within(stepList).queryByRole("button", { name: /observe-revision/ })).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Source" }));
    const source = sourceTextarea().value;
    expect(source).not.toContain("observe-revision");
    expect(source).toContain('id = "step-3"');
    expect(source).toContain("depends_on = []");
    expect(source).toContain('condition = { kind = "always" }');
  });

  it("undoes and redoes visual edits with the platform shortcut", async () => {
    const { user, stepList } = await openAfterMerge();
    await user.click(within(stepList).getByRole("button", { name: /observe-revision/ }));
    const description = () => screen.getByLabelText(/Description/) as HTMLInputElement;
    fireEvent.change(description(), { target: { value: "Edited once." } });
    expect(description().value).toBe("Edited once.");

    fireEvent.keyDown(window, { key: "z", metaKey: true });
    expect(description().value).toBe("Record the checked-out revision as evidence.");

    fireEvent.keyDown(window, { key: "z", metaKey: true, shiftKey: true });
    expect(description().value).toBe("Edited once.");
  });

  it("saves visual work through compose, validate, then save", async () => {
    const bridge = fixtureBridge();
    const order: string[] = [];
    const originalCompose = bridge.flowCompose.bind(bridge);
    const originalValidate = bridge.validateFlow.bind(bridge);
    const originalSave = bridge.saveFlow.bind(bridge);
    let composedSource: string | null = null;
    bridge.flowCompose = vi.fn(async (requestFence, definition) => {
      order.push("compose");
      const result = await originalCompose(requestFence, definition);
      if (result.status === "ok") composedSource = result.source;
      return result;
    });
    bridge.validateFlow = vi.fn(async (requestFence, documentHandle, source) => {
      order.push("validate");
      return originalValidate(requestFence, documentHandle, source);
    });
    bridge.saveFlow = vi.fn(async (requestFence, documentHandle, source) => {
      order.push("save");
      return originalSave(requestFence, documentHandle, source);
    });

    const { user, stepList, onToast } = await openAfterMerge(bridge);
    await user.click(within(stepList).getByRole("button", { name: /observe-revision/ }));
    fireEvent.change(screen.getByLabelText(/Description/), { target: { value: "Observe, calmly." } });

    await user.click(screen.getByRole("button", { name: "Validate" }));
    expect(await screen.findByText(/Valid · 1 dry-run steps/)).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onToast).toHaveBeenCalledWith("Flow saved durably in the shared flow library"));
    expect(order).toEqual(["compose", "validate", "save"]);
    expect(bridge.validateFlow).toHaveBeenCalledWith(expect.anything(), expect.any(String), composedSource);
  });

  it("round-trips the fixture flow losslessly between visual and source", async () => {
    const bridge = fixtureBridge();
    const composed = await bridge.flowCompose(fence, afterMergeDefinition);
    expect(composed.status).toBe("ok");
    const graphed = await bridge.flowGraph(fence, composed.status === "ok" ? composed.source : "");
    expect(graphed).toEqual({ status: "ok", definition: afterMergeDefinition });

    const { user, stepList } = await openAfterMerge(bridge);
    await user.click(within(stepList).getByRole("button", { name: /observe-revision/ }));
    fireEvent.change(screen.getByLabelText(/Description/), { target: { value: "Round trip." } });
    await user.click(screen.getByRole("button", { name: "Source" }));
    await user.click(screen.getByRole("button", { name: "Visual" }));
    const list = await screen.findByRole("group", { name: "Flow steps" });
    await user.click(within(list).getByRole("button", { name: /observe-revision/ }));
    expect((screen.getByLabelText(/Description/) as HTMLInputElement).value).toBe("Round trip.");
  });

  it("navigates the step list with arrow keys", async () => {
    const { user, stepList } = await openAfterMerge();
    const first = within(stepList).getByRole("button", { name: /observe-revision/ });
    const second = within(stepList).getByRole("button", { name: /verify-worktree/ });
    await user.click(first);
    expect(first).toHaveAttribute("aria-pressed", "true");

    await user.keyboard("{ArrowDown}");
    expect(second).toHaveFocus();
    expect(second).toHaveAttribute("aria-pressed", "true");

    await user.keyboard("{ArrowUp}");
    expect(first).toHaveFocus();
    expect(first).toHaveAttribute("aria-pressed", "true");
  });
});

describe("FlowsView without an active project", () => {
  it("loads the global flow library under the daemon authority", async () => {
    const bridge = fixtureBridge();
    const loadWorkspace = vi.spyOn(bridge, "loadFlowWorkspace");
    render(<FlowsView bridge={bridge} fence={null} onError={vi.fn()} onToast={vi.fn()} />);

    await screen.findByRole("region", { name: "Flow workspace" });
    expect(screen.getByRole("button", { name: /after-merge-checks/ })).toBeInTheDocument();
    expect(loadWorkspace.mock.calls[0][0]).toMatchObject({ projectHandle: "daemon", generation: "daemon" });
  });
});

describe("FlowsView fence rotations", () => {
  it("keeps the draft across a same-project generation rotation while refreshing the catalog", async () => {
    const bridge = fixtureBridge();
    const loadWorkspace = vi.spyOn(bridge, "loadFlowWorkspace");
    const user = userEvent.setup();
    const { rerender } = render(<FlowsView bridge={bridge} fence={fence} onError={vi.fn()} onToast={vi.fn()} />);
    await screen.findByRole("region", { name: "Flow workspace" });
    await user.click(screen.getByRole("button", { name: /after-merge-checks/ }));
    await screen.findByRole("group", { name: "Flow steps" });
    await user.click(screen.getByRole("button", { name: "Source" }));
    fireEvent.change(sourceTextarea(), { target: { value: "# unsaved draft\nschema_version = 2" } });

    rerender(
      <FlowsView
        bridge={bridge}
        fence={{ ...fence, generation: "33333333-3333-4333-8333-333333333333" }}
        onError={vi.fn()}
        onToast={vi.fn()}
      />,
    );

    // The generation rotation re-fetches the workspace in place…
    await waitFor(() => expect(loadWorkspace).toHaveBeenCalledTimes(2));
    // …but the unsaved draft and the open document survive.
    expect(sourceTextarea().value).toBe("# unsaved draft\nschema_version = 2");
    expect(sourceTextarea()).toBeEnabled();
  });

  it("keeps the editor when the project itself switches, because the library is global", async () => {
    const bridge = fixtureBridge();
    const loadWorkspace = vi.spyOn(bridge, "loadFlowWorkspace");
    const user = userEvent.setup();
    const { rerender } = render(<FlowsView bridge={bridge} fence={fence} onError={vi.fn()} onToast={vi.fn()} />);
    await screen.findByRole("region", { name: "Flow workspace" });
    await user.click(screen.getByRole("button", { name: /after-merge-checks/ }));
    await screen.findByRole("group", { name: "Flow steps" });
    await user.click(screen.getByRole("button", { name: "Source" }));
    fireEvent.change(sourceTextarea(), { target: { value: "# unsaved draft" } });

    rerender(
      <FlowsView
        bridge={bridge}
        fence={{ ...fence, projectHandle: "project:other", generation: "33333333-3333-4333-8333-333333333333" }}
        onError={vi.fn()}
        onToast={vi.fn()}
      />,
    );

    await waitFor(() => expect(loadWorkspace).toHaveBeenCalledTimes(2));
    expect(sourceTextarea().value).toBe("# unsaved draft");
    expect(screen.getByRole("heading", { name: "after-merge-checks.toml" })).toBeInTheDocument();
    // Every catalog command still speaks the daemon authority, never a project fence.
    for (const [requestFence] of loadWorkspace.mock.calls) {
      expect(requestFence).toMatchObject({ projectHandle: "daemon", generation: "daemon" });
    }
  });

  it("keeps the newest open when an earlier one is still in flight", async () => {
    const bridge = fixtureBridge();
    const gates: Array<() => void> = [];
    const names = new Map<string, string>();
    const nameFor = (handle: string) => {
      if (!names.has(handle)) names.set(handle, names.size === 0 ? "flow-a.toml" : "flow-b.toml");
      return names.get(handle) as string;
    };
    bridge.openFlow = vi.fn(async (requestFence, flowHandle) => {
      const fileName = nameFor(flowHandle);
      await new Promise<void>((resolve) => gates.push(resolve));
      return {
        fence: { ...requestFence },
        data: {
          handle: `document:${flowHandle}`,
          identity: { fileName, id: fileName.replace(".toml", ""), revision: 1, digest: "sha256:fixture" },
          source: `# ${fileName}\n`,
        },
      };
    });
    const user = userEvent.setup();
    render(<FlowsView bridge={bridge} fence={fence} onError={vi.fn()} onToast={vi.fn()} />);
    await screen.findByRole("region", { name: "Flow workspace" });

    await user.click(screen.getByRole("button", { name: /after-merge-checks/ }));
    await user.click(screen.getByRole("button", { name: /release-confidence/ }));
    // The stale open resolves last; the newest document still wins.
    await act(async () => { gates[1](); gates[0](); });

    expect(await screen.findByRole("heading", { name: "flow-b.toml" })).toBeInTheDocument();
    expect(sourceTextarea().value).toBe("# flow-b.toml\n");
  });
});

describe("FlowsView operation identity", () => {
  it("spends a fresh operation on each command of an open", async () => {
    const bridge = fixtureBridge();
    const openFlow = vi.spyOn(bridge, "openFlow");
    const flowGraph = vi.spyOn(bridge, "flowGraph");

    await openAfterMerge(bridge);

    expect(openFlow).toHaveBeenCalledTimes(1);
    expect(flowGraph).toHaveBeenCalledTimes(1);
    // Reusing one operation across two daemon commands trips the replay guard.
    expect(flowGraph.mock.calls[0][0].operationId).not.toBe(openFlow.mock.calls[0][0].operationId);
  });

  it("spends a fresh operation on compose and on validate", async () => {
    const bridge = fixtureBridge();
    const flowCompose = vi.spyOn(bridge, "flowCompose");
    const validateFlow = vi.spyOn(bridge, "validateFlow");

    const { user } = await openAfterMerge(bridge);
    await user.click(screen.getByRole("button", { name: "Validate" }));
    await screen.findByText(/Valid ·/);

    expect(flowCompose).toHaveBeenCalledTimes(1);
    expect(validateFlow).toHaveBeenCalledTimes(1);
    expect(validateFlow.mock.calls[0][0].operationId).not.toBe(flowCompose.mock.calls[0][0].operationId);
  });
});
