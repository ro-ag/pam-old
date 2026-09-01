import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { App } from "./App";
import { fixtureBridge } from "./fixtures";
import { HEATMAP_WEEKS } from "./views/OverviewView";

function memoryStorage(): Storage {
  const values = new Map<string, string>();
  return {
    get length() { return values.size; },
    clear: () => values.clear(),
    getItem: (key) => values.get(key) ?? null,
    key: (index) => [...values.keys()][index] ?? null,
    removeItem: (key) => values.delete(key),
    setItem: (key, value) => values.set(key, value),
  };
}

Object.defineProperty(window, "localStorage", {
  configurable: true,
  value: memoryStorage(),
});

function deferred() {
  let resolve!: () => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<void>((onResolve, onReject) => {
    resolve = onResolve;
    reject = onReject;
  });
  return { promise, resolve, reject };
}

describe("daemon observatory", () => {
  beforeEach(() => {
    window.localStorage.clear();
    delete document.documentElement.dataset.theme;
    delete document.documentElement.dataset.mode;
    document.documentElement.style.colorScheme = "";
  });

  it("renders the observatory spatial grammar without project claims on the main page", async () => {
    render(<App bridge={fixtureBridge()} initialView="overview" />);

    expect(await screen.findByRole("heading", { name: "Overview" })).toBeInTheDocument();
    const navigation = screen.getByRole("navigation", { name: "Primary" });
    expect(within(navigation).getAllByRole("button").map((button) => button.getAttribute("aria-label")))
      .toEqual(["Overview", "Models", "Flows", "Skills", "Access", "Activity"]);
    const sidebar = screen.getByRole("complementary", { name: "Daemon navigation" });
    // The sidebar brand carries the packaged app version, p-track style.
    expect(within(sidebar).getByText(/^v\d+\.\d+\.\d+$/)).toBeInTheDocument();
    expect(within(sidebar).queryByRole("button", { name: "payments-api" })).not.toBeInTheDocument();
    // The main page never offers project selection: no switcher, no picker.
    // A global, display-only fleet overview is fine — project names appear as
    // text, never as a button, and there is no per-project scoping control.
    expect(screen.queryByRole("button", { name: "payments-api" })).not.toBeInTheDocument();
    const fleet = screen.getByRole("region", { name: "Usage by project" });
    expect(within(fleet).getByText("payments-api")).toBeInTheDocument();
    expect(within(fleet).queryAllByRole("button")).toHaveLength(0);
    expect(screen.getByRole("separator", { name: "Resize project sidebar" })).toHaveAttribute("aria-valuenow", "248");
    expect(screen.getByText("Design fixture")).toBeInTheDocument();
  });

  it("defaults to the Overview view with the daemon truth and the fleet picture", async () => {
    render(<App bridge={fixtureBridge()} />);

    expect(await screen.findByRole("heading", { name: "Overview" })).toBeInTheDocument();
    expect(screen.getAllByText("Watch status").length).toBeGreaterThan(0);
    expect(screen.getByRole("region", { name: "Daemon overview" })).toBeInTheDocument();
    expect(
      screen.getByRole("heading", { name: `The last ${HEATMAP_WEEKS} weeks` }),
    ).toBeInTheDocument();
    expect(screen.getByRole("region", { name: "Usage by project" })).toBeInTheDocument();
    // The model is one read-only tile here; every model action is on Models.
    expect(await screen.findByRole("button", { name: /Local model/ })).toBeInTheDocument();
    expect(screen.queryByRole("region", { name: "Model runtime" })).not.toBeInTheDocument();
  });

  it("shows the Activity view with daemon health and the recent feed", async () => {
    render(<App bridge={fixtureBridge()} initialView="activity" />);

    expect(await screen.findByRole("heading", { name: "Activity" })).toBeInTheDocument();
    expect(screen.getByText("Watch status")).toBeInTheDocument();
    expect(screen.getByText("Daemon fixture-0.1.0")).toBeInTheDocument();
    expect(screen.getByText("Queue depth")).toBeInTheDocument();
    expect(await screen.findByText("project.current")).toBeInTheDocument();
    expect(screen.getByText(/gui:pam-desktop · payments-api/)).toBeInTheDocument();
  });

  it("shows the queue badge on the Overview nav entry", async () => {
    render(<App bridge={fixtureBridge("queued")} />);
    await screen.findByRole("heading", { name: "Overview" });

    const navigation = screen.getByRole("navigation", { name: "Primary" });
    const entry = within(navigation).getByRole("button", { name: "Overview" });
    expect(within(entry).getByLabelText("2 queued")).toBeInTheDocument();
    expect(within(within(navigation).getByRole("button", { name: "Models" })).queryByLabelText(/queued/)).not.toBeInTheDocument();
  });

  it("switches and persists both variants of both named themes from the toolbar", async () => {
    const user = userEvent.setup();
    const first = render(<App bridge={fixtureBridge()} />);
    await screen.findByRole("heading", { name: "Overview" });

    expect(document.documentElement).toHaveAttribute("data-theme", "ventisquero");
    expect(document.documentElement).toHaveAttribute("data-mode", "light");

    await user.click(screen.getByRole("button", { name: "Theme: Ventisquero · light" }));
    await user.click(screen.getByRole("menuitemradio", { name: /^Viña del Mar/ }));
    expect(document.documentElement).toHaveAttribute("data-theme", "vina");
    expect(document.documentElement).toHaveAttribute("data-mode", "light");

    await user.click(screen.getByRole("button", { name: "Theme: Viña del Mar · light" }));
    await user.click(screen.getByRole("menuitemradio", { name: /^Dark/ }));
    expect(document.documentElement).toHaveAttribute("data-theme", "vina");
    expect(document.documentElement).toHaveAttribute("data-mode", "dark");
    expect(document.documentElement.style.colorScheme).toBe("dark");
    expect(window.localStorage.getItem("pam-theme")).toBe("vina");
    expect(window.localStorage.getItem("pam-theme-mode")).toBe("dark");

    first.unmount();
    render(<App bridge={fixtureBridge()} />);
    expect(await screen.findByRole("button", { name: "Theme: Viña del Mar · dark" })).toBeInTheDocument();
  });

  it("keeps loading inside the shared shell without project claims", () => {
    render(<App bridge={fixtureBridge("loading")} />);

    expect(screen.getByRole("status")).toHaveTextContent("Finding the last registered project…");
    expect(screen.queryByText("payments-api")).not.toBeInTheDocument();
  });

  it("runs the daemon lifecycle under the daemon authority and re-probes health", async () => {
    const bridge = fixtureBridge();
    const stop = vi.spyOn(bridge, "stopDaemon");
    const health = vi.spyOn(bridge, "daemonHealth");
    const refreshProject = vi.spyOn(bridge, "refreshProject");
    render(<App bridge={bridge} />);

    await userEvent.click(await screen.findByRole("button", { name: "Stop Pam" }));
    await userEvent.click(await screen.findByRole("button", { name: "Stop" }));

    expect(await screen.findByRole("button", { name: "Start Pam" })).toBeEnabled();
    expect(stop).toHaveBeenCalledTimes(1);
    expect(stop.mock.calls[0][0]).toMatchObject({ projectHandle: "daemon", generation: "daemon" });
    // Bootstrap probes once; the lifecycle action re-probes.
    await waitFor(() => expect(health).toHaveBeenCalledTimes(2));
    // With a project active, the snapshot is refreshed under the project fence.
    await waitFor(() => expect(refreshProject).toHaveBeenCalledTimes(1));
    expect(refreshProject.mock.calls[0][0].projectHandle).not.toBe("daemon");
    expect(screen.queryByText(/did not match the latest project operation/)).not.toBeInTheDocument();
  });

  it("keeps Access global and project-free while the daemon is offline", async () => {
    const bridge = fixtureBridge("offline");
    const activate = vi.spyOn(bridge, "activateProject");
    render(<App bridge={bridge} initialView="access" />);
    await screen.findByRole("heading", { name: "Access" });

    // No switcher, no menu, no project name anywhere in the view canvas.
    expect(screen.queryByRole("button", { name: "payments-api" })).not.toBeInTheDocument();
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
    expect(screen.queryByText("/Users/dev/payments-api")).not.toBeInTheDocument();
    expect(activate).not.toHaveBeenCalled();

    // The daemon-scope grants and the observed boundary both stay readable.
    expect(await screen.findByRole("heading", { name: "Capabilities this window uses" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Authorized capabilities" })).toBeInTheDocument();
  });

  it("re-reads the observed boundary after a daemon-scope grant changes", async () => {
    // The boundary's verdicts are derived from these grants, so a grant that
    // leaves the boundary showing its pre-grant answer tells the owner to run
    // the exact CLI command the button they just pressed already ran.
    const user = userEvent.setup();
    const bridge = fixtureBridge("connector-blocked");
    const readBoundary = vi.spyOn(bridge, "daemonAccessConfig");
    render(<App bridge={bridge} initialView="access" />);

    await screen.findByRole("heading", { name: "Authorized capabilities" });
    await waitFor(() => expect(readBoundary).toHaveBeenCalledTimes(1));

    const capabilityRow = within(await screen.findByRole("article", { name: "connector.test" }));
    await user.click(capabilityRow.getByRole("button", { name: "Grant" }));

    await waitFor(() => expect(readBoundary).toHaveBeenCalledTimes(2));
  });

  it("keeps a calm paused Activity view while the daemon is offline", async () => {
    render(<App bridge={fixtureBridge("offline")} initialView="activity" />);

    expect(await screen.findByRole("heading", { name: "Pam is paused" })).toBeInTheDocument();
    expect(screen.getByText(/pick up where it left off/)).toBeInTheDocument();
    expect(within(screen.getByRole("main")).getByRole("button", { name: "Start Pam" })).toBeEnabled();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("renders startup transport failure in the bounded recovery shell", async () => {
    render(<App bridge={fixtureBridge("startup-error")} />);

    expect(await screen.findByRole("heading", { name: "Pam needs a moment" })).toBeInTheDocument();
    expect(screen.getByText("The Pam daemon fixture is unavailable.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Retry safely" })).toBeEnabled();
  });

  it("supports keyboard resizing, the seven view shortcuts, and Escape drawer recovery", async () => {
    const user = userEvent.setup();
    render(<App bridge={fixtureBridge()} />);
    await screen.findByRole("heading", { name: "Overview" });

    const separator = screen.getByRole("separator", { name: "Resize project sidebar" });
    fireEvent.keyDown(separator, { key: "ArrowRight" });
    expect(separator).toHaveAttribute("aria-valuenow", "264");
    expect(window.localStorage.getItem("pam-sidebar-width")).toBe("264");

    fireEvent.keyDown(window, { key: "2", metaKey: true });
    expect(await screen.findByRole("heading", { name: "Models" })).toBeInTheDocument();
    fireEvent.keyDown(window, { key: "3", metaKey: true });
    expect(await screen.findByRole("heading", { name: "Flows" })).toBeInTheDocument();
    fireEvent.keyDown(window, { key: "4", metaKey: true });
    expect(await screen.findByRole("heading", { name: "Skills" })).toBeInTheDocument();
    fireEvent.keyDown(window, { key: "5", metaKey: true });
    expect(await screen.findByRole("heading", { name: "Access" })).toBeInTheDocument();
    fireEvent.keyDown(window, { key: "6", metaKey: true });
    expect(await screen.findByRole("heading", { name: "Activity" })).toBeInTheDocument();
    fireEvent.keyDown(window, { key: "7", metaKey: true });
    expect(await screen.findByRole("heading", { name: "Settings" })).toBeInTheDocument();
    fireEvent.keyDown(window, { key: "1", metaKey: true });
    expect(await screen.findByRole("heading", { name: "Overview" })).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Open queue" }));
    expect(screen.getByRole("dialog", { name: "Project queue" })).toBeInTheDocument();
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Project queue" })).not.toBeInTheDocument());
  });

  it("renders the observed boundary over the daemon authority with a project active", async () => {
    const bridge = fixtureBridge("access-available");
    const read = vi.spyOn(bridge, "daemonAccessConfig");
    render(<App bridge={bridge} initialView="access" />);

    expect(await screen.findByRole("heading", { name: "Access" })).toBeInTheDocument();
    expect(await screen.findByText("Model access")).toBeInTheDocument();
    // Daemon authority, and a fresh operation per call so the replay guard holds.
    expect(read).toHaveBeenCalledWith(expect.objectContaining({ projectHandle: "daemon", generation: "daemon" }));
    expect(await screen.findByText("Access policy")).toBeInTheDocument();
    expect(screen.getByText("Certificates")).toBeInTheDocument();
    expect(screen.getByText("Network configuration")).toBeInTheDocument();
    expect(screen.getByText(/Operating-system certificate verifier enabled/)).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Skill library" })).not.toBeInTheDocument();
  });

  it("hosts the skill panels behind the Skills view tabs", async () => {
    const user = userEvent.setup();
    render(<App bridge={fixtureBridge("access-available")} initialView="skills" />);

    expect(await screen.findByRole("heading", { name: "Skills" })).toBeInTheDocument();
    expect(await screen.findByRole("heading", { name: "Skill inventory" })).toBeInTheDocument();

    await user.click(screen.getByRole("tab", { name: "Library" }));
    expect(await screen.findByRole("heading", { name: "Skill library" })).toBeInTheDocument();
    expect(screen.getByLabelText("Library state definitions")).toHaveTextContent("Observed");
    expect(screen.getByRole("button", { name: "Adopt into library" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Install into library" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Inspect drift" })).toBeInTheDocument();

    await user.click(screen.getByRole("tab", { name: "Audit" }));
    expect(await screen.findByRole("heading", { name: "Skill audit" })).toBeInTheDocument();
  });

  it("renders policy-blocked Access without available diagnostics", async () => {
    render(<App bridge={fixtureBridge("access-blocked")} initialView="access" />);

    expect(await screen.findByRole("heading", { name: "Access" })).toBeInTheDocument();
    // The observed boundary loads after the heading, so this has to be awaited:
    // a synchronous read races the panel's own fetch on a loaded runner.
    expect(await screen.findByText("Access policy")).toBeInTheDocument();
    expect(screen.getByText("policy-gated")).toBeInTheDocument();
    expect(screen.getByText(/Network diagnostics are blocked by policy for this Pam window/)).toBeInTheDocument();
    expect(screen.queryByText("Certificates")).not.toBeInTheDocument();
  });

  it("surfaces a failed observed-boundary read without losing the daemon-scope grants", async () => {
    const bridge = fixtureBridge();
    bridge.daemonAccessConfig = vi.fn(async () => { throw new Error("Pam could not read the observed boundary."); });
    render(<App bridge={bridge} initialView="access" />);

    expect(await screen.findByRole("heading", { name: "Capabilities this window uses" })).toBeInTheDocument();
    expect(await screen.findByText("Pam could not read the observed boundary.")).toBeInTheDocument();
    expect(screen.queryByText("Certificates")).not.toBeInTheDocument();
  });

  it("keeps Skills global: the audit renders with no project switcher to change it", async () => {
    const auditSummary =
      "The always-loaded footprint is usable, with one overlapping review pair and one stale candidate to inspect.";
    const user = userEvent.setup();
    render(<App bridge={fixtureBridge()} initialView="skills" />);

    await user.click(await screen.findByRole("tab", { name: "Audit" }));
    expect(await screen.findByText(auditSummary)).toBeInTheDocument();

    // Nothing in the view canvas scopes, names, or switches a project.
    expect(screen.queryByRole("button", { name: "payments-api" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "ledger-web" })).not.toBeInTheDocument();
    expect(screen.queryByRole("menuitemradio")).not.toBeInTheDocument();

    // Navigating away and back keeps the same global audit in view.
    await user.click(screen.getByRole("button", { name: "Access" }));
    await user.click(screen.getByRole("button", { name: "Skills" }));
    await user.click(await screen.findByRole("tab", { name: "Audit" }));
    expect(await screen.findByText(auditSummary)).toBeInTheDocument();
  });

  it("activates only the newest overlay and restores its underlay and exact openers", async () => {
    const user = userEvent.setup();
    const bridge = fixtureBridge();
    const approvalSnapshot = (await fixtureBridge("approval").bootstrap()).snapshot!;
    const refreshGate = deferred();
    bridge.refreshProject = vi.fn(async (fence) => {
      await refreshGate.promise;
      return { fence: structuredClone(fence), data: structuredClone(approvalSnapshot.data) };
    });
    render(<App bridge={bridge} initialView="overview" />);
    await screen.findByRole("button", { name: "Refresh project" });

    await user.click(screen.getByRole("button", { name: "Refresh project" }));
    const queueOpener = screen.getByRole("button", { name: "Open queue" });
    await user.click(queueOpener);
    expect(await screen.findByRole("dialog", { name: "Project queue" })).toBeInTheDocument();

    await act(async () => { refreshGate.resolve(); });
    const approval = await screen.findByRole("dialog", { name: "Approval required" });
    const queueUnderlay = screen.getByRole("dialog", { name: "Project queue", hidden: true });
    const approvalLayer = approval.closest<HTMLElement>("[data-application-overlay-layer]");
    const queueLayer = queueUnderlay.closest<HTMLElement>("[data-application-overlay-layer]");
    const appShell = document.querySelector<HTMLElement>(".app-shell");

    expect(screen.getAllByRole("dialog")).toEqual([approval]);
    expect(document.querySelectorAll('[data-application-overlay-layer="active"]')).toHaveLength(1);
    expect(approvalLayer).toHaveAttribute("data-application-overlay-layer", "active");
    expect(queueLayer).toHaveAttribute("data-application-overlay-layer", "underlay");
    expect(queueLayer).toHaveAttribute("aria-hidden", "true");
    expect(queueLayer).toHaveAttribute("inert");
    expect(appShell).toHaveAttribute("aria-hidden", "true");
    expect(appShell).toHaveAttribute("inert");

    await user.keyboard("{Escape}");
    const restoredQueue = await screen.findByRole("dialog", { name: "Project queue" });
    await waitFor(() => expect(within(restoredQueue).getByRole("button", { name: "Close Project queue" })).toHaveFocus());
    expect(screen.queryByRole("dialog", { name: "Approval required" })).not.toBeInTheDocument();
    expect(appShell).toHaveAttribute("aria-hidden", "true");
    expect(appShell).toHaveAttribute("inert");

    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Project queue" })).not.toBeInTheDocument());
    expect(queueOpener).toHaveFocus();
    expect(appShell).not.toHaveAttribute("aria-hidden");
    expect(appShell).not.toHaveAttribute("inert");
  });

  it("moves focus into and out of the compact sidebar while the workspace is inert", async () => {
    window.localStorage.setItem("pam-sidebar-collapsed", "false");
    const originalMatchMedia = window.matchMedia;
    Object.defineProperty(window, "matchMedia", {
      configurable: true,
      value: (query: string) => ({
        matches: query.includes("max-width: 780px"),
        media: query,
        onchange: null,
        addListener: () => undefined,
        removeListener: () => undefined,
        addEventListener: () => undefined,
        removeEventListener: () => undefined,
        dispatchEvent: () => false,
      }),
    });
    try {
      const user = userEvent.setup();
      render(<App bridge={fixtureBridge()} />);
      await screen.findByRole("heading", { name: "Overview" });
      const trigger = screen.getByRole("button", { name: "Expand sidebar" });
      await user.click(trigger);

      const workspace = document.querySelector<HTMLElement>(".workspace");
      const sidebar = screen.getByRole("complementary", { name: "Daemon navigation" });
      const firstNav = within(sidebar).getByRole("button", { name: "Overview" });
      await waitFor(() => expect(firstNav).toHaveFocus());
      expect(workspace).toHaveAttribute("inert");
      expect(workspace).toHaveAttribute("aria-hidden", "true");
      expect(screen.getByText("Skip to content")).toHaveAttribute("tabindex", "-1");
      expect(screen.getByRole("button", { name: "Close project sidebar" })).toHaveAttribute("tabindex", "-1");
      expect(screen.getByRole("separator", { name: "Resize project sidebar" })).toHaveAttribute("tabindex", "-1");

      const enabledSidebarButtons = within(sidebar).getAllByRole("button").filter((button) => !button.hasAttribute("disabled"));
      const lastSidebarButton = enabledSidebarButtons[enabledSidebarButtons.length - 1];
      await user.tab({ shift: true });
      expect(lastSidebarButton).toHaveFocus();
      await user.tab();
      expect(firstNav).toHaveFocus();

      await user.keyboard("{Escape}");
      await waitFor(() => expect(screen.getByRole("button", { name: "Expand sidebar" })).toHaveFocus());
      expect(workspace).not.toHaveAttribute("inert");
      expect(workspace).not.toHaveAttribute("aria-hidden");
      expect(window.localStorage.getItem("pam-sidebar-collapsed")).toBe("false");
    } finally {
      Object.defineProperty(window, "matchMedia", { configurable: true, value: originalMatchMedia });
    }
  });

  it("yields compact-sidebar focus containment to an active application overlay", async () => {
    const originalMatchMedia = window.matchMedia;
    Object.defineProperty(window, "matchMedia", {
      configurable: true,
      value: (query: string) => ({
        matches: query.includes("max-width: 780px"),
        media: query,
        onchange: null,
        addListener: () => undefined,
        removeListener: () => undefined,
        addEventListener: () => undefined,
        removeEventListener: () => undefined,
        dispatchEvent: () => false,
      }),
    });
    try {
      const user = userEvent.setup();
      const bridge = fixtureBridge();
      const approvalSnapshot = (await fixtureBridge("approval").bootstrap()).snapshot!;
      const refreshGate = deferred();
      bridge.refreshProject = vi.fn(async (fence) => {
        await refreshGate.promise;
        return { fence: structuredClone(fence), data: structuredClone(approvalSnapshot.data) };
      });
      render(<App bridge={bridge} />);
      await screen.findByRole("heading", { name: "Overview" });

      await user.click(screen.getByRole("button", { name: "Refresh project" }));
      await user.click(screen.getByRole("button", { name: "Expand sidebar" }));
      const sidebar = screen.getByRole("complementary", { name: "Daemon navigation" });
      await waitFor(() => expect(within(sidebar).getByRole("button", { name: "Overview" })).toHaveFocus());

      await act(async () => { refreshGate.resolve(); });
      const dialog = await screen.findByRole("dialog", { name: "Approval required" });
      await waitFor(() => expect(within(dialog).getByRole("button", { name: "Close Approval required" })).toHaveFocus());
      await user.tab();
      expect(dialog).toContainElement(document.activeElement as HTMLElement);
      expect(sidebar).not.toContainElement(document.activeElement as HTMLElement);
    } finally {
      Object.defineProperty(window, "matchMedia", { configurable: true, value: originalMatchMedia });
    }
  });

  it("filters and runs command-palette actions with keyboard focus restoration", async () => {
    const user = userEvent.setup();
    render(<App bridge={fixtureBridge()} />);
    await screen.findByRole("heading", { name: "Overview" });
    const commandOpener = screen.getByRole("button", { name: "Open command palette (⌘K)" });
    commandOpener.focus();

    await user.keyboard("{Control>}k{/Control}");
    let palette = await screen.findByRole("dialog", { name: "Command palette" });
    let search = within(palette).getByRole("searchbox", { name: "Search commands" });
    await waitFor(() => expect(search).toHaveFocus());
    await user.type(search, "models");
    expect(within(palette).getAllByRole("option")).toHaveLength(1);
    const modelsCommand = within(palette).getByRole("option", { name: /Open Models/ });
    await user.keyboard("{ArrowDown}");
    await waitFor(() => expect(modelsCommand).toHaveFocus());
    await user.keyboard("{Enter}");

    expect(await screen.findByRole("heading", { name: "Models" })).toBeInTheDocument();
    expect(screen.queryByRole("dialog", { name: "Command palette" })).not.toBeInTheDocument();
    await waitFor(() => expect(commandOpener).toHaveFocus());

    await user.keyboard("{Meta>}k{/Meta}");
    palette = await screen.findByRole("dialog", { name: "Command palette" });
    search = within(palette).getByRole("searchbox", { name: "Search commands" });
    await user.type(search, "queue");
    const queueCommand = within(palette).getByRole("option", { name: /Open project queue/ });
    await user.keyboard("{ArrowUp}");
    await waitFor(() => expect(queueCommand).toHaveFocus());
    await user.keyboard("{Enter}");

    expect(await screen.findByRole("dialog", { name: "Project queue" })).toBeInTheDocument();
    expect(screen.queryByRole("dialog", { name: "Command palette", hidden: true })).not.toBeInTheDocument();
    expect(document.querySelectorAll('[data-application-overlay-layer="underlay"]')).toHaveLength(0);
    expect(screen.getAllByRole("dialog")).toHaveLength(1);
  });

  it("opens bounded evidence from the Activity page as escaped text", async () => {
    const user = userEvent.setup();
    render(<App bridge={fixtureBridge()} initialView="activity" />);
    await screen.findByRole("heading", { name: "Activity" });

    const opener = await screen.findByRole("button", { name: "Open Evidence 1" });
    expect(opener).toHaveAccessibleDescription("44444444-4444-4444-8444-444444444444");
    await user.click(opener);
    expect(await screen.findByRole("dialog", { name: "Evidence" })).toBeInTheDocument();
    expect(await screen.findByText(/Null currency in fixture/)).toBeInTheDocument();
    expect(document.querySelector(".evidence-document pre script")).toBeNull();
    await user.click(screen.getByRole("button", { name: "Close Evidence" }));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Evidence" })).not.toBeInTheDocument());
  });

  it("shows the exact bounded approval effect without protocol request identifiers", async () => {
    render(<App bridge={fixtureBridge("approval")} />);

    const dialog = await screen.findByRole("dialog", { name: "Approval required" });
    expect(within(dialog).getByText("Read the selected project's bounded current queue and latest run")).toBeInTheDocument();
    expect(within(dialog).getByText("payments-api")).toBeInTheDocument();
    expect(within(dialog).getByText("project.current · exact project policy")).toBeInTheDocument();
    expect(within(dialog).getByText("aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee")).toBeInTheDocument();
    expect(within(dialog).queryByText(/fixture-request/)).not.toBeInTheDocument();
  });

  it("keeps auto approval dismissed after an explicit close", async () => {
    const user = userEvent.setup();
    render(<App bridge={fixtureBridge("approval")} initialView="overview" />);
    const initialDialog = await screen.findByRole("dialog", { name: "Approval required" });
    await user.click(within(initialDialog).getByRole("button", { name: "Close Approval required" }));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Approval required" })).not.toBeInTheDocument());
    await act(async () => { await Promise.resolve(); });
    expect(screen.queryByRole("dialog", { name: "Approval required" })).not.toBeInTheDocument();
  });

  it("keeps the approval handle actionable after an ambiguous decision failure", async () => {
    const user = userEvent.setup();
    const bridge = fixtureBridge("approval");
    const decideApproval = bridge.decideApproval.bind(bridge);
    let attempts = 0;
    bridge.decideApproval = vi.fn(async (fence, handle, decision) => {
      attempts += 1;
      if (attempts === 1) throw new Error("Approval response was not observed; retry the same decision safely.");
      return decideApproval(fence, handle, decision);
    });
    render(<App bridge={bridge} initialView="overview" />);
    const dialog = await screen.findByRole("dialog", { name: "Approval required" });
    await user.click(within(dialog).getByRole("button", { name: "Approve exact request" }));

    expect(await within(dialog).findByRole("alert")).toHaveTextContent("Approval response was not observed");
    expect(screen.getByRole("dialog", { name: "Approval required" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Approve exact request" }));
    expect(await screen.findByRole("status")).toHaveTextContent("Exact request approved");
    expect(bridge.decideApproval).toHaveBeenCalledTimes(2);
  });

  it("surfaces an explicit expired approval without a success claim", async () => {
    const user = userEvent.setup();
    const bridge = fixtureBridge("approval");
    const decideApproval = bridge.decideApproval.bind(bridge);
    bridge.decideApproval = vi.fn(async (fence, handle, decision) => {
      const response = await decideApproval(fence, handle, decision);
      response.disposition = "expired";
      response.snapshot.data.current = {
        status: "unavailable",
        failure: {
          kind: "unavailable",
          code: "approval_expired",
          detail: "This approval expired before the decision was applied.",
          recovery: "Retry project current to receive a new challenge.",
        },
      };
      return response;
    });
    render(<App bridge={bridge} initialView="overview" />);
    await user.click(within(await screen.findByRole("dialog", { name: "Approval required" })).getByRole("button", { name: "Approve exact request" }));

    expect(await screen.findByRole("status")).toHaveTextContent("Approval expired; request a new challenge");
    expect(screen.queryByText("Exact request approved")).not.toBeInTheDocument();
  });

  it("discards a command response that answers a different operation, toast included", async () => {
    const bridge = fixtureBridge();
    const nativeRefresh = bridge.refreshProject.bind(bridge);
    bridge.refreshProject = vi.fn(async (fence) => {
      const response = await nativeRefresh(fence);
      // A snapshot answering some other project operation must never commit.
      return { ...response, fence: { ...response.fence, operationId: "stale-operation" } };
    });
    render(<App bridge={bridge} initialView="access" />);
    await screen.findByRole("heading", { name: "Access" });

    fireEvent.keyDown(window, { key: "r", metaKey: true });

    expect(await screen.findByText(/did not match the latest project operation/)).toBeInTheDocument();
    expect(screen.queryByText("Project state refreshed")).not.toBeInTheDocument();
  });

  it("opens, validates, and durably saves a bounded flow document", async () => {
    const user = userEvent.setup();
    const bridge = fixtureBridge();
    const saveFlow = bridge.saveFlow.bind(bridge);
    bridge.saveFlow = vi.fn(saveFlow);
    render(<App bridge={bridge} initialView="overview" />);
    await screen.findByRole("button", { name: "Refresh project" });

    await user.click(screen.getByRole("button", { name: "Flows" }));
    expect(await screen.findByRole("region", { name: "Flow workspace" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /after-merge-checks/ }));
    await screen.findByRole("group", { name: "Flow steps" });
    await user.click(screen.getByRole("button", { name: "Source" }));
    const source = await screen.findByRole("textbox", { name: "Flow TOML source" }) as HTMLTextAreaElement;
    expect(source.value).toContain("schema_version = 2");
    fireEvent.change(source, { target: { value: `${source.value.replace("revision = 4", "revision = 5")}\n\n` } });
    await waitFor(() => expect(screen.getByRole("button", { name: "Validate" })).toBeEnabled());
    await user.click(screen.getByRole("button", { name: "Validate" }));
    expect(await screen.findByText(/Valid · 1 dry-run steps/)).toBeInTheDocument();
    const dryRunTab = screen.getByRole("tab", { name: "Dry run" });
    const diffTab = screen.getByRole("tab", { name: /Version diff · changed/ });
    dryRunTab.focus();
    await user.keyboard("{ArrowRight}");
    expect(diffTab).toHaveAttribute("aria-selected", "true");
    await user.keyboard("{ArrowLeft}");
    expect(dryRunTab).toHaveAttribute("aria-selected", "true");
    await user.keyboard("{End}");
    expect(diffTab).toHaveAttribute("aria-selected", "true");
    await user.keyboard("{Home}");
    expect(dryRunTab).toHaveAttribute("aria-selected", "true");
    await user.keyboard("{End}");
    expect(screen.getByRole("tabpanel", { name: /Version diff · changed/ })).toHaveTextContent("revision = 4");
    await waitFor(() => expect(screen.getByRole("button", { name: "Save" })).toBeEnabled());
    await user.click(screen.getByRole("button", { name: "Save" }));
    expect(await screen.findByText(/saved durably/i)).toBeInTheDocument();
    expect((screen.getByRole("textbox", { name: "Flow TOML source" }) as HTMLTextAreaElement).value.endsWith("\n\n")).toBe(false);
    expect(bridge.saveFlow).toHaveBeenCalledWith(expect.anything(), expect.any(String), expect.not.stringMatching(/\n\n$/));
  });

  it("keeps validation errors beside the flow source until the user edits", async () => {
    const user = userEvent.setup();
    const bridge = fixtureBridge();
    bridge.validateFlow = vi.fn().mockRejectedValue(new Error("Line 4: expected a TOML value"));
    render(<App bridge={bridge} initialView="overview" />);
    await screen.findByRole("button", { name: "Refresh project" });
    await user.click(screen.getByRole("button", { name: "Flows" }));
    await user.click(await screen.findByRole("button", { name: /after-merge-checks/ }));
    await screen.findByRole("group", { name: "Flow steps" });
    await user.click(screen.getByRole("button", { name: "Source" }));

    const source = await screen.findByRole("textbox", { name: "Flow TOML source" });
    await user.click(screen.getByRole("button", { name: "Validate" }));
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("Line 4: expected a TOML value");
    expect(source).toHaveAttribute("aria-invalid", "true");
    expect(source).toHaveAttribute("aria-describedby", alert.id);

    fireEvent.change(source, { target: { value: "schema_version = 2\n" } });
    expect(screen.queryByText("Line 4: expected a TOML value")).not.toBeInTheDocument();
    expect(source).not.toHaveAttribute("aria-invalid");
  });

  it("keeps Save disabled when the source changes during validation", async () => {
    const user = userEvent.setup();
    const bridge = fixtureBridge();
    const originalValidate = bridge.validateFlow.bind(bridge);
    const gate = deferred();
    bridge.validateFlow = vi.fn(async (fence, documentHandle, source) => {
      await gate.promise;
      return originalValidate(fence, documentHandle, source);
    });
    render(<App bridge={bridge} initialView="overview" />);
    await screen.findByRole("button", { name: "Refresh project" });
    await user.click(screen.getByRole("button", { name: "Flows" }));
    await screen.findByRole("region", { name: "Flow workspace" });
    await user.click(screen.getByRole("button", { name: /after-merge-checks/ }));
    await screen.findByRole("group", { name: "Flow steps" });
    await user.click(screen.getByRole("button", { name: "Source" }));
    const source = await screen.findByRole("textbox", { name: "Flow TOML source" });

    await user.click(screen.getByRole("button", { name: "Validate" }));
    fireEvent.change(source, { target: { value: `${(source as HTMLTextAreaElement).value}\n# edited while validating` } });
    await act(async () => { gate.resolve(); });

    expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();
    expect(screen.queryByText(/Valid ·/)).not.toBeInTheDocument();
  });

  it("accepts only the newest validation when responses arrive in reverse", async () => {
    const user = userEvent.setup();
    const bridge = fixtureBridge();
    const originalValidate = bridge.validateFlow.bind(bridge);
    const gates = [deferred(), deferred()];
    let call = 0;
    bridge.validateFlow = vi.fn(async (fence, documentHandle, source) => {
      const gate = gates[call++];
      await gate.promise;
      return originalValidate(fence, documentHandle, source);
    });
    render(<App bridge={bridge} initialView="overview" />);
    await screen.findByRole("button", { name: "Refresh project" });
    await user.click(screen.getByRole("button", { name: "Flows" }));
    await screen.findByRole("region", { name: "Flow workspace" });
    await user.click(screen.getByRole("button", { name: /after-merge-checks/ }));
    await screen.findByRole("group", { name: "Flow steps" });
    await user.click(screen.getByRole("button", { name: "Source" }));
    const source = await screen.findByRole("textbox", { name: "Flow TOML source" }) as HTMLTextAreaElement;

    await user.click(screen.getByRole("button", { name: "Validate" }));
    fireEvent.change(source, { target: { value: `${source.value}\n# newest source` } });
    await user.click(screen.getByRole("button", { name: "Validate" }));
    await act(async () => { gates[1].resolve(); });
    await waitFor(() => expect(screen.getByRole("button", { name: "Save" })).toBeEnabled());

    await act(async () => { gates[0].resolve(); });
    expect(screen.getByRole("button", { name: "Save" })).toBeEnabled();
    expect(screen.getByText(/Valid · 1 dry-run steps/)).toBeInTheDocument();
  });

  it("keeps the flow library global: no project switcher, no project-scoped fence", async () => {
    const user = userEvent.setup();
    const bridge = fixtureBridge();
    const loadWorkspace = vi.spyOn(bridge, "loadFlowWorkspace");
    render(<App bridge={bridge} initialView="overview" />);
    await screen.findByRole("button", { name: "Refresh project" });
    await user.click(screen.getByRole("button", { name: "Flows" }));
    await screen.findByRole("region", { name: "Flow workspace" });
    await user.click(screen.getByRole("button", { name: /after-merge-checks/ }));
    await screen.findByRole("group", { name: "Flow steps" });

    // The Flows view carries no project identity: nothing to select, nothing to scope.
    expect(screen.queryByRole("button", { name: "payments-api" })).not.toBeInTheDocument();

    // Access and Skills carry no switcher either, so nothing can re-scope the
    // library behind its back; a round trip leaves it exactly as it was.
    await user.click(screen.getByRole("button", { name: "Access" }));
    await screen.findByRole("heading", { name: "Access" });
    expect(screen.queryByRole("button", { name: "payments-api" })).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Skills" }));
    await screen.findByRole("heading", { name: "Skills" });
    expect(screen.queryByRole("button", { name: "payments-api" })).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Flows" }));
    await screen.findByRole("region", { name: "Flow workspace" });
    expect(screen.getByRole("button", { name: /after-merge-checks/ })).toBeInTheDocument();

    expect(loadWorkspace.mock.calls.length).toBeGreaterThan(0);
    for (const [requestFence] of loadWorkspace.mock.calls) {
      expect(requestFence).toMatchObject({ projectHandle: "daemon", generation: "daemon" });
    }
  });

  it("shows an inline retry when the flow workspace load fails", async () => {
    const user = userEvent.setup();
    const bridge = fixtureBridge();
    const originalLoadWorkspace = bridge.loadFlowWorkspace.bind(bridge);
    let attempts = 0;
    bridge.loadFlowWorkspace = vi.fn(async (fence) => {
      attempts += 1;
      if (attempts === 1) throw new Error("flow catalog temporarily unavailable");
      return originalLoadWorkspace(fence);
    });
    render(<App bridge={bridge} initialView="overview" />);
    await screen.findByRole("button", { name: "Refresh project" });
    await user.click(screen.getByRole("button", { name: "Flows" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("flow catalog temporarily unavailable");
    await user.click(screen.getByRole("button", { name: "Retry flows" }));
    expect(await screen.findByRole("region", { name: "Flow workspace" })).toBeInTheDocument();
  });

  it("registers a missing GUI caller through the fenced native recovery action", async () => {
    const user = userEvent.setup();
    const bridge = fixtureBridge("missing-credential");
    const originalRegister = bridge.registerGuiCaller.bind(bridge);
    const gate = deferred();
    bridge.registerGuiCaller = vi.fn(async (fence) => {
      await gate.promise;
      return originalRegister(fence);
    });
    render(<App bridge={bridge} initialView="access" />);

    const register = await screen.findByRole("button", { name: "Register GUI caller" });
    expect(screen.queryByText(/pam caller register|\/usr\/|\\\\/i)).not.toBeInTheDocument();
    await user.click(register);
    expect(screen.getByRole("button", { name: "Registering…" })).toBeDisabled();

    await act(async () => { gate.resolve(); });
    expect(await screen.findByRole("status")).toHaveTextContent("GUI caller registered");
    await waitFor(() => expect(screen.queryByRole("button", { name: "Register GUI caller" })).not.toBeInTheDocument());
    expect(bridge.registerGuiCaller).toHaveBeenCalledTimes(1);
  });

  it("keeps missing-credential recovery actionable after registration fails", async () => {
    const user = userEvent.setup();
    const bridge = fixtureBridge("missing-credential");
    bridge.registerGuiCaller = vi.fn().mockRejectedValue(new Error("/usr/local/bin/pam rejected secret-token"));
    render(<App bridge={bridge} initialView="access" />);

    await user.click(await screen.findByRole("button", { name: "Register GUI caller" }));

    expect(await screen.findByText("GUI caller registration could not be completed. Retry from this screen.")).toBeInTheDocument();
    expect(screen.queryByText(/\/usr\/local|secret-token/)).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Register GUI caller" })).toBeEnabled();
    expect(bridge.registerGuiCaller).toHaveBeenCalledTimes(1);
  });

  it("surfaces the bounded desktop reason when registration fails with a typed error", async () => {
    const user = userEvent.setup();
    const bridge = fixtureBridge("missing-credential");
    bridge.registerGuiCaller = vi.fn().mockRejectedValue({
      kind: "unavailable",
      message: "Pam GUI caller registration failed: Pam's native credential store is unavailable.",
      recovery: "Retry registration or inspect the local Pam data store.",
    });
    render(<App bridge={bridge} initialView="access" />);

    await user.click(await screen.findByRole("button", { name: "Register GUI caller" }));

    expect(await screen.findByText(/Pam's native credential store is unavailable/)).toBeInTheDocument();
    expect(screen.queryByText("GUI caller registration could not be completed. Retry from this screen.")).not.toBeInTheDocument();
    expect(bridge.registerGuiCaller).toHaveBeenCalledTimes(1);
  });

  it("never substitutes fixture data after a production bridge failure", async () => {
    const bridge = fixtureBridge();
    bridge.bootstrap = vi.fn().mockRejectedValue(new Error("daemon socket unavailable"));
    render(<App bridge={bridge} />);

    expect(await screen.findByRole("heading", { name: "Pam needs a moment" })).toBeInTheDocument();
    expect(screen.getByText("daemon socket unavailable")).toBeInTheDocument();
    expect(screen.queryByText("payments-api")).not.toBeInTheDocument();
  });

  it("restarts the running daemon as one stop-then-start command", async () => {
    const user = userEvent.setup();
    const bridge = fixtureBridge();
    const calls: string[] = [];
    const originalStop = bridge.stopDaemon.bind(bridge);
    const originalStart = bridge.startDaemon.bind(bridge);
    bridge.stopDaemon = vi.fn(async (fence) => { calls.push("stop"); return originalStop(fence); });
    bridge.startDaemon = vi.fn(async (fence) => { calls.push("start"); return originalStart(fence); });
    render(<App bridge={bridge} />);
    await screen.findByRole("heading", { name: "Overview" });

    await user.click(screen.getByRole("button", { name: "Restart Pam (unloads the loaded model)" }));

    expect(calls).toEqual(["stop", "start"]);
    expect(await screen.findByText("Pam restarted")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Stop Pam" })).toBeInTheDocument();
  });

  it("keeps the restart control in place, disabled, while the daemon is stopped", async () => {
    render(<App bridge={fixtureBridge("offline")} />);
    await screen.findByRole("heading", { name: "Overview" });
    await screen.findByText("The activity picture returns when Pam is back on watch.");

    expect(screen.getByRole("button", { name: "Restart Pam (unavailable while Pam is stopped)" })).toBeDisabled();
  });

  it("chats with the local model in an ephemeral drawer with usage lines", async () => {
    const user = userEvent.setup();
    render(<App bridge={fixtureBridge()} initialView="models" />);

    await user.click(await screen.findByRole("button", { name: "Chat" }));
    const drawer = await screen.findByRole("dialog", { name: "Model chat" });
    expect(within(drawer).getAllByText("qwen/qwen3-14b-instruct-q4").length).toBeGreaterThan(0);
    expect(within(drawer).getByText(/close the drawer and the transcript drifts away/)).toBeInTheDocument();

    await user.type(within(drawer).getByRole("textbox", { name: "Message the model" }), "hello tide");
    await user.click(within(drawer).getByRole("button", { name: "Send" }));

    expect(await within(drawer).findByText("hello tide")).toBeInTheDocument();
    expect(await within(drawer).findByText(/You said: hello tide/)).toBeInTheDocument();
    expect(within(drawer).getByText(/in \d+ · out \d+ tokens/)).toBeInTheDocument();

    await user.click(within(drawer).getByRole("button", { name: "Clear" }));
    expect(within(drawer).queryByText("hello tide")).not.toBeInTheDocument();
    expect(within(drawer).getByText(/Say hello to the local model/)).toBeInTheDocument();
  });

  it("discards the transcript on close, restores focus, and reopens empty", async () => {
    const user = userEvent.setup();
    render(<App bridge={fixtureBridge()} initialView="models" />);

    const chatOpener = await screen.findByRole("button", { name: "Chat" });
    await user.click(chatOpener);
    let drawer = await screen.findByRole("dialog", { name: "Model chat" });
    await user.type(within(drawer).getByRole("textbox", { name: "Message the model" }), "remember me");
    await user.click(within(drawer).getByRole("button", { name: "Send" }));
    await within(drawer).findByText(/You said: remember me/);

    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Model chat" })).not.toBeInTheDocument());
    await waitFor(() => expect(chatOpener).toHaveFocus());

    await user.click(chatOpener);
    drawer = await screen.findByRole("dialog", { name: "Model chat" });
    expect(within(drawer).queryByText(/remember me/)).not.toBeInTheDocument();
    expect(within(drawer).getByText(/Say hello to the local model/)).toBeInTheDocument();
  });

  it("shows a patient busy state and discards a stale reply after the drawer closes", async () => {
    const user = userEvent.setup();
    const bridge = fixtureBridge();
    let releaseReply!: () => void;
    const originalInfer = bridge.modelInfer.bind(bridge);
    bridge.modelInfer = vi.fn(async (fence, model, messages, maxOutputTokens) => {
      await new Promise<void>((resolve) => { releaseReply = resolve; });
      return originalInfer(fence, model, messages, maxOutputTokens);
    });
    render(<App bridge={bridge} initialView="models" />);

    await user.click(await screen.findByRole("button", { name: "Chat" }));
    const drawer = await screen.findByRole("dialog", { name: "Model chat" });
    await user.type(within(drawer).getByRole("textbox", { name: "Message the model" }), "slow reply");
    await user.click(within(drawer).getByRole("button", { name: "Send" }));

    expect(within(drawer).getByRole("status")).toHaveTextContent(/The model is thinking/);
    expect(within(drawer).getByRole("button", { name: "Send" })).toBeDisabled();

    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Model chat" })).not.toBeInTheDocument());
    await act(async () => { releaseReply(); });
    expect(screen.queryByText(/You said: slow reply/)).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Chat" }));
    const reopened = await screen.findByRole("dialog", { name: "Model chat" });
    expect(within(reopened).queryByText(/slow reply/)).not.toBeInTheDocument();
  });

  it("surfaces a blocked model.infer grant calmly inside the drawer", async () => {
    const user = userEvent.setup();
    render(<App bridge={fixtureBridge("model-infer-blocked")} initialView="models" />);

    await user.click(await screen.findByRole("button", { name: "Chat" }));
    const drawer = await screen.findByRole("dialog", { name: "Model chat" });
    await user.type(within(drawer).getByRole("textbox", { name: "Message the model" }), "hello");
    await user.click(within(drawer).getByRole("button", { name: "Send" }));

    const note = await within(drawer).findByText(/pam access grant model\.infer/);
    expect(note).toHaveTextContent("Project policy has not granted model.infer to this caller yet.");
    expect(within(drawer).queryByRole("alert")).not.toBeInTheDocument();
    expect(within(drawer).getByRole("button", { name: "Send" })).toBeDisabled();
  });

  it("offers the model chat palette command only while a model is present", async () => {
    const user = userEvent.setup();
    const { unmount } = render(<App bridge={fixtureBridge()} />);
    await screen.findByRole("heading", { name: "Overview" });

    await user.click(screen.getByRole("button", { name: "Open command palette (⌘K)" }));
    let palette = await screen.findByRole("dialog", { name: "Command palette" });
    await user.type(within(palette).getByRole("searchbox", { name: "Search commands" }), "chat");
    await user.click(await within(palette).findByRole("option", { name: /Chat with the model/ }));
    expect(await screen.findByRole("dialog", { name: "Model chat" })).toBeInTheDocument();
    unmount();

    render(<App bridge={fixtureBridge("offline")} />);
    await screen.findByRole("heading", { name: "Overview" });
    await user.click(screen.getByRole("button", { name: "Open command palette (⌘K)" }));
    palette = await screen.findByRole("dialog", { name: "Command palette" });
    await user.type(within(palette).getByRole("searchbox", { name: "Search commands" }), "chat");
    expect(within(palette).queryByRole("option", { name: /Chat with the model/ })).not.toBeInTheDocument();
    expect(within(palette).getByText("No matching commands.")).toBeInTheDocument();
  });
});

describe("global-first workspace", () => {
  beforeEach(() => {
    window.localStorage.clear();
  });

  it("boots to the shell with an empty catalog instead of a recovery screen", async () => {
    render(<App bridge={fixtureBridge("global-only")} />);

    expect(await screen.findByRole("heading", { name: "Overview" })).toBeInTheDocument();
    expect(screen.getByRole("region", { name: "Daemon overview" })).toBeInTheDocument();
    expect(screen.getByRole("region", { name: "Usage by project" })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Pam needs a moment" })).not.toBeInTheDocument();
    expect(document.querySelector(".breadcrumb")).toHaveTextContent(/^Daemon observatory$/);
    // The daemon-health probe feeds the sidebar pill without any project.
    expect(await screen.findByRole("button", { name: "Stop Pam" })).toBeEnabled();
  });

  it("keeps the breadcrumb project-free even while a project is active", async () => {
    render(<App bridge={fixtureBridge()} />);
    await screen.findByRole("heading", { name: "Overview" });
    await screen.findByRole("button", { name: "Refresh project" });

    expect(document.querySelector(".breadcrumb")).toHaveTextContent(/^Daemon observatory$/);
  });

  it("serves Activity, model status, and chat with zero projects under the daemon authority", async () => {
    const user = userEvent.setup();
    const bridge = fixtureBridge("global-only");
    const activity = vi.spyOn(bridge, "daemonActivity");
    const status = vi.spyOn(bridge, "modelStatus");
    const infer = vi.spyOn(bridge, "modelInfer");
    render(<App bridge={bridge} initialView="activity" />);

    expect(await screen.findByText("project.current")).toBeInTheDocument();
    expect(activity.mock.calls[0][0]).toMatchObject({ projectHandle: "daemon", generation: "daemon" });
    await user.click(screen.getByRole("button", { name: "Models" }));
    await waitFor(() => expect(status).toHaveBeenCalled());
    expect(status.mock.calls[0][0]).toMatchObject({ projectHandle: "daemon", generation: "daemon" });

    await user.click(await screen.findByRole("button", { name: "Chat" }));
    const drawer = await screen.findByRole("dialog", { name: "Model chat" });
    await user.type(within(drawer).getByRole("textbox", { name: "Message the model" }), "hello tide");
    await user.click(within(drawer).getByRole("button", { name: "Send" }));

    expect(await within(drawer).findByText(/You said: hello tide/)).toBeInTheDocument();
    expect(infer.mock.calls[0][0]).toMatchObject({ projectHandle: "daemon", generation: "daemon" });
  });

  it("serves the registered callers from Access with zero projects under the daemon authority", async () => {
    const bridge = fixtureBridge("global-only");
    const callers = vi.spyOn(bridge, "callerRegistry");
    render(<App bridge={bridge} initialView="access" />);

    const registry = await screen.findByRole("region", { name: "Registered callers" });
    expect(await within(registry).findByText("gui:pam-desktop")).toBeInTheDocument();
    expect(callers.mock.calls[0][0]).toMatchObject({ projectHandle: "daemon", generation: "daemon" });
  });

  it("shows no project-shaped placeholder in any view with an empty catalog", async () => {
    const user = userEvent.setup();
    render(<App bridge={fixtureBridge("global-only")} />);
    await screen.findByRole("heading", { name: "Overview" });

    // Every view is global: with zero projects each one still serves content,
    // and none of them falls back to a project-shaped empty state or picker.
    await user.click(screen.getByRole("button", { name: "Access" }));
    expect(await screen.findByRole("heading", { name: "Access" })).toBeInTheDocument();
    expect(await screen.findByRole("heading", { name: "Capabilities this window uses" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Authorized capabilities" })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "No projects discovered yet" })).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Skills" }));
    expect(await screen.findByRole("heading", { name: "Skill inventory" })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "No projects discovered yet" })).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Flows" }));
    expect(await screen.findByRole("region", { name: "Flow workspace" })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "No projects discovered yet" })).not.toBeInTheDocument();
  });

  it("serves the daemon-scope capability grants from Access with zero projects", async () => {
    const user = userEvent.setup();
    const bridge = fixtureBridge("global-only");
    const read = vi.spyOn(bridge, "daemonAccess");
    const write = vi.spyOn(bridge, "setDaemonAccess");
    render(<App bridge={bridge} initialView="access" />);

    const row = await screen.findByRole("article", { name: "model.infer" });
    expect(within(row).getByText("granted")).toBeInTheDocument();
    expect(read.mock.calls[0][0]).toMatchObject({ projectHandle: "daemon", generation: "daemon" });
    // Daemon-scope grants carry no project identity, even once one exists.
    expect(within(row).queryByText(/payments-api/)).not.toBeInTheDocument();

    await user.click(within(row).getByRole("button", { name: "Revoke" }));

    await waitFor(() => expect(within(row).getByText("not granted")).toBeInTheDocument());
    expect(write.mock.calls[0][0]).toMatchObject({ projectHandle: "daemon", generation: "daemon" });
  });

  it("serves Flows under the daemon authority with zero projects", async () => {
    const bridge = fixtureBridge("global-only");
    const loadWorkspace = vi.spyOn(bridge, "loadFlowWorkspace");
    render(<App bridge={bridge} initialView="flows" />);

    expect(await screen.findByRole("region", { name: "Flow workspace" })).toBeInTheDocument();
    expect(await screen.findByRole("button", { name: /after-merge-checks/ })).toBeInTheDocument();
    expect(loadWorkspace.mock.calls[0][0]).toMatchObject({ projectHandle: "daemon", generation: "daemon" });
  });

  it("keeps the flow draft across a ⌘R refresh while reloading the catalog", async () => {
    const user = userEvent.setup();
    const bridge = fixtureBridge();
    const loadWorkspace = vi.spyOn(bridge, "loadFlowWorkspace");
    render(<App bridge={bridge} initialView="flows" />);

    await screen.findByRole("region", { name: "Flow workspace" });
    await user.click(await screen.findByRole("button", { name: /after-merge-checks/ }));
    await screen.findByRole("group", { name: "Flow steps" });
    await user.click(screen.getByRole("button", { name: "Source" }));
    const source = screen.getByRole("textbox", { name: "Flow TOML source" }) as HTMLTextAreaElement;
    fireEvent.change(source, { target: { value: "# unsaved draft" } });

    // ⌘R refreshes the project snapshot, which rotates the fence generation.
    fireEvent.keyDown(window, { key: "r", metaKey: true });

    await waitFor(() => expect(loadWorkspace.mock.calls.length).toBeGreaterThan(1));
    expect((screen.getByRole("textbox", { name: "Flow TOML source" }) as HTMLTextAreaElement).value).toBe("# unsaved draft");
  });

  it("serves Skills under the daemon authority with no project pick on offer", async () => {
    const user = userEvent.setup();
    const bridge = fixtureBridge();
    const originalBootstrap = bridge.bootstrap.bind(bridge);
    bridge.bootstrap = vi.fn(async () => ({ ...(await originalBootstrap()), snapshot: null }));
    const inventory = vi.spyOn(bridge, "loadSkillInventory");
    const activate = vi.spyOn(bridge, "activateProject");
    render(<App bridge={bridge} initialView="skills" />);

    expect(await screen.findByText("Global review checklist")).toBeInTheDocument();
    expect(inventory.mock.calls[0][0]).toMatchObject({ projectHandle: "daemon", generation: "daemon" });

    await user.click(screen.getByRole("tab", { name: "Library" }));
    // Assignment stays gated without a project scope, and nothing offers one.
    expect(await screen.findByText(/Pam has none open/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Enable target" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /payments-api/ })).not.toBeInTheDocument();
    expect(activate).not.toHaveBeenCalled();
  });

  it("serves Access with no snapshot and offers no picker for the missing one", async () => {
    const bridge = fixtureBridge();
    const originalBootstrap = bridge.bootstrap.bind(bridge);
    bridge.bootstrap = vi.fn(async () => ({ ...(await originalBootstrap()), snapshot: null }));
    const activate = vi.spyOn(bridge, "activateProject");
    render(<App bridge={bridge} initialView="access" />);

    expect(await screen.findByRole("heading", { name: "Access" })).toBeInTheDocument();
    // Both panels are daemon-scope reads, so they render with zero projects.
    expect(await screen.findByRole("heading", { name: "Capabilities this window uses" })).toBeInTheDocument();
    expect(await screen.findByText("Certificates")).toBeInTheDocument();
    expect(screen.queryByText("The daemon has not reported an access boundary yet.")).not.toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Pick a project to bring its queue into view." })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /payments-api/ })).not.toBeInTheDocument();
    expect(activate).not.toHaveBeenCalled();
  });

  it("runs the daemon lifecycle from the global pill and re-probes health", async () => {
    const user = userEvent.setup();
    const bridge = fixtureBridge("global-only");
    const stop = vi.spyOn(bridge, "stopDaemon");
    const start = vi.spyOn(bridge, "startDaemon");
    const health = vi.spyOn(bridge, "daemonHealth");
    render(<App bridge={bridge} />);

    await user.click(await screen.findByRole("button", { name: "Stop Pam" }));
    await user.click(await screen.findByRole("button", { name: "Stop" }));
    expect(await screen.findByRole("button", { name: "Start Pam" })).toBeEnabled();
    expect(stop.mock.calls[0][0]).toMatchObject({ projectHandle: "daemon", generation: "daemon" });

    await user.click(screen.getByRole("button", { name: "Start Pam" }));
    expect(await screen.findByRole("button", { name: "Stop Pam" })).toBeEnabled();
    expect(start.mock.calls[0][0]).toMatchObject({ projectHandle: "daemon", generation: "daemon" });
    // Bootstrap probe plus one re-probe per lifecycle action.
    await waitFor(() => expect(health).toHaveBeenCalledTimes(3));
  });

  // #238: a running Pam changes models in place. The restart dance is gone —
  // nothing is stopped, nothing is started, and the copy no longer promises
  // a restart that does not happen.
  it("loads a registered model into the running daemon without restarting it", async () => {
    const user = userEvent.setup();
    const bridge = fixtureBridge("model-on-deck");
    const stop = vi.spyOn(bridge, "stopDaemon");
    const start = vi.spyOn(bridge, "startDaemon");
    const load = vi.spyOn(bridge, "modelLoad");
    render(<App bridge={bridge} initialView="models" />);

    const panel = await screen.findByRole("region", { name: "Model runtime" });
    expect(
      within(panel).queryByRole("button", { name: "Restart Pam with this model" }),
    ).not.toBeInTheDocument();
    await user.click((await within(panel).findAllByRole("button", { name: "Load" }))[0]);

    await waitFor(() => expect(load).toHaveBeenCalled());
    expect(load.mock.calls[0][1]).toBe("qwen/qwen3-14b-instruct-q4");
    expect(stop).not.toHaveBeenCalled();
    expect(start).not.toHaveBeenCalled();
    // The reloaded status moves the chosen model into the loaded slot.
    expect(await within(panel).findByText("loaded")).toBeInTheDocument();
  });

  // The other half of the same capability: the loaded model can be dropped
  // and Pam keeps serving.
  it("unloads the loaded model and keeps the daemon running", async () => {
    const user = userEvent.setup();
    const bridge = fixtureBridge("solved");
    const stop = vi.spyOn(bridge, "stopDaemon");
    const unload = vi.spyOn(bridge, "modelUnload");
    render(<App bridge={bridge} initialView="models" />);

    const panel = await screen.findByRole("region", { name: "Model runtime" });
    await user.click(await within(panel).findByRole("button", { name: "Unload" }));

    await waitFor(() => expect(unload).toHaveBeenCalled());
    expect(stop).not.toHaveBeenCalled();
    expect(await within(panel).findByText("on deck")).toBeInTheDocument();
  });

  // A refusal has to reach the person who asked for it, with the line that
  // fixes it, rather than disappearing into a silent no-op.
  it("renders an ungranted unload's recovery line next to the model", async () => {
    const user = userEvent.setup();
    const bridge = fixtureBridge("model-load-blocked");
    render(<App bridge={bridge} initialView="models" />);

    const panel = await screen.findByRole("region", { name: "Model runtime" });
    await user.click(await within(panel).findByRole("button", { name: "Unload" }));

    expect(
      await within(panel).findByText(/Grant the GUI caller the model.unload capability in Access/),
    ).toBeInTheDocument();
  });

  it("starts the daemon with a registered model while Pam is paused", async () => {
    const user = userEvent.setup();
    const bridge = fixtureBridge("offline");
    const stop = vi.spyOn(bridge, "stopDaemon");
    const start = vi.spyOn(bridge, "startDaemon");
    render(<App bridge={bridge} initialView="models" />);

    const panel = await screen.findByRole("region", { name: "Model runtime" });
    await user.click(
      (await within(panel).findAllByRole("button", { name: "Start Pam with this model" }))[0],
    );

    await waitFor(() => expect(start).toHaveBeenCalled());
    // Already paused: no stop round-trip first.
    expect(stop).not.toHaveBeenCalled();
    expect(start.mock.calls[0][1]).toBe("qwen/qwen3-14b-instruct-q4");
    expect(await within(panel).findByText("loaded")).toBeInTheDocument();
  });
});
