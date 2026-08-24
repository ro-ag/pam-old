import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { withDaemonOperation } from "../bridge";
import { fixtureBridge, type FixtureScenario } from "../fixtures";
import { selectControlCenter, selectDaemonView } from "../selectors";
import {
  ControlCenterView,
  HEATMAP_WEEKS,
  buildHeatmapWeeks,
  computeStreaks,
} from "./ControlCenterView";

const DAY_MS = 86_400_000;

async function controlCenterProps(scenario: FixtureScenario = "solved", withProject = true) {
  const bridge = fixtureBridge(scenario);
  const { snapshot, catalog } = await bridge.bootstrap();
  const control = snapshot ? selectControlCenter(snapshot.data, catalog, true) : null;
  return {
    bridge,
    daemon: control
      ? control.daemon
      : selectDaemonView(await bridge.daemonHealth(withDaemonOperation())),
    projects: catalog.projects,
    onSelectProject: vi.fn(),
    modelStatus: await bridge.modelStatus(withDaemonOperation()),
    modelBusy: false,
    onOpenModelChat: vi.fn(),
    onStartWithModel: vi.fn(),
    project:
      withProject && control
        ? {
            data: control,
            onCopy: vi.fn(),
            onEvidence: vi.fn(),
            onContinue: vi.fn(),
            onOpenQueue: vi.fn(),
            onOpenApproval: vi.fn(),
            onRecoverDaemon: vi.fn(),
            onRefresh: vi.fn(),
            onRegisterCaller: vi.fn(),
            registrationBusy: false,
          }
        : null,
  };
}

