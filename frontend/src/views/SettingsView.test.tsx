import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { fixtureBridge, type FixtureScenario } from "../fixtures";
import { SettingsView } from "./SettingsView";

function settingsProps(scenario: FixtureScenario = "global-only") {
  return { bridge: fixtureBridge(scenario), onOpenConsole: vi.fn() };
}

describe("SettingsView", () => {
  it("renders storage locations and the logs section with zero project coupling", async () => {
    // "global-only" has no registered projects at all: Settings must still
    // render fully, proving it never depends on an active project.
    const props = settingsProps("global-only");
    render(<SettingsView {...props} />);

    expect(screen.getByRole("heading", { name: "Settings" })).toBeInTheDocument();
    expect(await screen.findByText("/Users/fixture/llm")).toBeInTheDocument();
    expect(screen.getByText("/Users/fixture/Library/Application Support/dev.PAM.PAM")).toBeInTheDocument();
    expect(screen.getByText("/Users/fixture/Library/Application Support/dev.PAM.PAM/logs")).toBeInTheDocument();
    expect(screen.getByText(/on disk today/)).toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: "Reveal" })).toHaveLength(3);
  });

  it("round-trips a custom models directory through settingsUpdate", async () => {
    const user = userEvent.setup();
    const props = settingsProps();
    const spy = vi.spyOn(props.bridge, "settingsUpdate");
    render(<SettingsView {...props} />);

    const input = await screen.findByLabelText("Change directory");
    await user.clear(input);
    await user.type(input, "/Volumes/external/models");
    await user.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => expect(spy).toHaveBeenCalledWith(expect.objectContaining({ projectHandle: "daemon", generation: "daemon" }), "/Volumes/external/models"));
    expect(await screen.findByText("/Volumes/external/models")).toBeInTheDocument();

    // Resetting to default clears the persisted override.
    await user.click(screen.getByRole("button", { name: "Reset to default" }));
    await waitFor(() => expect(spy).toHaveBeenLastCalledWith(expect.anything(), null));
    expect(await screen.findByText("/Users/fixture/llm")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Reset to default" })).not.toBeInTheDocument();
  });

  it("deletes on-disk logs only after an explicit confirmation step", async () => {
    const user = userEvent.setup();
    const props = settingsProps();
    const spy = vi.spyOn(props.bridge, "logsDelete");
    render(<SettingsView {...props} />);

    await screen.findByText(/on disk today/);
    await user.click(screen.getByRole("button", { name: "Delete logs" }));
    expect(spy).not.toHaveBeenCalled();
    expect(screen.getByText(/Delete PAM's on-disk log files\?/)).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Keep" }));
    expect(spy).not.toHaveBeenCalled();
    expect(screen.queryByText(/Delete PAM's on-disk log files\?/)).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Delete logs" }));
    await user.click(screen.getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(spy).toHaveBeenCalledTimes(1));
    expect(await screen.findByText(/^0 B on disk today/)).toBeInTheDocument();
  });

  it("opens Console from the logs explanation", async () => {
    const user = userEvent.setup();
    const props = settingsProps();
    render(<SettingsView {...props} />);

    await user.click(await screen.findByRole("button", { name: "Console" }));
    expect(props.onOpenConsole).toHaveBeenCalledTimes(1);
  });
});
