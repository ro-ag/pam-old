import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { withDaemonOperation } from "../bridge";
import type { ModelStatusDto } from "../domain";
import { fixtureBridge, type FixtureScenario } from "../fixtures";
import { selectControlCenter, selectDaemonView } from "../selectors";
import { ActivityView, formatModelSize } from "./ActivityView";

async function activityProps(scenario: FixtureScenario = "solved", modelStatus: ModelStatusDto | null = null) {
  const bridge = fixtureBridge(scenario);
  const { snapshot, catalog } = await bridge.bootstrap();
  const control = snapshot ? selectControlCenter(snapshot.data, catalog, true) : null;
  return {
    bridge,
    daemon: control
      ? control.daemon
      : selectDaemonView(await bridge.daemonHealth(withDaemonOperation())),
    projects: catalog.projects,
    pending: false,
    modelStatus,
    evidence: control?.current.latestOutcome?.brief
      ? {
          projectName: control.project.name,
          handles: control.current.latestOutcome.brief.evidenceHandles,
          truncated: control.current.latestOutcome.brief.evidenceTruncated,
        }
      : null,
    onEvidence: vi.fn(),
    onReloadModel: vi.fn(),
    onOpenModelChat: vi.fn(),
    onStartDaemon: vi.fn(),
  };
}

const loadedStatus: ModelStatusDto = {
  status: "ok",
  loaded: { modelId: "qwen/qwen3-14b-instruct-q4", sizeBytes: 19_500_000_000 },
  registered: [{ modelId: "qwen/qwen3-14b-instruct-q4", sizeBytes: 19_500_000_000 }],
};