describe("ControlCenterView", () => {
  it("leads with the daemon overview and keeps the project in a second row", async () => {
    const props = await controlCenterProps();
    render(<ControlCenterView {...props} />);

    expect(screen.getByRole("heading", { name: "Control center" })).toBeInTheDocument();
    const overview = screen.getByRole("region", { name: "Daemon overview" });
    // The sidebar switcher already lists projects; the overview does not repeat the count.
    expect(within(overview).queryByText("Projects")).not.toBeInTheDocument();
    expect(within(overview).getByText("Watch status")).toBeInTheDocument();
    expect(within(overview).getByText("Active days")).toBeInTheDocument();
    expect(await screen.findByRole("img", { name: /Daily daemon activity/ })).toBeInTheDocument();

    // The active project keeps its content, demoted below the overview.
    expect(screen.getByRole("region", { name: "Active project" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Ready for the next agent" })).toBeInTheDocument();

    // One watch-status source of truth per screen: the daemon overview row.
    expect(screen.getAllByText("Watch status")).toHaveLength(1);
  });

  it("offers a compact project picker when no project is active", async () => {
    const props = await controlCenterProps("global-only", false);
    render(<ControlCenterView {...props} />);

    expect(screen.getByRole("region", { name: "Daemon overview" })).toBeInTheDocument();
    expect(screen.getByRole("region", { name: "Projects" })).toBeInTheDocument();
    expect(screen.queryByRole("region", { name: "Active project" })).not.toBeInTheDocument();
  });

  it("keeps the overview calm while PAM is paused", async () => {
    const props = await controlCenterProps("offline", false);
    render(<ControlCenterView {...props} />);

    expect(
      screen.getByText("The activity picture returns when PAM is back on watch."),
    ).toBeInTheDocument();
  });
});

describe("model runtime panel", () => {
  it("shows the loaded model with size and verifies it with a live round-trip", async () => {
    const props = await controlCenterProps();
    render(<ControlCenterView {...props} />);

    const panel = screen.getByRole("region", { name: "Model runtime" });
    expect(within(panel).getByText("loaded")).toBeInTheDocument();
    expect(within(panel).getByText("qwen/qwen3-14b-instruct-q4")).toBeInTheDocument();
    expect(within(panel).getByText(/19\.5 GB/)).toBeInTheDocument();

    await userEvent.click(within(panel).getByRole("button", { name: "Verify" }));
    expect(await within(panel).findByText(/Verified · \d+ ms/)).toBeInTheDocument();
  });

  it("reports a failed verification with the bounded failure detail", async () => {
    const props = await controlCenterProps("model-infer-blocked");
    render(<ControlCenterView {...props} />);

    const panel = screen.getByRole("region", { name: "Model runtime" });
    await userEvent.click(within(panel).getByRole("button", { name: "Verify" }));
    expect(
      await within(panel).findByText(/Project policy has not granted model\.infer/),
    ).toBeInTheDocument();
  });

  it("opens the chat from the panel in one click", async () => {
    const props = await controlCenterProps();
    render(<ControlCenterView {...props} />);

    await userEvent.click(
      within(screen.getByRole("region", { name: "Model runtime" })).getByRole("button", { name: "Chat" }),
    );
    expect(props.onOpenModelChat).toHaveBeenCalledWith(
      "qwen/qwen3-14b-instruct-q4",
      expect.any(HTMLElement),
    );
  });

  it("offers a restart with a registered model when nothing is loaded", async () => {
    const props = await controlCenterProps("model-on-deck");
    render(<ControlCenterView {...props} />);

    const panel = screen.getByRole("region", { name: "Model runtime" });
    expect(within(panel).getByText("on deck")).toBeInTheDocument();
    await userEvent.click(
      within(panel).getAllByRole("button", { name: /Restart PAM with this model/ })[0],
    );
    expect(props.onStartWithModel).toHaveBeenCalledWith("qwen/qwen3-14b-instruct-q4");
  });

  it("walks through the import steps when no model is registered", async () => {
    const props = await controlCenterProps("model-none");
    render(<ControlCenterView {...props} />);

    const panel = screen.getByRole("region", { name: "Model runtime" });
    expect(within(panel).getByText("none")).toBeInTheDocument();
    expect(within(panel).getByText(/pam model import/)).toBeInTheDocument();
    expect(within(panel).getByRole("button", { name: /Copy command/ })).toBeInTheDocument();
  });

  it("marks the runtime unreachable while PAM is paused", async () => {
    const props = await controlCenterProps("offline", false);
    render(<ControlCenterView {...props} />);

    const panel = screen.getByRole("region", { name: "Model runtime" });
    expect(within(panel).getByText("unreachable")).toBeInTheDocument();
    expect(within(panel).getByText(/local model runtime is not reachable/)).toBeInTheDocument();
  });
});

describe("overview helpers", () => {
  const today = 20_000 * DAY_MS;

  it("computes totals, active days, and streaks", () => {
    const days = [
      { dayStartMs: today - 3 * DAY_MS, events: 2 },
      { dayStartMs: today - 2 * DAY_MS, events: 5 },
      { dayStartMs: today - DAY_MS, events: 1 },
    ];
    const streaks = computeStreaks(days, today);
    expect(streaks.totalEvents).toBe(8);
    expect(streaks.activeDays).toBe(3);
    // Today is quiet, so the streak counts back from yesterday.
    expect(streaks.currentStreak).toBe(3);
    expect(streaks.longestStreak).toBe(3);
  });

  it("breaks the current streak on a gap", () => {
    const days = [
      { dayStartMs: today - 4 * DAY_MS, events: 3 },
      { dayStartMs: today, events: 1 },
    ];
    const streaks = computeStreaks(days, today);
    expect(streaks.currentStreak).toBe(1);
    expect(streaks.longestStreak).toBe(1);
  });

  it("lays out Sunday-aligned week columns with bounded intensity", () => {
    const days = [
      { dayStartMs: today, events: 8 },
      { dayStartMs: today - DAY_MS, events: 2 },
    ];
    const weeks = buildHeatmapWeeks(days, today);
    expect(weeks).toHaveLength(HEATMAP_WEEKS);
    for (const week of weeks) {
      expect(week).toHaveLength(7);
      // Every column starts on a Sunday (epoch day 0 was a Thursday).
      expect((Math.floor(week[0].dayStartMs / DAY_MS) + 4) % 7).toBe(0);
    }
    const cells = weeks.flat();
    const todayCell = cells.find((cell) => cell.dayStartMs === today);
    const yesterdayCell = cells.find((cell) => cell.dayStartMs === today - DAY_MS);
    expect(todayCell?.intensity).toBe(4);
    expect(yesterdayCell?.intensity).toBe(1);
    // The Sunday-aligned grid trims the window's leading days and hides the
    // trailing future days: visible cells run from the grid start to today.
    const todayWeekday = (Math.floor(today / DAY_MS) + 4) % 7;
    expect(cells.filter((cell) => cell.inWindow)).toHaveLength(
      (HEATMAP_WEEKS - 1) * 7 + todayWeekday + 1,
    );
    expect(cells.every((cell) => cell.intensity <= 4)).toBe(true);
  });
});
