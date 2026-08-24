import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { withDaemonOperation } from "../bridge";
import { fixtureBridge, type FixtureScenario } from "../fixtures";
import { selectControlCenter, selectDaemonView } from "../selectors";
import { ConsoleView, filterEntries, formatConsoleLine } from "./ConsoleView";

async function consoleProps(scenario: FixtureScenario = "solved") {
  const bridge = fixtureBridge(scenario);
  const { snapshot, catalog } = await bridge.bootstrap();
  return {
    bridge,
    daemon: snapshot
      ? selectControlCenter(snapshot.data, catalog, true).daemon
      : selectDaemonView(await bridge.daemonHealth(withDaemonOperation())),
    pending: false,
    onStartDaemon: vi.fn(),
  };
}

describe("ConsoleView", () => {
  it("renders the daemon diagnostics oldest first", async () => {
    const props = await consoleProps();
    render(<ConsoleView {...props} />);

    expect(screen.getByRole("heading", { name: "Console" })).toBeInTheDocument();
    expect(await screen.findByText(/PAM daemon ready/)).toBeInTheDocument();
    expect(screen.getByText(/queued operation failed/)).toBeInTheDocument();
    expect(screen.getAllByText("info", { selector: ".state-pill" })).toHaveLength(2);
    expect(screen.getByText("warn", { selector: ".state-pill" })).toBeInTheDocument();
    expect(screen.getByText("error", { selector: ".state-pill" })).toBeInTheDocument();
  });

  it("filters by severity and copies the visible lines", async () => {
    const user = userEvent.setup();
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    const props = await consoleProps();
    render(<ConsoleView {...props} />);
    await screen.findByText(/PAM daemon ready/);

    await user.click(screen.getByRole("button", { name: "error" }));
    expect(screen.getByText(/queued operation failed/)).toBeInTheDocument();
    expect(screen.queryByText(/PAM daemon ready/)).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Copy visible console lines" }));
    await waitFor(() => expect(writeText).toHaveBeenCalledTimes(1));
    const copied = writeText.mock.calls[0][0] as string;
    expect(copied).toContain("ERROR queued operation failed");
    expect(copied).not.toContain("PAM daemon ready");
  });

  it("always loads the log under the exact daemon authority and refreshes on demand", async () => {
    const user = userEvent.setup();
    const props = await consoleProps();
    const spy = vi.spyOn(props.bridge, "daemonLogs");
    render(<ConsoleView {...props} />);
    await screen.findByText(/PAM daemon ready/);

    expect(spy.mock.calls[0][0]).toMatchObject({ projectHandle: "daemon", generation: "daemon" });
    await user.click(screen.getByRole("button", { name: "Refresh console" }));
    await waitFor(() => expect(spy).toHaveBeenCalledTimes(2));
  });

  it("offers a start affordance while PAM is paused", async () => {
    const user = userEvent.setup();
    const props = await consoleProps("offline");
    render(<ConsoleView {...props} />);

    expect(screen.getByRole("heading", { name: "PAM is paused" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /Start PAM/ }));
    expect(props.onStartDaemon).toHaveBeenCalledTimes(1);
  });

  it("explains an empty log without alarm", async () => {
    const props = await consoleProps("empty");
    render(<ConsoleView {...props} />);

    expect(await screen.findByText(/No diagnostics yet/)).toBeInTheDocument();
  });
});

describe("console helpers", () => {
  const entries = [
    { timestampMs: 1_000, severity: "info", message: "one" },
    { timestampMs: 2_000, severity: "error", message: "two" },
  ];

  it("filters entries by severity", () => {
    expect(filterEntries(entries, "all")).toHaveLength(2);
    expect(filterEntries(entries, "error")).toEqual([entries[1]]);
    expect(filterEntries(entries, "warn")).toEqual([]);
  });

  it("formats copyable lines with ISO clocks and upper-case severities", () => {
    expect(formatConsoleLine(entries[0])).toBe("1970-01-01T00:00:01.000Z INFO one");
  });
});
