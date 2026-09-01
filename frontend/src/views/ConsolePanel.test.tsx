import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { fixtureBridge, type FixtureScenario } from "../fixtures";
import { ConsolePanel, filterEntries, formatConsoleLine } from "./ConsolePanel";

function consoleProps(scenario: FixtureScenario = "solved") {
  return { bridge: fixtureBridge(scenario) };
}

describe("ConsolePanel", () => {
  it("renders the daemon diagnostics oldest first", async () => {
    const props = consoleProps();
    render(<ConsolePanel {...props} />);

    expect(screen.getByRole("heading", { name: "Debug console" })).toBeInTheDocument();
    expect(await screen.findByText(/Pam daemon ready/)).toBeInTheDocument();
    expect(screen.getByText(/queued operation failed/)).toBeInTheDocument();
    expect(screen.getAllByText("info", { selector: ".state-pill" })).toHaveLength(2);
    expect(screen.getByText("warn", { selector: ".state-pill" })).toBeInTheDocument();
    expect(screen.getByText("error", { selector: ".state-pill" })).toBeInTheDocument();
  });

  it("filters by severity and copies the visible lines", async () => {
    const user = userEvent.setup();
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    const props = consoleProps();
    render(<ConsolePanel {...props} />);
    await screen.findByText(/Pam daemon ready/);

    await user.click(screen.getByRole("button", { name: "error" }));
    expect(screen.getByText(/queued operation failed/)).toBeInTheDocument();
    expect(screen.queryByText(/Pam daemon ready/)).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Copy visible console lines" }));
    await waitFor(() => expect(writeText).toHaveBeenCalledTimes(1));
    const copied = writeText.mock.calls[0][0] as string;
    expect(copied).toContain("ERROR queued operation failed");
    expect(copied).not.toContain("Pam daemon ready");
  });

  it("always loads the log under the exact daemon authority and refreshes on demand", async () => {
    const user = userEvent.setup();
    const props = consoleProps();
    const spy = vi.spyOn(props.bridge, "daemonLogs");
    render(<ConsolePanel {...props} />);
    await screen.findByText(/Pam daemon ready/);

    expect(spy.mock.calls[0][0]).toMatchObject({ projectHandle: "daemon", generation: "daemon" });
    await user.click(screen.getByRole("button", { name: "Refresh console" }));
    await waitFor(() => expect(spy).toHaveBeenCalledTimes(2));
  });

  it("explains an empty log without alarm", async () => {
    const props = consoleProps("empty");
    render(<ConsolePanel {...props} />);

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
