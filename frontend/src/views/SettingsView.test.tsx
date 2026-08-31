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
    expect(screen.getByText("/Users/fixture/Library/Application Support/dev.PAM.PAM/.pam/flows")).toBeInTheDocument();
    expect(screen.getByText("/Users/fixture/Library/Application Support/dev.PAM.PAM/logs")).toBeInTheDocument();
    expect(screen.getByText(/on disk today/)).toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: "Reveal" })).toHaveLength(4);
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

describe("SettingsView danger zone", () => {
  it("shows a tier's dry-run counts and bytes before its confirm arms", async () => {
    const user = userEvent.setup();
    const props = settingsProps();
    const spy = vi.spyOn(props.bridge, "resetAccess");
    render(<SettingsView {...props} />);

    await screen.findByText(/on disk today/);
    expect(screen.queryByRole("button", { name: "Reset access" })).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: /Preview reset access/ }));

    // The blast radius is on screen before anything can be confirmed.
    const preview = await screen.findByTestId("reset-preview-access");
    expect(preview).toHaveTextContent("9 items");
    expect(preview).toHaveTextContent("grants 6");
    expect(preview).toHaveTextContent("approvals 2");
    expect(spy).toHaveBeenCalledTimes(1);
    expect(spy).toHaveBeenCalledWith(
      expect.objectContaining({ projectHandle: "daemon", generation: "daemon" }),
      true,
    );

    await user.click(screen.getByRole("button", { name: "Reset access" }));
    await waitFor(() => expect(spy).toHaveBeenCalledTimes(2));
    expect(spy).toHaveBeenLastCalledWith(expect.anything(), false);
    expect(await screen.findByText(/Removed 9 items/)).toBeInTheDocument();
  });

  it("keeps every tier on its own bridge call", async () => {
    const user = userEvent.setup();
    const props = settingsProps();
    const identity = vi.spyOn(props.bridge, "resetIdentity");
    const history = vi.spyOn(props.bridge, "resetHistory");
    const registry = vi.spyOn(props.bridge, "resetRegistry");
    render(<SettingsView {...props} />);

    await screen.findByText(/on disk today/);
    await user.click(screen.getByRole("button", { name: /Preview clear history/ }));
    await screen.findByTestId("reset-preview-history");
    expect(history).toHaveBeenCalledTimes(1);
    expect(identity).not.toHaveBeenCalled();
    expect(registry).not.toHaveBeenCalled();
  });

  it("dismissing a preview disarms the confirm without calling the bridge again", async () => {
    const user = userEvent.setup();
    const props = settingsProps();
    const spy = vi.spyOn(props.bridge, "resetRegistry");
    render(<SettingsView {...props} />);

    await screen.findByText(/on disk today/);
    await user.click(screen.getByRole("button", { name: /Preview reset the model registry/ }));
    await screen.findByTestId("reset-preview-registry");

    await user.click(screen.getByRole("button", { name: "Keep" }));
    expect(screen.queryByTestId("reset-preview-registry")).not.toBeInTheDocument();
    expect(spy).toHaveBeenCalledTimes(1);
  });

  it("requires the typed word before a factory reset can fire", async () => {
    const user = userEvent.setup();
    const props = settingsProps("offline");
    const spy = vi.spyOn(props.bridge, "factoryReset");
    render(<SettingsView {...props} />);

    await screen.findByText(/on disk today/);
    await user.click(screen.getByRole("button", { name: /Preview factory reset/ }));

    const preview = await screen.findByTestId("reset-preview-factory");
    // The flow library is named, so the typed confirmation is informed.
    expect(preview).toHaveTextContent("release-readiness.toml");
    expect(preview).toHaveTextContent("flows 2");

    const confirm = screen.getByRole("button", { name: "Factory reset" });
    expect(confirm).toBeDisabled();
    await user.type(screen.getByLabelText(/Type RESET to confirm/), "reset");
    expect(confirm).toBeDisabled();

    await user.clear(screen.getByLabelText(/Type RESET to confirm/));
    await user.type(screen.getByLabelText(/Type RESET to confirm/), "RESET");
    expect(confirm).toBeEnabled();
    await user.click(confirm);
    await waitFor(() => expect(spy).toHaveBeenCalledTimes(2));
    expect(spy).toHaveBeenLastCalledWith(expect.anything(), false, false);
    expect(await screen.findByText(/Receipt written to/)).toBeInTheDocument();
  });

  it("carries the weights opt-in into the factory preview", async () => {
    const user = userEvent.setup();
    const props = settingsProps("offline");
    const spy = vi.spyOn(props.bridge, "factoryReset");
    render(<SettingsView {...props} />);

    await screen.findByText(/on disk today/);
    await user.click(screen.getByLabelText(/Also delete the weights/));
    await user.click(screen.getByRole("button", { name: /Preview factory reset/ }));

    expect(spy).toHaveBeenCalledWith(expect.anything(), true, true);
    expect(await screen.findByTestId("reset-preview-factory")).toHaveTextContent("model_weights 2");
  });

  it("renders a refusal with its recovery line and never arms the confirm", async () => {
    const user = userEvent.setup();
    const props = settingsProps("reset-blocked");
    render(<SettingsView {...props} />);

    await screen.findByText(/on disk today/);
    await user.click(screen.getByRole("button", { name: /Preview reset identity/ }));

    const alert = await screen.findByText(/project policy denied this capability/);
    expect(alert).toHaveTextContent("pam access grant reset.identity --daemon");
    expect(screen.queryByTestId("reset-preview-identity")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Reset identity" })).not.toBeInTheDocument();
  });

  it("refuses a factory reset while PAM is running and shows how to stop it", async () => {
    const user = userEvent.setup();
    const props = settingsProps();
    render(<SettingsView {...props} />);

    await screen.findByText(/on disk today/);
    await user.click(screen.getByRole("button", { name: /Preview factory reset/ }));

    expect(await screen.findByText(/a running daemon still owns/)).toHaveTextContent("Stop PAM first");
    expect(screen.queryByTestId("reset-preview-factory")).not.toBeInTheDocument();
  });
});