describe("ActivityView", () => {
  it("renders daemon health and the bounded activity feed", async () => {
    const props = await activityProps();
    render(<ActivityView {...props} />);

    expect(screen.getByRole("heading", { name: "Activity" })).toBeInTheDocument();
    expect(screen.getByText("PAM is on watch")).toBeInTheDocument();
    expect(screen.getByText("Daemon fixture-0.1.0")).toBeInTheDocument();
    expect(screen.getByText("Queue depth")).toBeInTheDocument();

    expect(await screen.findByText("project.current")).toBeInTheDocument();
    expect(screen.getByText(/gui:pam-desktop · payments-api · served/)).toBeInTheDocument();
    expect(screen.getByText(/cli:release-agent · ledger-web/)).toBeInTheDocument();
    expect(screen.getAllByText("allowed")).toHaveLength(3);
    expect(screen.getByText("approval required")).toBeInTheDocument();
  });

  it("refreshes the feed on demand", async () => {
    const user = userEvent.setup();
    const props = await activityProps();
    const spy = vi.spyOn(props.bridge, "daemonActivity");
    render(<ActivityView {...props} />);
    await screen.findByText("project.current");
    expect(spy).toHaveBeenCalledTimes(1);

    await user.click(screen.getByRole("button", { name: "Refresh activity" }));
    await waitFor(() => expect(spy).toHaveBeenCalledTimes(2));
  });

  it("always loads the feed under the exact daemon authority", async () => {
    const props = await activityProps();
    const spy = vi.spyOn(props.bridge, "daemonActivity");
    render(<ActivityView {...props} />);
    await screen.findByText("project.current");

    expect(spy.mock.calls[0][0]).toMatchObject({ projectHandle: "daemon", generation: "daemon" });
  });

  it("serves the feed with zero projects using opaque project labels", async () => {
    const props = await activityProps("global-only");
    render(<ActivityView {...props} />);

    expect(props.projects).toHaveLength(0);
    expect(await screen.findByText("project.current")).toBeInTheDocument();
    expect(screen.getByText(/gui:pam-desktop · 11111111…/)).toBeInTheDocument();
  });

  it("labels a project outside the catalog by its remembered root, and only truncates the id when rootless", async () => {
    const props = await activityProps();
    render(<ActivityView {...props} />);

    const rooted = await screen.findByText(/cli:daemon-only-rooted · scratch-agent/);
    expect(rooted).toHaveAttribute("title", "/work/scratch-agent");

    const rootless = screen.getByText(/cli:daemon-only-rootless · 77777777…/);
    expect(rootless).not.toHaveAttribute("title");
  });

  it("renders the exact empty feed without inventing events", async () => {
    const props = await activityProps("empty");
    render(<ActivityView {...props} />);

    expect(await screen.findByText(/No recent activity/)).toBeInTheDocument();
  });

  it("renders a calm unavailable feed with its recovery guidance", async () => {
    const props = await activityProps();
    props.bridge.daemonActivity = vi.fn().mockResolvedValue({
      status: "unavailable",
      failure: { code: "feed_unavailable", detail: "The activity feed is not being served.", recovery: "Retry shortly." },
    });
    render(<ActivityView {...props} />);

    expect(await screen.findByText("The activity feed is not being served. Retry shortly.")).toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("keeps a transport failure retryable inside the feed panel", async () => {
    const props = await activityProps();
    props.bridge.daemonActivity = vi.fn().mockRejectedValue(new Error("daemon socket unavailable"));
    render(<ActivityView {...props} />);

    expect(await screen.findByRole("alert")).toHaveTextContent("daemon socket unavailable");
    expect(screen.getByRole("button", { name: "Refresh activity" })).toBeEnabled();
  });

  it("shows a calm paused state offline and wires the start control", async () => {
    const user = userEvent.setup();
    const props = await activityProps("offline");
    const spy = vi.spyOn(props.bridge, "daemonActivity");
    render(<ActivityView {...props} />);

    expect(screen.getByRole("heading", { name: "PAM is paused" })).toBeInTheDocument();
    expect(screen.getByText(/pick up where it left off/)).toBeInTheDocument();
    expect(spy).not.toHaveBeenCalled();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Start PAM" }));
    expect(props.onStartDaemon).toHaveBeenCalledTimes(1);
  });

  it("formats model sizes in human units", () => {
    expect(formatModelSize(19_500_000_000)).toBe("19.5 GB");
    expect(formatModelSize(2_800_000_000)).toBe("2.8 GB");
    expect(formatModelSize(850_000_000)).toBe("850 MB");
    expect(formatModelSize(512)).toBe("512 bytes");
  });

  it("shows the loaded local model with size, pill, and chat entry point", async () => {
    const user = userEvent.setup();
    const props = await activityProps("solved", loadedStatus);
    render(<ActivityView {...props} />);

    expect(screen.getByText("qwen/qwen3-14b-instruct-q4")).toBeInTheDocument();
    expect(screen.getByText("19.5 GB")).toBeInTheDocument();
    expect(screen.getByText("loaded")).toBeInTheDocument();
    await waitFor(() => expect(props.onReloadModel).toHaveBeenCalled());

    await user.click(screen.getByRole("button", { name: "Chat" }));
    expect(props.onOpenModelChat).toHaveBeenCalledWith("qwen/qwen3-14b-instruct-q4", expect.any(HTMLElement));
  });

  it("shows a registered-but-not-loaded model as on deck with chat available", async () => {
    const props = await activityProps("solved", {
      status: "ok",
      loaded: null,
      registered: [{ modelId: "qwen/qwen3-4b-instruct-q4", sizeBytes: 2_800_000_000 }],
    });
    render(<ActivityView {...props} />);

    expect(screen.getByText("qwen/qwen3-4b-instruct-q4")).toBeInTheDocument();
    expect(screen.getByText("on deck")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Chat" })).toBeEnabled();
  });

  it("shows a calm empty model state pointing at the Control Center, never a CLI command", async () => {
    const props = await activityProps("solved", { status: "ok", loaded: null, registered: [] });
    render(<ActivityView {...props} />);

    expect(screen.getByText("No local model yet")).toBeInTheDocument();
    expect(screen.getByText(/Import one from the Control Center/)).toBeInTheDocument();
    expect(screen.queryByText(/pam model import/)).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Chat" })).not.toBeInTheDocument();
  });

  it("lists the latest run evidence for the active project and opens a handle", async () => {
    const user = userEvent.setup();
    const props = await activityProps();
    render(<ActivityView {...props} />);

    const panel = screen.getByRole("region", { name: "Latest run evidence" });
    expect(props.evidence?.handles.length).toBeGreaterThan(0);
    const opener = within(panel).getByRole("button", { name: "Open Evidence 1" });
    expect(opener).toHaveAccessibleDescription(props.evidence!.handles[0]);
    await user.click(opener);
    expect(props.onEvidence).toHaveBeenCalledWith(props.evidence!.handles[0]);
  });

  it("omits the evidence panel without an active project outcome", async () => {
    const props = await activityProps("global-only");
    render(<ActivityView {...props} />);

    expect(props.evidence).toBeNull();
    expect(screen.queryByRole("region", { name: "Latest run evidence" })).not.toBeInTheDocument();
  });

  it("shows blocked and unavailable model failures without a chat entry point", async () => {
    const props = await activityProps("solved", {
      status: "blocked",
      failure: { kind: "blocked", code: "model_status_blocked", detail: "Model status is blocked by project policy.", recovery: null },
    });
    const { unmount } = render(<ActivityView {...props} />);
    expect(screen.getByText("Model status is blocked by project policy.")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Chat" })).not.toBeInTheDocument();
    unmount();

    const offlineProps = await activityProps("offline", {
      status: "unavailable",
      failure: { kind: "unavailable", code: "daemon_offline", detail: "PAM is paused, so the local model runtime is not reachable.", recovery: null },
    });
    render(<ActivityView {...offlineProps} />);
    expect(screen.getByText(/local model runtime is not reachable/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Chat" })).not.toBeInTheDocument();
    expect(offlineProps.onReloadModel).toHaveBeenCalled();
  });

  it("reloads the model card together with the feed refresh", async () => {
    const user = userEvent.setup();
    const props = await activityProps("solved", loadedStatus);
    render(<ActivityView {...props} />);
    await screen.findByText("project.current");
    const mountCalls = props.onReloadModel.mock.calls.length;

    await user.click(screen.getByRole("button", { name: "Refresh activity" }));
    await waitFor(() => expect(props.onReloadModel.mock.calls.length).toBeGreaterThan(mountCalls));
  });
});
