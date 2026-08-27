import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { withDaemonOperation } from "../bridge";
import { fixtureBridge, type FixtureScenario } from "../fixtures";
import { selectDaemonView } from "../selectors";
import { CallerRequestsPanel, CallersPanel, aggregateCallerRequests } from "./CallersPanel";

function callersProps(scenario: FixtureScenario = "solved") {
  return { bridge: fixtureBridge(scenario) };
}

async function requestsProps(scenario: FixtureScenario = "solved") {
  const bridge = fixtureBridge(scenario);
  return {
    bridge,
    daemon: selectDaemonView(await bridge.daemonHealth(withDaemonOperation())),
    registrationNeeded: false,
    registrationBusy: false,
    onRegisterCaller: vi.fn(),
  };
}

describe("CallersPanel", () => {
  it("lists registered callers with registration dates and revoked badges", async () => {
    const props = callersProps();
    render(<CallersPanel {...props} />);

    expect(await screen.findByText("gui:pam-desktop")).toBeInTheDocument();
    expect(screen.getByText("cli:release-agent")).toBeInTheDocument();
    const revokedRow = screen.getByText("cli:retired-agent").closest("article");
    expect(revokedRow).not.toBeNull();
    expect(within(revokedRow!).getByText("revoked")).toBeInTheDocument();
    expect(screen.getAllByText("active")).toHaveLength(4);
    expect(screen.getAllByText(/^Registered .*\d{4}$/)).toHaveLength(5);
    // A caller with a declared kind gets a badge and a shortened UUID.
    expect(screen.getByText("GUI")).toBeInTheDocument();
    expect(screen.getByText("CLI")).toBeInTheDocument();
    expect(screen.getByTitle("8f14e45f-ceea-467e-adc9-15794b520d1d")).toHaveTextContent("GUI 8f14e45f…");
  });

  it("refreshes the caller registry on demand", async () => {
    const user = userEvent.setup();
    const props = callersProps();
    const spy = vi.spyOn(props.bridge, "callerRegistry");
    render(<CallersPanel {...props} />);
    await screen.findByText("gui:pam-desktop");
    expect(spy).toHaveBeenCalledTimes(1);

    await user.click(screen.getByRole("button", { name: "Refresh callers" }));
    await waitFor(() => expect(spy).toHaveBeenCalledTimes(2));
    expect(spy.mock.calls[0][0]).toMatchObject({ projectHandle: "daemon", generation: "daemon" });
    expect(spy.mock.calls[1][0]).toMatchObject({ projectHandle: "daemon", generation: "daemon" });
    expect(spy.mock.calls[0][0].operationId).not.toBe(spy.mock.calls[1][0].operationId);
  });

  it("serves the caller registry with zero projects", async () => {
    const props = callersProps("global-only");
    render(<CallersPanel {...props} />);

    expect(await screen.findByText("gui:pam-desktop")).toBeInTheDocument();
  });

  it("explains the paused registry while the daemon is offline", async () => {
    const props = callersProps("offline");
    render(<CallersPanel {...props} />);

    expect(await screen.findByText(/caller registry is not being served/)).toBeInTheDocument();
    expect(screen.getByText(/Start PAM to read the registered callers/)).toBeInTheDocument();
  });
});

// Issue #39: a legacy caller (no recorded kind) used to render its raw
// 36-character ID, so two of them were indistinguishable on a real machine.
describe("CallerLabel", () => {
  it("shortens a long ID with or without a kind badge, keeping the full ID as its tooltip", async () => {
    const bridge = fixtureBridge();
    const registry = await bridge.callerRegistry(withDaemonOperation());
    if (registry.status !== "ok") throw new Error("fixture registry unavailable");
    const legacyUuid = "5c4e2b7a-1111-4222-8333-444444444444";
    vi.spyOn(bridge, "callerRegistry").mockResolvedValue({
      status: "ok",
      callers: [
        ...registry.callers,
        { callerId: legacyUuid, registeredAtMs: 1_776_000_000_000, revokedAtMs: null, kind: null },
      ],
    });
    render(<CallersPanel bridge={bridge} />);

    const legacy = await screen.findByTitle(legacyUuid);
    expect(legacy).toHaveTextContent("5c4e2b7a…");
    expect(legacy).not.toHaveTextContent(legacyUuid);
    // A short, readable ID is not mangled by the same rule.
    expect(screen.getByTitle("gui:pam-desktop")).toHaveTextContent("gui:pam-desktop");
  });
});

describe("CallerRequestsPanel", () => {
  it("shows recent daemon requests grouped by caller", async () => {
    const props = await requestsProps();
    render(<CallerRequestsPanel {...props} />);

    const panel = await screen.findByRole("region", { name: "Requests per caller" });
    expect(await within(panel).findByText("gui:pam-desktop")).toBeInTheDocument();
    expect(within(panel).getByText("cli:release-agent")).toBeInTheDocument();
    // Two fixture events each.
    expect(within(panel).getAllByText("2 requests recently")).toHaveLength(2);
    // The revoked caller stays visible with a zero count.
    expect(within(panel).getByText("cli:retired-agent")).toBeInTheDocument();
    expect(within(panel).getByText("0 requests recently")).toBeInTheDocument();
    expect(within(panel).getByText("revoked")).toBeInTheDocument();
  });

  it("offers GUI caller registration inside the caller panel when needed", async () => {
    const props = await requestsProps();
    render(<CallerRequestsPanel {...props} registrationNeeded />);

    await userEvent.click(screen.getByRole("button", { name: "Register GUI caller" }));
    expect(props.onRegisterCaller).toHaveBeenCalled();
  });

  it("stays calm while PAM is paused", async () => {
    const props = await requestsProps("offline");
    render(<CallerRequestsPanel {...props} />);

    expect(screen.getByText("PAM is paused, so no requests are being served.")).toBeInTheDocument();
  });
});

describe("caller request aggregation", () => {
  it("counts events per caller and keeps quiet registered callers", () => {
    const rows = aggregateCallerRequests(
      [
        { callerId: "gui:desktop", registeredAtMs: 1, revokedAtMs: null, kind: "gui" },
        { callerId: "cli:quiet", registeredAtMs: 2, revokedAtMs: 3, kind: null },
      ],
      [
        { sequence: 2, projectId: null, callerId: "gui:desktop", action: "daemon.status", decision: "allowed", outcome: "served", occurredAtMs: 5, projectRoot: null },
        { sequence: 1, projectId: null, callerId: "cli:unregistered", action: "flow.save", decision: "allowed", outcome: "served", occurredAtMs: 4, projectRoot: null },
      ],
    );
    expect(rows).toEqual([
      { callerId: "cli:unregistered", requests: 1, revoked: false, kind: null },
      { callerId: "gui:desktop", requests: 1, revoked: false, kind: "gui" },
      { callerId: "cli:quiet", requests: 0, revoked: true, kind: null },
    ]);
  });
});
