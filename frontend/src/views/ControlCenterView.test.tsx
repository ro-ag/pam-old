import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { withDaemonOperation } from "../bridge";
import { fixtureBridge, type FixtureScenario } from "../fixtures";
import { selectControlCenter, selectDaemonView } from "../selectors";
import {
  ControlCenterView,
  HEATMAP_WEEKS,
  aggregateCallerRequests,
  buildHeatmapWeeks,
  computeStreaks,
} from "./ControlCenterView";

const DAY_MS = 86_400_000;

async function controlCenterProps(scenario: FixtureScenario = "solved") {
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
    modelBusy: false,
    onOpenModelChat: vi.fn(),
    onStartWithModel: vi.fn(),
    onModelImported: vi.fn(),
  };
}

describe("ControlCenterView", () => {
  it("leads with the daemon overview and never offers project selection", async () => {
    const props = await controlCenterProps();
    render(<ControlCenterView {...props} />);

    expect(screen.getByRole("heading", { name: "Control center" })).toBeInTheDocument();
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

  it("shows recent daemon requests grouped by caller", async () => {
    const props = await controlCenterProps();
    render(<ControlCenterView {...props} />);

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
    const props = await controlCenterProps();
    const onRegisterCaller = vi.fn();
    render(
      <ControlCenterView {...props} registrationNeeded registrationBusy={false} onRegisterCaller={onRegisterCaller} />,
    );

    await userEvent.click(screen.getByRole("button", { name: "Register GUI caller" }));
    expect(onRegisterCaller).toHaveBeenCalled();
  });

  it("keeps the overview calm while PAM is paused", async () => {
    const props = await controlCenterProps("offline");
    render(<ControlCenterView {...props} />);

    expect(
      screen.getByText("The activity picture returns when PAM is back on watch."),
    ).toBeInTheDocument();
    expect(screen.getByText("PAM is paused, so no requests are being served.")).toBeInTheDocument();
  });
});

describe("Projects panel", () => {
  it("lists every catalog project plus usage, display-only, with zero-usage projects included", async () => {
    const props = await controlCenterProps();
    render(<ControlCenterView {...props} />);

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

  it("renders a usage row for a project outside the catalog under its truncated id", async () => {
    const props = await controlCenterProps();
    vi.spyOn(props.bridge, "daemonStats").mockResolvedValue({
      status: "ok",
      days: [],
      projects: [
        { projectId: "99999999-9999-4999-8999-999999999999", events: 3, lastEventMs: 1_777_000_000_000 },
      ],
    });
    render(<ControlCenterView {...props} />);

    const panel = await screen.findByRole("region", { name: "Usage by project" });
    expect(await within(panel).findByText("99999999…")).toBeInTheDocument();
    expect(within(panel).getByText("3 events")).toBeInTheDocument();
  });

  it("treats a missing projects field defensively as empty, for an older daemon", async () => {
    const props = await controlCenterProps();
    vi.spyOn(props.bridge, "daemonStats").mockResolvedValue({
      status: "ok",
      days: [],
    } as never);
    render(<ControlCenterView {...props} />);

    const panel = await screen.findByRole("region", { name: "Usage by project" });
    expect(within(panel).getByText("payments-api")).toBeInTheDocument();
    expect(within(panel).getAllByText("0 events").length).toBeGreaterThan(0);
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

  it("imports a model entirely from the panel when none is registered", async () => {
    const props = await controlCenterProps("model-none");
    render(<ControlCenterView {...props} />);

    const panel = screen.getByRole("region", { name: "Model runtime" });
    expect(within(panel).getByText("none")).toBeInTheDocument();
    // The setup is UI-owned: no terminal command is ever shown.
    expect(within(panel).queryByText(/pam model import/)).not.toBeInTheDocument();

    const importButton = within(panel).getByRole("button", { name: "Import model" });
    expect(importButton).toBeDisabled();

    // License fields collapse behind Advanced by default.
    expect(within(panel).queryByLabelText("License identifier")).not.toBeInTheDocument();
    await userEvent.click(within(panel).getByRole("button", { name: /Advanced — license details/ }));

    await userEvent.type(
      within(panel).getByLabelText("GGUF file path"),
      "/models/qwen3-4b-instruct-q4.gguf",
    );
    await userEvent.type(within(panel).getByLabelText("Model identity"), "qwen/qwen3-4b-instruct-q4");
    await userEvent.type(within(panel).getByLabelText("License identifier"), "Apache-2.0");
    await userEvent.type(
      within(panel).getByLabelText("License URL"),
      "https://example.com/license",
    );
    await userEvent.type(within(panel).getByLabelText("License notice"), "Apache License 2.0");
    expect(importButton).toBeDisabled();
    await userEvent.click(
      within(panel).getByLabelText(/I accept this model's license/),
    );

    await userEvent.click(importButton);
    expect(props.onModelImported).toHaveBeenCalled();
  });

  it("surfaces a bounded import failure inside the panel", async () => {
    const props = await controlCenterProps("model-none");
    render(<ControlCenterView {...props} />);

    const panel = screen.getByRole("region", { name: "Model runtime" });
    await userEvent.click(within(panel).getByRole("button", { name: /Advanced — license details/ }));
    await userEvent.type(within(panel).getByLabelText("GGUF file path"), "not-a-path");
    await userEvent.type(within(panel).getByLabelText("Model identity"), "qwen/qwen3-4b-instruct-q4");
    await userEvent.type(within(panel).getByLabelText("License identifier"), "Apache-2.0");
    await userEvent.type(within(panel).getByLabelText("License URL"), "https://example.com/license");
    await userEvent.type(within(panel).getByLabelText("License notice"), "Apache License 2.0");
    await userEvent.click(within(panel).getByLabelText(/I accept this model's license/));

    await userEvent.click(within(panel).getByRole("button", { name: "Import model" }));
    expect(
      await within(panel).findByText(/must be an absolute path to a GGUF file/),
    ).toBeInTheDocument();
    expect(props.onModelImported).not.toHaveBeenCalled();
  });

  it("lists the curated presets from the fixture, with a fit hint per option", async () => {
    const props = await controlCenterProps("model-none");
    render(<ControlCenterView {...props} />);

    const panel = screen.getByRole("region", { name: "Model runtime" });
    await userEvent.click(await within(panel).findByRole("button", { name: "Choose a model" }));

    const menu = await screen.findByRole("menu");
    expect(within(menu).getByText("Qwen3 Coder 30B — minimum")).toBeInTheDocument();
    expect(within(menu).getByText("Qwen3 Coder 30B — balanced")).toBeInTheDocument();
    expect(within(menu).getByText("Qwen3 Coder 30B — high fidelity")).toBeInTheDocument();
    // The fixture host has 24 GB: only the minimum quant fits; the larger two do not.
    expect(within(menu).getAllByText("Runs on this Mac")).toHaveLength(1);
    expect(within(menu).getByText(/Needs ~25\.3 GB memory; this Mac has 24\.0 GB/)).toBeInTheDocument();
    expect(within(menu).getByText(/Needs ~33\.5 GB memory; this Mac has 24\.0 GB/)).toBeInTheDocument();
  });

  it("gates the preset download button on the license checkbox", async () => {
    const props = await controlCenterProps("model-none");
    render(<ControlCenterView {...props} />);

    const panel = screen.getByRole("region", { name: "Model runtime" });
    await userEvent.click(await within(panel).findByRole("button", { name: "Choose a model" }));
    await userEvent.click(await screen.findByRole("menuitemradio", { name: /Qwen3 Coder 30B — minimum/ }));

    expect(within(panel).getByText("qwen/qwen3-coder-30b-a3b-instruct-q4_k_s")).toBeInTheDocument();
    expect(within(panel).getByText(/Apache License, Version 2\.0/)).toBeInTheDocument();
    const downloadButton = within(panel).getByRole("button", { name: "Download" });
    expect(downloadButton).toBeDisabled();

    await userEvent.click(within(panel).getAllByLabelText(/I accept this model's license/)[0]);
    expect(downloadButton).toBeEnabled();
  });

  it("disables download for a preset that does not fit this Mac's memory", async () => {
    const props = await controlCenterProps("model-none");
    render(<ControlCenterView {...props} />);

    const panel = screen.getByRole("region", { name: "Model runtime" });
    await userEvent.click(await within(panel).findByRole("button", { name: "Choose a model" }));
    await userEvent.click(await screen.findByRole("menuitemradio", { name: /Qwen3 Coder 30B — balanced/ }));

    expect(
      within(panel).getByText(/Needs ~25\.3 GB memory; this Mac has 24\.0 GB/),
    ).toBeInTheDocument();
    await userEvent.click(within(panel).getAllByLabelText(/I accept this model's license/)[0]);
    expect(within(panel).getByRole("button", { name: "Download" })).toBeDisabled();
  });

  it("downloads a preset with polled progress, then registers it", async () => {
    const props = await controlCenterProps("model-none");
    render(<ControlCenterView {...props} />);

    const panel = screen.getByRole("region", { name: "Model runtime" });
    await userEvent.click(await within(panel).findByRole("button", { name: "Choose a model" }));
    await userEvent.click(await screen.findByRole("menuitemradio", { name: /Qwen3 Coder 30B — minimum/ }));
    await userEvent.click(within(panel).getAllByLabelText(/I accept this model's license/)[0]);
    await userEvent.click(within(panel).getByRole("button", { name: "Download" }));

    // The fixture bridge advances the download 40% of its total per poll,
    // on an ~800ms interval, so later assertions need a longer wait window.
    expect(await within(panel).findByText(/40%/)).toBeInTheDocument();
    expect(await within(panel).findByText(/80%/, {}, { timeout: 2_000 })).toBeInTheDocument();
    expect(
      await within(panel).findByText("Downloaded and registered.", {}, { timeout: 2_000 }),
    ).toBeInTheDocument();
    expect(props.onModelImported).toHaveBeenCalledTimes(1);
  }, 10_000);

  it("shows a retry after a failed preset download", async () => {
    const props = await controlCenterProps("model-none");
    vi.spyOn(props.bridge, "modelDownloadStatus")
      .mockResolvedValueOnce({ status: "running", presetId: "qwen3-coder-30b-q4ks", receivedBytes: 1_000, totalBytes: 17_456_012_448 })
      .mockResolvedValueOnce({
        status: "failed",
        presetId: "qwen3-coder-30b-q4ks",
        receivedBytes: 1_000,
        totalBytes: 17_456_012_448,
        failure: { code: "connection_reset", detail: "The download connection dropped.", recovery: "Check the network and retry." },
      });
    render(<ControlCenterView {...props} />);

    const panel = screen.getByRole("region", { name: "Model runtime" });
    await userEvent.click(await within(panel).findByRole("button", { name: "Choose a model" }));
    await userEvent.click(await screen.findByRole("menuitemradio", { name: /Qwen3 Coder 30B — minimum/ }));
    await userEvent.click(within(panel).getAllByLabelText(/I accept this model's license/)[0]);
    await userEvent.click(within(panel).getByRole("button", { name: "Download" }));

    expect(
      await within(panel).findByText(
        /The download connection dropped\. Check the network and retry\./,
        {},
        { timeout: 2_000 },
      ),
    ).toBeInTheDocument();
    expect(within(panel).getByRole("button", { name: "Retry download" })).toBeInTheDocument();
    expect(props.onModelImported).not.toHaveBeenCalled();
  }, 10_000);

  it("marks the runtime unreachable while PAM is paused", async () => {
    const props = await controlCenterProps("offline");
    render(<ControlCenterView {...props} />);

    const panel = screen.getByRole("region", { name: "Model runtime" });
    expect(within(panel).getByText("unreachable")).toBeInTheDocument();
    expect(within(panel).getByText(/local model runtime is not reachable/)).toBeInTheDocument();
  });
});

describe("caller request aggregation", () => {
  it("counts events per caller and keeps quiet registered callers", () => {
    const rows = aggregateCallerRequests(
      [
        { callerId: "gui:desktop", registeredAtMs: 1, revokedAtMs: null },
        { callerId: "cli:quiet", registeredAtMs: 2, revokedAtMs: 3 },
      ],
      [
        { sequence: 2, projectId: null, callerId: "gui:desktop", action: "daemon.status", decision: "allowed", outcome: "served", occurredAtMs: 5 },
        { sequence: 1, projectId: null, callerId: "cli:unregistered", action: "flow.save", decision: "allowed", outcome: "served", occurredAtMs: 4 },
      ],
    );
    expect(rows).toEqual([
      { callerId: "cli:unregistered", requests: 1, revoked: false },
      { callerId: "gui:desktop", requests: 1, revoked: false },
      { callerId: "cli:quiet", requests: 0, revoked: true },
    ]);
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
