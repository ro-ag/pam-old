import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { withDaemonOperation } from "../bridge";
import { fixtureBridge, type FixtureScenario } from "../fixtures";
import { selectControlCenter, selectDaemonView } from "../selectors";
import {
  HEATMAP_WEEKS,
  OverviewView,
  buildHeatmapWeeks,
  computeStreaks,
} from "./OverviewView";

const DAY_MS = 86_400_000;

async function overviewProps(scenario: FixtureScenario = "solved") {
  const bridge = fixtureBridge(scenario);
  const { snapshot, catalog } = await bridge.bootstrap();
  const control = snapshot ? selectControlCenter(snapshot.data, catalog, true) : null;
  return {
    bridge,
    daemon: control
      ? control.daemon
      : selectDaemonView(await bridge.daemonHealth(withDaemonOperation())),
    catalog: catalog.projects,
    modelStatus: await bridge.modelStatus(withDaemonOperation()),
    onOpenModels: vi.fn(),
  };
}

describe("OverviewView", () => {
  it("leads with the daemon overview and never offers project selection", async () => {
    const props = await overviewProps();
    render(<OverviewView {...props} />);

    expect(screen.getByRole("heading", { name: "Overview" })).toBeInTheDocument();
    const overview = screen.getByRole("region", { name: "Daemon overview" });
    expect(within(overview).getByText("Watch status")).toBeInTheDocument();
    expect(within(overview).getByText("Active days")).toBeInTheDocument();
    expect(await screen.findByRole("img", { name: /Daily daemon activity/ })).toBeInTheDocument();

    // The fleet overview is display-only and global: no picker, no switcher,
    // no single "active project" row on this screen.
    expect(screen.queryByRole("region", { name: "Active project" })).not.toBeInTheDocument();
    expect(screen.queryByText("Bring a queue into view")).not.toBeInTheDocument();

    // One watch-status source of truth per screen: the daemon overview row.
    expect(screen.getAllByText("Watch status")).toHaveLength(1);
  });

  it("shows the model as one read-only tile that navigates to Models", async () => {
    const props = await overviewProps();
    render(<OverviewView {...props} />);

    const overview = screen.getByRole("region", { name: "Daemon overview" });
    const tile = within(overview).getByRole("button", { name: /Local model/ });
    // The cell is too narrow for a vendor-qualified id — sharing its line with
    // the state pill left it ellipsised to "qwen…" — so the tile shows the
    // name and keeps the full id as the tooltip.
    const identity = within(tile).getByText("qwen3-14b-instruct-q4");
    expect(identity).toBeInTheDocument();
    expect(identity).toHaveAttribute("title", "qwen/qwen3-14b-instruct-q4");
    expect(within(tile).getByText("LOADED")).toBeInTheDocument();

    await userEvent.click(tile);
    expect(props.onOpenModels).toHaveBeenCalledTimes(1);
  });

  // Every model action lives on Models now; Overview only reports.
  it("keeps every model action off the overview", async () => {
    const props = await overviewProps();
    render(<OverviewView {...props} />);

    expect(screen.queryByRole("region", { name: "Model runtime" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Verify" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Chat" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Import model" })).not.toBeInTheDocument();
    expect(screen.queryByRole("progressbar")).not.toBeInTheDocument();
  });

  it("reports an unreachable model while Pam is paused", async () => {
    const props = await overviewProps("offline");
    render(<OverviewView {...props} />);

    const tile = screen.getByRole("button", { name: /Local model/ });
    expect(within(tile).getByText("UNREACHABLE")).toBeInTheDocument();
  });

  it("keeps the overview calm while Pam is paused", async () => {
    const props = await overviewProps("offline");
    render(<OverviewView {...props} />);

    expect(
      screen.getByText("The activity picture returns when Pam is back on watch."),
    ).toBeInTheDocument();
  });

  // Requests per caller moved to Access, with the callers it describes.
  it("no longer carries the per-caller request panel", async () => {
    const props = await overviewProps();
    render(<OverviewView {...props} />);

    expect(screen.queryByRole("region", { name: "Requests per caller" })).not.toBeInTheDocument();
  });
});

describe("Projects panel", () => {
  it("lists every catalog project plus usage, display-only, with zero-usage projects included", async () => {
    const props = await overviewProps();
    render(<OverviewView {...props} />);

    const panel = await screen.findByRole("region", { name: "Usage by project" });
    expect(within(panel).getByText("payments-api")).toBeInTheDocument();
    expect(within(panel).getByText("128 events")).toBeInTheDocument();
    expect(within(panel).getByText("ledger-web")).toBeInTheDocument();
    expect(within(panel).getByText("54 events")).toBeInTheDocument();
    // "docs" carries no usage fixture row but is still a catalog project.
    expect(within(panel).getByText("docs")).toBeInTheDocument();
    expect(within(panel).getByText("0 events")).toBeInTheDocument();
    // Display only: no click-through, no selector.
    expect(within(panel).queryAllByRole("button")).toHaveLength(0);
  });

  it("renders a usage row for a project outside the catalog under its truncated id when rootless", async () => {
    const props = await overviewProps();
    vi.spyOn(props.bridge, "daemonStats").mockResolvedValue({
      status: "ok",
      days: [],
      projects: [
        { projectId: "99999999-9999-4999-8999-999999999999", events: 3, lastEventMs: 1_777_000_000_000, root: null },
      ],
    });
    render(<OverviewView {...props} />);

    const panel = await screen.findByRole("region", { name: "Usage by project" });
    expect(await within(panel).findByText("99999999…")).toBeInTheDocument();
    expect(within(panel).getByText("3 events")).toBeInTheDocument();
  });

  it("renders a usage row for a project outside the catalog under its remembered root's basename", async () => {
    const props = await overviewProps();
    render(<OverviewView {...props} />);

    const panel = await screen.findByRole("region", { name: "Usage by project" });
    expect(await within(panel).findByText("scratch-agent")).toBeInTheDocument();
    expect(within(panel).getByText("/work/scratch-agent")).toBeInTheDocument();
    expect(within(panel).getByText("12 events")).toBeInTheDocument();
  });

  it("treats a missing projects field defensively as empty, for an older daemon", async () => {
    const props = await overviewProps();
    vi.spyOn(props.bridge, "daemonStats").mockResolvedValue({
      status: "ok",
      days: [],
    } as never);
    render(<OverviewView {...props} />);

    const panel = await screen.findByRole("region", { name: "Usage by project" });
    expect(within(panel).getByText("payments-api")).toBeInTheDocument();
    expect(within(panel).getAllByText("0 events").length).toBeGreaterThan(0);
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
