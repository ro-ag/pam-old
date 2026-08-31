import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { withDaemonOperation } from "../bridge";
import { fixtureBridge, type FixtureScenario } from "../fixtures";
import { selectControlCenter, selectDaemonView } from "../selectors";
import { ActivityView, formatModelSize } from "./ActivityView";

async function activityProps(scenario: FixtureScenario = "solved") {
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
    evidence: control?.current.latestOutcome?.brief
      ? {
          projectName: control.project.name,
          handles: control.current.latestOutcome.brief.evidenceHandles,
          truncated: control.current.latestOutcome.brief.evidenceTruncated,
        }
      : null,
    onEvidence: vi.fn(),
    onStartDaemon: vi.fn(),
  };
}

describe("ActivityView", () => {
  it("renders daemon health and the bounded activity feed", async () => {
    const props = await activityProps();
    render(<ActivityView {...props} />);

    expect(screen.getByRole("heading", { name: "Activity" })).toBeInTheDocument();
    expect(screen.getByText("Running")).toBeInTheDocument();
    expect(screen.getByText("Daemon fixture-0.1.0")).toBeInTheDocument();
    expect(screen.getByText("Queue depth")).toBeInTheDocument();

    expect(await screen.findByText("project.current")).toBeInTheDocument();
    expect(screen.getByText(/gui:pam-desktop · payments-api · served/)).toBeInTheDocument();
    expect(screen.getByText(/cli:release-agent · ledger-web/)).toBeInTheDocument();
    expect(screen.getAllByText("allowed")).toHaveLength(5);
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

  it("names a daemon-scope event daemon rather than truncating the reserved id", async () => {
    const props = await activityProps();
    render(<ActivityView {...props} />);

    expect(await screen.findByText("daemon.status")).toBeInTheDocument();
    expect(screen.getByText(/gui:pam-desktop · daemon · served/)).toBeInTheDocument();
  });

  it("falls back to the remembered root when the catalog holds no project", async () => {
    const props = await activityProps("global-only");
    render(<ActivityView {...props} />);

    expect(props.projects).toHaveLength(0);
    expect(await screen.findByText("project.current")).toBeInTheDocument();
    const fallback = screen.getByText(/gui:pam-desktop · payments-api · served/);
    expect(fallback).toHaveAttribute("title", "/work/payments-api");
    expect(screen.getByText(/cli:daemon-only-rootless · 77777777…/)).toBeInTheDocument();
  });

  it("labels a project outside the catalog by its remembered root, and only truncates the id when rootless", async () => {
    const props = await activityProps();
    render(<ActivityView {...props} />);

    const rooted = await screen.findByText(/cli:daemon-only-rooted · scratch-agent/);
    expect(rooted).toHaveAttribute("title", "/work/scratch-agent");

    const rootless = screen.getByText(/cli:daemon-only-rootless · 77777777…/);
    expect(rootless).toHaveAttribute("title", "77777777-7777-4777-8777-777777777777");
  });

  it("names a catalog project by the root the daemon remembers, not the GUI handle", async () => {
    const props = await activityProps();
    render(<ActivityView {...props} />);

    // The event's project ID is the daemon's, never the catalog handle: the
    // name has to come from the root the two surfaces share.
    const payments = props.projects.find((project) => project.name === "payments-api");
    expect(payments?.handle).toBe("11111111-1111-4111-8111-111111111111");
    const named = await screen.findByText(/gui:pam-desktop · payments-api · served/);
    expect(named).toHaveAttribute("title", "/work/payments-api");
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

  it("hosts the daemon's debug console below the feed", async () => {
    const props = await activityProps();
    render(<ActivityView {...props} />);

    expect(await screen.findByRole("heading", { name: "Debug console" })).toBeInTheDocument();
    expect(await screen.findByText(/PAM daemon ready/)).toBeInTheDocument();
  });

  it("keeps the console out of the paused view, which owns that state", async () => {
    const props = await activityProps("offline");
    render(<ActivityView {...props} />);

    expect(screen.getByRole("heading", { name: "PAM is paused" })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Debug console" })).not.toBeInTheDocument();
  });

  // The model surface moved to Models; Activity keeps no tile and no chat.
  it("carries no local-model tile", async () => {
    const props = await activityProps();
    render(<ActivityView {...props} />);

    expect(screen.queryByText("Local model")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Chat" })).not.toBeInTheDocument();
  });
});
