import { render, screen, within } from "@testing-library/react";
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
    expect(within(overview).getByText("Projects")).toBeInTheDocument();
    expect(within(overview).getByText("3")).toBeInTheDocument();
    expect(within(overview).getByText("Watch status")).toBeInTheDocument();
    expect(within(overview).getByText("Active days")).toBeInTheDocument();
    expect(await screen.findByRole("img", { name: /Daily daemon activity/ })).toBeInTheDocument();

    // The active project keeps its content, demoted below the overview.
    expect(screen.getByRole("region", { name: "Active project" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Ready for the next agent" })).toBeInTheDocument();
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
