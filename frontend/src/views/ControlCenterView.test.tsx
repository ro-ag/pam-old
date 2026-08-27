import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

// The dialog plugin only exists in the native shell; tests stub the module so
// the Browse wiring is provable without Tauri.
const openDialog = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: openDialog }));
import { withDaemonOperation } from "../bridge";
import type { ModelStatusDto } from "../domain";
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

// Drives the manual GGUF import form to a submittable state.
async function fillManualImport(panel: HTMLElement) {
  await userEvent.click(within(panel).getByRole("button", { name: /Advanced — license details/ }));
  await userEvent.type(within(panel).getByLabelText("GGUF file path"), "/models/qwen3-4b-instruct-q4.gguf");
  await userEvent.type(within(panel).getByLabelText("Model identity"), "qwen/qwen3-4b-instruct-q4");
  await userEvent.type(within(panel).getByLabelText("License identifier"), "Apache-2.0");
  await userEvent.type(within(panel).getByLabelText("License URL"), "https://example.com/license");
  await userEvent.type(within(panel).getByLabelText("License notice"), "Apache License 2.0");
  await userEvent.click(within(panel).getByLabelText(/I accept this model's license/));
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

  it("renders a usage row for a project outside the catalog under its truncated id when rootless", async () => {
    const props = await controlCenterProps();
    vi.spyOn(props.bridge, "daemonStats").mockResolvedValue({
      status: "ok",
      days: [],
      projects: [
        { projectId: "99999999-9999-4999-8999-999999999999", events: 3, lastEventMs: 1_777_000_000_000, root: null },
      ],
    });
    render(<ControlCenterView {...props} />);

    const panel = await screen.findByRole("region", { name: "Usage by project" });
    expect(await within(panel).findByText("99999999…")).toBeInTheDocument();
    expect(within(panel).getByText("3 events")).toBeInTheDocument();
  });

  it("renders a usage row for a project outside the catalog under its remembered root's basename", async () => {
    const props = await controlCenterProps();
    render(<ControlCenterView {...props} />);

    const panel = await screen.findByRole("region", { name: "Usage by project" });
    expect(await within(panel).findByText("scratch-agent")).toBeInTheDocument();
    expect(within(panel).getByText("/work/scratch-agent")).toBeInTheDocument();
    expect(within(panel).getByText("12 events")).toBeInTheDocument();
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

    // The import runs in the background and is polled: hashing progress
    // first, then the indeterminate registering stage, then completion. The
    // fixture advances the hash 40% of its total per ~800ms poll, so later
    // assertions need a longer wait window.
    expect(await within(panel).findByText(/Hashing .* · 40%/)).toBeInTheDocument();
    expect(
      await within(panel).findByText("Registering — verifying the copy…", {}, { timeout: 3_000 }),
    ).toBeInTheDocument();
    await waitFor(() => expect(props.onModelImported).toHaveBeenCalled(), { timeout: 3_000 });
  }, 10_000);

  it("reattaches to an in-flight import on mount instead of assuming idle", async () => {
    const props = await controlCenterProps("model-none");
    vi.spyOn(props.bridge, "modelImportStatus").mockResolvedValue({
      status: "running",
      model: "vendor/model",
      stage: "hashing",
      hashedBytes: 6_000_000_000,
      totalBytes: 18_000_000_000,
      calibrated: true,
      failure: null,
    });
    render(<ControlCenterView {...props} />);

    const panel = screen.getByRole("region", { name: "Model runtime" });
    expect(
      await within(panel).findByRole("button", { name: "Verifying and registering…" }),
    ).toBeDisabled();
    expect(await within(panel).findByText(/Hashing .* · 33%/)).toBeInTheDocument();
  });

  it("surfaces a failure reported by the import status poll", async () => {
    const props = await controlCenterProps("model-none");
    vi.spyOn(props.bridge, "modelImportStatus")
      // The mount-time reattach check: nothing running yet.
      .mockResolvedValueOnce({ status: "idle", model: null, stage: null, hashedBytes: 0, totalBytes: 0, calibrated: true, failure: null })
      .mockResolvedValue({
        status: "failed",
        model: "qwen/qwen3-4b-instruct-q4",
        stage: null,
        hashedBytes: 1_000,
        totalBytes: 4_600_000_000,
        calibrated: true,
        failure: { kind: "unavailable", code: "model_import_failed", detail: "The GGUF digest changed on disk.", recovery: "Import the file again." },
      });
    render(<ControlCenterView {...props} />);

    const panel = screen.getByRole("region", { name: "Model runtime" });
    await userEvent.click(within(panel).getByRole("button", { name: /Advanced — license details/ }));
    await userEvent.type(within(panel).getByLabelText("GGUF file path"), "/models/qwen3-4b-instruct-q4.gguf");
    await userEvent.type(within(panel).getByLabelText("Model identity"), "qwen/qwen3-4b-instruct-q4");
    await userEvent.type(within(panel).getByLabelText("License identifier"), "Apache-2.0");
    await userEvent.type(within(panel).getByLabelText("License URL"), "https://example.com/license");
    await userEvent.type(within(panel).getByLabelText("License notice"), "Apache License 2.0");
    await userEvent.click(within(panel).getByLabelText(/I accept this model's license/));
    await userEvent.click(within(panel).getByRole("button", { name: "Import model" }));

    expect(
      await within(panel).findByText(/The GGUF digest changed on disk\. Import the file again\./),
    ).toBeInTheDocument();
    expect(props.onModelImported).not.toHaveBeenCalled();
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

  it("warns when a completed manual import is outside PAM's calibrated set", async () => {
    const props = await controlCenterProps("model-none");
    vi.spyOn(props.bridge, "modelImportStatus")
      // The mount-time reattach check: nothing running yet.
      .mockResolvedValueOnce({ status: "idle", model: null, stage: null, hashedBytes: 0, totalBytes: 0, calibrated: true, failure: null })
      .mockResolvedValue({
        status: "complete",
        model: "qwen/qwen3-4b-instruct-q4",
        stage: null,
        hashedBytes: 4_600_000_000,
        totalBytes: 4_600_000_000,
        calibrated: false,
        failure: null,
      });
    render(<ControlCenterView {...props} />);

    const panel = screen.getByRole("region", { name: "Model runtime" });
    await fillManualImport(panel);
    await userEvent.click(within(panel).getByRole("button", { name: "Import model" }));

    // The import still succeeds — the warning is additive, not a failure.
    expect(
      await within(panel).findByText(/not in PAM's calibrated set — it may fail to load under this Mac's runtime profile/),
    ).toBeInTheDocument();
    expect(props.onModelImported).toHaveBeenCalled();
  });

  it("does not warn when a completed manual import is calibrated", async () => {
    const props = await controlCenterProps("model-none");
    vi.spyOn(props.bridge, "modelImportStatus")
      .mockResolvedValueOnce({ status: "idle", model: null, stage: null, hashedBytes: 0, totalBytes: 0, calibrated: true, failure: null })
      .mockResolvedValue({
        status: "complete",
        model: "qwen/qwen3-4b-instruct-q4",
        stage: null,
        hashedBytes: 4_600_000_000,
        totalBytes: 4_600_000_000,
        calibrated: true,
        failure: null,
      });
    render(<ControlCenterView {...props} />);

    const panel = screen.getByRole("region", { name: "Model runtime" });
    await fillManualImport(panel);
    await userEvent.click(within(panel).getByRole("button", { name: "Import model" }));

    await waitFor(() => expect(props.onModelImported).toHaveBeenCalled());
    expect(within(panel).queryByText(/calibrated set/)).not.toBeInTheDocument();
  });

  it("re-fetches the mount-time loaders on a refresh tick without remounting the import form", async () => {
    const props = await controlCenterProps("model-none");
    const stats = vi.spyOn(props.bridge, "daemonStats");
    const presets = vi.spyOn(props.bridge, "modelPresets");
    const callers = vi.spyOn(props.bridge, "callerRegistry");
    const { rerender } = render(<ControlCenterView {...props} refreshTick={0} />);

    const panel = screen.getByRole("region", { name: "Model runtime" });
    await userEvent.type(within(panel).getByLabelText("Model identity"), "qwen/qwen3-4b-instruct-q4");
    await waitFor(() => expect(stats).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(presets).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(callers).toHaveBeenCalledTimes(1));

    rerender(<ControlCenterView {...props} refreshTick={1} />);

    await waitFor(() => expect(stats).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(presets).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(callers).toHaveBeenCalledTimes(2));
    // No remount: the in-progress form entry survives the refresh.
    expect(within(panel).getByLabelText("Model identity")).toHaveValue("qwen/qwen3-4b-instruct-q4");
  });

  it("inspects a typed path on blur and prefills identity from what came back", async () => {
    const props = await controlCenterProps("model-none");
    render(<ControlCenterView {...props} />);

    const panel = screen.getByRole("region", { name: "Model runtime" });
    await userEvent.type(
      within(panel).getByLabelText("GGUF file path"),
      "/models/qwen3-coder-30b-a3b-instruct.gguf",
    );
    await userEvent.tab();

    expect(await within(panel).findByText("qwen3-coder-30b-a3b-instruct.gguf")).toBeInTheDocument();
    expect(within(panel).getByText(/qwen3moe · Qwen3-Coder-30B-A3B-Instruct/)).toBeInTheDocument();
    expect(within(panel).getByLabelText("Model identity")).toHaveValue(
      "qwen3moe/qwen3-coder-30b-a3b-instruct",
    );
  });

  it("prefills the license from a known SPDX id in the GGUF header, without opening Advanced", async () => {
    const props = await controlCenterProps("model-none");
    render(<ControlCenterView {...props} />);

    const panel = screen.getByRole("region", { name: "Model runtime" });
    await userEvent.type(within(panel).getByLabelText("GGUF file path"), "/models/licensed.gguf");
    await userEvent.tab();
    await within(panel).findByText("licensed.gguf");

    expect(
      within(panel).getByRole("button", { name: /Advanced — license details/ }),
    ).toHaveAttribute("aria-expanded", "false");

    await userEvent.click(within(panel).getByRole("button", { name: /Advanced — license details/ }));
    expect(within(panel).getByLabelText("License identifier")).toHaveValue("Apache-2.0");
    expect(within(panel).getByLabelText("License URL")).toHaveValue(
      "https://www.apache.org/licenses/LICENSE-2.0",
    );
    expect(within(panel).getByLabelText("License notice")).toHaveValue(
      "licensed.gguf is distributed under the Apache-2.0 license at https://www.apache.org/licenses/LICENSE-2.0.",
    );
  });

  it("discovers a missing license on Hugging Face, narrated, and prefills it", async () => {
    const props = await controlCenterProps("model-none");
    render(<ControlCenterView {...props} />);

    const panel = screen.getByRole("region", { name: "Model runtime" });
    await userEvent.type(within(panel).getByLabelText("GGUF file path"), "/models/community.gguf");
    await userEvent.tab();
    await within(panel).findByText("community.gguf");

    // The lookup is narrated, names the repo it matched, and the raw
    // "apache-2.0" tag maps onto the canonical SPDX prefill.
    expect(
      await within(panel).findByText(/License found on Hugging Face \(the-community\/Qwen3-Coder-30B-A3B-Community\): Apache-2\.0/),
    ).toBeInTheDocument();
    await userEvent.click(within(panel).getByRole("button", { name: /Advanced — license details/ }));
    expect(within(panel).getByLabelText("License identifier")).toHaveValue("Apache-2.0");
    expect(within(panel).getByLabelText("License URL")).toHaveValue(
      "https://www.apache.org/licenses/LICENSE-2.0",
    );
    expect(within(panel).getByLabelText("License notice")).toHaveValue(
      "community.gguf is distributed under the Apache-2.0 license at https://www.apache.org/licenses/LICENSE-2.0.",
    );
  });

  it("stays quiet when discovery finds nothing and manual entry proceeds", async () => {
    const props = await controlCenterProps("model-none");
    const spy = vi.spyOn(props.bridge, "modelLicenseDiscover");
    render(<ControlCenterView {...props} />);

    const panel = screen.getByRole("region", { name: "Model runtime" });
    await userEvent.type(within(panel).getByLabelText("GGUF file path"), "/models/plain.gguf");
    await userEvent.tab();
    await within(panel).findByText("plain.gguf");

    // The ordinary fixture model resolves nothing on Hugging Face: the
    // lookup ran, no narration sticks, and the license fields stay empty
    // for manual entry.
    await waitFor(() => expect(spy).toHaveBeenCalled());
    expect(within(panel).queryByText(/License found on Hugging Face/)).not.toBeInTheDocument();
    await userEvent.click(within(panel).getByRole("button", { name: /Advanced — license details/ }));
    expect(within(panel).getByLabelText("License identifier")).toHaveValue("");
  });

  it("never overwrites a license identifier the user already typed", async () => {
    const props = await controlCenterProps("model-none");
    render(<ControlCenterView {...props} />);

    const panel = screen.getByRole("region", { name: "Model runtime" });
    await userEvent.click(within(panel).getByRole("button", { name: /Advanced — license details/ }));
    await userEvent.type(within(panel).getByLabelText("License identifier"), "MIT");
    await userEvent.type(within(panel).getByLabelText("GGUF file path"), "/models/licensed.gguf");
    await userEvent.tab();
    await within(panel).findByText("licensed.gguf");

    expect(within(panel).getByLabelText("License identifier")).toHaveValue("MIT");
    expect(within(panel).getByLabelText("License URL")).toHaveValue("");
  });

  it("warns when the inspected file falls below PAM's recommended floor", async () => {
    const props = await controlCenterProps("model-none");
    render(<ControlCenterView {...props} />);

    const panel = screen.getByRole("region", { name: "Model runtime" });
    await userEvent.type(within(panel).getByLabelText("GGUF file path"), "/models/tiny.gguf");
    await userEvent.tab();

    expect(
      await within(panel).findByText(/Below PAM's recommended minimum of 17\.0 GB/),
    ).toBeInTheDocument();
  });

  it("never overwrites a model identity the user already typed", async () => {
    const props = await controlCenterProps("model-none");
    render(<ControlCenterView {...props} />);

    const panel = screen.getByRole("region", { name: "Model runtime" });
    await userEvent.type(within(panel).getByLabelText("Model identity"), "custom/my-model");
    await userEvent.type(
      within(panel).getByLabelText("GGUF file path"),
      "/models/qwen3-coder-30b-a3b-instruct.gguf",
    );
    await userEvent.tab();

    await within(panel).findByText(/qwen3moe · Qwen3-Coder-30B-A3B-Instruct/);
    expect(within(panel).getByLabelText("Model identity")).toHaveValue("custom/my-model");
  });

  it("shows a calm note, not an alert, when inspection can't place the path", async () => {
    const props = await controlCenterProps("model-none");
    render(<ControlCenterView {...props} />);

    const panel = screen.getByRole("region", { name: "Model runtime" });
    await userEvent.type(within(panel).getByLabelText("GGUF file path"), "/models/notes.txt");
    await userEvent.tab();

    const note = await within(panel).findByText(/Point PAM at a downloaded \.gguf file/);
    expect(note).not.toHaveAttribute("role", "alert");
  });

  it("reveals the license fields and explains, instead of a dead Import button", async () => {
    const props = await controlCenterProps("model-none");
    render(<ControlCenterView {...props} />);

    const panel = screen.getByRole("region", { name: "Model runtime" });
    await userEvent.type(within(panel).getByLabelText("GGUF file path"), "/tmp/model.gguf");
    await userEvent.clear(within(panel).getByLabelText("Model identity"));
    await userEvent.type(within(panel).getByLabelText("Model identity"), "vendor/model");
    await userEvent.click(within(panel).getAllByLabelText(/I accept this model's license/).slice(-1)[0]);
    await userEvent.click(within(panel).getByRole("button", { name: "Import model" }));

    expect(await within(panel).findByRole("alert")).toHaveTextContent(/under Advanced/);
    // The disclosure opened so the named fields are visible for filling.
    expect(within(panel).getByLabelText("License identifier")).toBeVisible();
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
    // The fixture host is a 32 GB Mac — PAM's supported minimum — and every
    // curated quant fits it.
    expect(within(menu).getAllByText("Runs on this Mac")).toHaveLength(3);
    expect(screen.queryByText(/of memory or more; this Mac reports/)).not.toBeInTheDocument();
  });

  it("browses for a GGUF through the native dialog and inspects the pick", async () => {
    const props = await controlCenterProps("model-none");
    // Browse renders only in the native shell; the fixture bridge stays the
    // data source while the mode flag flips the button on.
    Object.defineProperty(props.bridge, "mode", { value: "native" });
    openDialog.mockResolvedValueOnce("/Users/rodox/models/Qwen3-Coder-30B-A3B-Instruct-Q4_K_S.gguf");
    render(<ControlCenterView {...props} />);

    const panel = screen.getByRole("region", { name: "Model runtime" });
    await userEvent.click(await within(panel).findByRole("button", { name: "Browse…" }));

    expect(openDialog).toHaveBeenCalledWith(
      expect.objectContaining({ multiple: false, filters: [{ name: "GGUF model", extensions: ["gguf"] }] }),
    );
    expect(await within(panel).findByText("Qwen3-Coder-30B-A3B-Instruct-Q4_K_S.gguf")).toBeInTheDocument();
    expect(within(panel).getByLabelText("GGUF file path")).toHaveValue(
      "/Users/rodox/models/Qwen3-Coder-30B-A3B-Instruct-Q4_K_S.gguf",
    );
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

    await userEvent.click(within(panel).getByLabelText(/I accept the .* license/));
    expect(downloadButton).toBeEnabled();
  });

  it("disables download and warns below the supported minimum on an undersized Mac", async () => {
    const props = await controlCenterProps("model-none");
    // A 24 GB machine: below PAM's supported 32 GB minimum.
    vi.spyOn(props.bridge, "hostMemory").mockResolvedValue({
      totalBytes: 25_769_803_776,
      supportedMinimumBytes: 34_359_738_368,
    });
    render(<ControlCenterView {...props} />);

    const panel = screen.getByRole("region", { name: "Model runtime" });
    expect(
      await within(panel).findByText(/32 GB of memory or more; this Mac reports 24 GB/),
    ).toBeInTheDocument();
    await userEvent.click(await within(panel).findByRole("button", { name: "Choose a model" }));
    await userEvent.click(await screen.findByRole("menuitemradio", { name: /Qwen3 Coder 30B — high fidelity/ }));

    expect(
      within(panel).getByText(/Needs ~31 GB memory; this Mac has 24 GB/),
    ).toBeInTheDocument();
    await userEvent.click(within(panel).getByLabelText(/I accept the .* license/));
    expect(within(panel).getByRole("button", { name: "Download" })).toBeDisabled();
  });

  it("downloads a preset with polled progress, then registers it", async () => {
    const props = await controlCenterProps("model-none");
    render(<ControlCenterView {...props} />);

    const panel = screen.getByRole("region", { name: "Model runtime" });
    await userEvent.click(await within(panel).findByRole("button", { name: "Choose a model" }));
    await userEvent.click(await screen.findByRole("menuitemradio", { name: /Qwen3 Coder 30B — minimum/ }));
    await userEvent.click(within(panel).getByLabelText(/I accept the .* license/));
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

  it("cancels a running download, keeps the partial bytes, and offers resume", async () => {
    const props = await controlCenterProps("model-none");
    render(<ControlCenterView {...props} />);

    const panel = screen.getByRole("region", { name: "Model runtime" });
    await userEvent.click(await within(panel).findByRole("button", { name: "Choose a model" }));
    await userEvent.click(await screen.findByRole("menuitemradio", { name: /Qwen3 Coder 30B — minimum/ }));
    await userEvent.click(within(panel).getByLabelText(/I accept the .* license/));
    await userEvent.click(within(panel).getByRole("button", { name: "Download" }));

    // Cancel mid-flight (the fixture advances 40% per ~800ms poll).
    expect(await within(panel).findByText(/40%/)).toBeInTheDocument();
    await userEvent.click(within(panel).getByRole("button", { name: "Cancel" }));

    expect(
      await within(panel).findByText(/Download cancelled — .* kept on disk/, {}, { timeout: 2_000 }),
    ).toBeInTheDocument();
    const resume = within(panel).getByRole("button", { name: "Resume download" });
    expect(props.onModelImported).not.toHaveBeenCalled();

    // Resuming picks up from the kept bytes and completes.
    await userEvent.click(resume);
    expect(
      await within(panel).findByText("Downloaded and registered.", {}, { timeout: 4_000 }),
    ).toBeInTheDocument();
    expect(props.onModelImported).toHaveBeenCalledTimes(1);
  }, 10_000);

  it("shows a retry after a failed preset download", async () => {
    const props = await controlCenterProps("model-none");
    vi.spyOn(props.bridge, "modelDownloadStatus")
      // The mount-time reattach check: nothing running yet.
      .mockResolvedValueOnce({ status: "idle", presetId: null, receivedBytes: 0, totalBytes: 0, failure: null })
      .mockResolvedValueOnce({ status: "running", presetId: "qwen3-coder-30b-q4ks", receivedBytes: 1_000, totalBytes: 17_456_012_448, failure: null })
      .mockResolvedValueOnce({
        status: "failed",
        presetId: "qwen3-coder-30b-q4ks",
        receivedBytes: 1_000,
        totalBytes: 17_456_012_448,
        failure: { kind: "unavailable", code: "connection_reset", detail: "The download connection dropped.", recovery: "Check the network and retry." },
      });
    render(<ControlCenterView {...props} />);

    const panel = screen.getByRole("region", { name: "Model runtime" });
    await userEvent.click(await within(panel).findByRole("button", { name: "Choose a model" }));
    await userEvent.click(await screen.findByRole("menuitemradio", { name: /Qwen3 Coder 30B — minimum/ }));
    await userEvent.click(within(panel).getByLabelText(/I accept the .* license/));
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

  it("reattaches to an in-flight download on mount instead of assuming idle", async () => {
    const props = await controlCenterProps("model-none");
    vi.spyOn(props.bridge, "modelDownloadStatus").mockResolvedValue({
      status: "running",
      presetId: "qwen3-coder-30b-q4ks",
      receivedBytes: 6_000_000_000,
      totalBytes: 17_456_012_448,
      failure: null,
    });
    render(<ControlCenterView {...props} />);

    const panel = screen.getByRole("region", { name: "Model runtime" });
    expect(await within(panel).findByText("qwen/qwen3-coder-30b-a3b-instruct-q4_k_s")).toBeInTheDocument();
    expect(await within(panel).findByText(/34%/)).toBeInTheDocument();
    expect(within(panel).getByRole("button", { name: "Downloading…" })).toBeDisabled();
  });

  it("disables switching presets while a download is running", async () => {
    const props = await controlCenterProps("model-none");
    render(<ControlCenterView {...props} />);

    const panel = screen.getByRole("region", { name: "Model runtime" });
    await userEvent.click(await within(panel).findByRole("button", { name: "Choose a model" }));
    await userEvent.click(await screen.findByRole("menuitemradio", { name: /Qwen3 Coder 30B — minimum/ }));
    await userEvent.click(within(panel).getByLabelText(/I accept the .* license/));
    await userEvent.click(within(panel).getByRole("button", { name: "Download" }));

    expect(await within(panel).findByRole("button", { name: "Downloading…" })).toBeDisabled();
    expect(within(panel).getByRole("button", { name: /Qwen3 Coder 30B — minimum/ })).toBeDisabled();
    expect(
      within(panel).getByText(/A download is already running — wait for it to finish/),
    ).toBeInTheDocument();
  });

  it("clears a stale Verified badge once the loaded model changes", async () => {
    const props = await controlCenterProps();
    const { rerender } = render(<ControlCenterView {...props} />);

    const panel = screen.getByRole("region", { name: "Model runtime" });
    await userEvent.click(within(panel).getByRole("button", { name: "Verify" }));
    expect(await within(panel).findByText(/Verified · \d+ ms/)).toBeInTheDocument();

    const restarted: ModelStatusDto = {
      status: "ok",
      loaded: { modelId: "qwen/qwen3-4b-instruct-q4", sizeBytes: 2_800_000_000 },
      registered: [],
    };
    rerender(<ControlCenterView {...props} modelStatus={restarted} />);

    expect(within(panel).getByText("qwen/qwen3-4b-instruct-q4")).toBeInTheDocument();
    expect(within(panel).queryByText(/Verified · \d+ ms/)).not.toBeInTheDocument();
  });

  it("refreshes a stale auto-filled identity when the path changes to a different file", async () => {
    const props = await controlCenterProps("model-none");
    vi.spyOn(props.bridge, "modelInspect")
      .mockResolvedValueOnce({
        status: "ok",
        fileName: "model-a.gguf",
        sizeBytes: 17_456_012_448,
        architecture: "qwen3moe",
        modelName: "Model-A",
        license: null,
        belowFloor: false,
        floorBytes: 17_000_000_000,
      })
      .mockResolvedValueOnce({
        status: "ok",
        fileName: "model-b.gguf",
        sizeBytes: 18_000_000_000,
        architecture: "llama",
        modelName: "Model-B",
        license: null,
        belowFloor: false,
        floorBytes: 17_000_000_000,
      });
    render(<ControlCenterView {...props} />);

    const panel = screen.getByRole("region", { name: "Model runtime" });
    await userEvent.type(within(panel).getByLabelText("GGUF file path"), "/models/model-a.gguf");
    await userEvent.tab();
    expect(await within(panel).findByText("model-a.gguf")).toBeInTheDocument();
    expect(within(panel).getByLabelText("Model identity")).toHaveValue("qwen3moe/model-a");

    await userEvent.clear(within(panel).getByLabelText("GGUF file path"));
    await userEvent.type(within(panel).getByLabelText("GGUF file path"), "/models/model-b.gguf");
    await userEvent.tab();

    expect(await within(panel).findByText("model-b.gguf")).toBeInTheDocument();
    expect(within(panel).getByLabelText("Model identity")).toHaveValue("llama/model-b");
  });

  it("shows a retry after a failed preset download driven through the fixture bridge", async () => {
    const props = await controlCenterProps("model-download-fail");
    render(<ControlCenterView {...props} />);

    const panel = screen.getByRole("region", { name: "Model runtime" });
    await userEvent.click(await within(panel).findByRole("button", { name: "Choose a model" }));
    await userEvent.click(await screen.findByRole("menuitemradio", { name: /Qwen3 Coder 30B — minimum/ }));
    await userEvent.click(within(panel).getByLabelText(/I accept the .* license/));
    await userEvent.click(within(panel).getByRole("button", { name: "Download" }));

    expect(
      await within(panel).findByText(
        /The download connection dropped\. Check the network and retry\./,
        {},
        { timeout: 2_000 },
      ),
    ).toBeInTheDocument();
    const retryButton = within(panel).getByRole("button", { name: "Retry download" });
    expect(retryButton).toBeInTheDocument();

    // Retrying re-runs the same fixture download, which now succeeds.
    await userEvent.click(retryButton);
    expect(
      await within(panel).findByText("Downloaded and registered.", {}, { timeout: 4_000 }),
    ).toBeInTheDocument();
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
