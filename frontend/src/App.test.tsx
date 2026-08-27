import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { App } from "./App";
import { fixtureBridge } from "./fixtures";

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
    render(<App bridge={fixtureBridge()} initialView="control-center" />);

    expect(await screen.findByRole("heading", { name: "Control center" })).toBeInTheDocument();
    const navigation = screen.getByRole("navigation", { name: "Primary" });
    expect(within(navigation).getAllByRole("button").map((button) => button.getAttribute("aria-label")))
      .toEqual(["Control Center", "Access", "Skills", "Flows", "Activity", "Console", "Connections"]);
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

  it("defaults to the Control Center view with the daemon truth and per-caller requests", async () => {
    render(<App bridge={fixtureBridge()} />);

    expect(await screen.findByRole("heading", { name: "Control center" })).toBeInTheDocument();
    expect(screen.getAllByText("Watch status").length).toBeGreaterThan(0);
    expect(screen.getByRole("region", { name: "Daemon overview" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "The last 26 weeks" })).toBeInTheDocument();
    const callers = await screen.findByRole("region", { name: "Requests per caller" });
    expect(await within(callers).findByText("gui:pam-desktop")).toBeInTheDocument();
    expect(within(callers).getByText("cli:release-agent")).toBeInTheDocument();
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

  it("shows the queue badge on the Control Center nav entry", async () => {
    render(<App bridge={fixtureBridge("queued")} />);
    await screen.findByRole("heading", { name: "Control center" });

    const navigation = screen.getByRole("navigation", { name: "Primary" });
    const entry = within(navigation).getByRole("button", { name: "Control Center" });
    expect(within(entry).getByLabelText("2 queued")).toBeInTheDocument();
    expect(within(within(navigation).getByRole("button", { name: "Connections" })).queryByLabelText(/queued/)).not.toBeInTheDocument();
  });

  it("switches and persists both variants of both named themes from the toolbar", async () => {
    const user = userEvent.setup();
    const first = render(<App bridge={fixtureBridge()} />);
    await screen.findByRole("heading", { name: "Control center" });

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

    await userEvent.click(await screen.findByRole("button", { name: "PAM is on watch" }));

    expect(await screen.findByRole("button", { name: "PAM is paused" })).toBeEnabled();
    expect(stop).toHaveBeenCalledTimes(1);
    expect(stop.mock.calls[0][0]).toMatchObject({ projectHandle: "daemon", generation: "daemon" });
    // Bootstrap probes once; the lifecycle action re-probes.
    await waitFor(() => expect(health).toHaveBeenCalledTimes(2));
    // With a project active, the snapshot is refreshed under the project fence.
    await waitFor(() => expect(refreshProject).toHaveBeenCalledTimes(1));
    expect(refreshProject.mock.calls[0][0].projectHandle).not.toBe("daemon");
    expect(screen.queryByText(/did not match the latest project operation/)).not.toBeInTheDocument();
  });

  it("keeps project browsing available from the Access context bar while the daemon is offline", async () => {
    const user = userEvent.setup();
    const bridge = fixtureBridge("offline");
    const activate = vi.spyOn(bridge, "activateProject");
    render(<App bridge={bridge} initialView="access" />);
    await screen.findByRole("heading", { name: "Access" });

    const switcher = await screen.findByRole("button", { name: "payments-api" });
    switcher.focus();
    await user.keyboard("{ArrowDown}");
    const menu = await screen.findByRole("menu");
    expect(menu).toHaveAttribute("aria-label", "Registered projects");
    expect(within(menu).getAllByRole("menuitemradio")).toHaveLength(3);

    await user.click(within(menu).getByRole("menuitemradio", { name: /ledger-web/ }));
    expect(await screen.findByRole("status")).toHaveTextContent("Now watching ledger-web");
    expect(activate).toHaveBeenCalledTimes(1);
  });

  it("keeps a calm paused Activity view while the daemon is offline", async () => {
    render(<App bridge={fixtureBridge("offline")} initialView="activity" />);

    expect(await screen.findByRole("heading", { name: "PAM is paused" })).toBeInTheDocument();
    expect(screen.getByText(/pick up where it left off/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Start PAM" })).toBeEnabled();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("renders startup transport failure in the bounded recovery shell", async () => {
    render(<App bridge={fixtureBridge("startup-error")} />);

    expect(await screen.findByRole("heading", { name: "PAM needs a moment" })).toBeInTheDocument();
    expect(screen.getByText("The PAM daemon fixture is unavailable.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Retry safely" })).toBeEnabled();
  });

  it("supports keyboard resizing, the eight view shortcuts, and Escape drawer recovery", async () => {
    const user = userEvent.setup();
    render(<App bridge={fixtureBridge()} />);
    await screen.findByRole("heading", { name: "Control center" });

    const separator = screen.getByRole("separator", { name: "Resize project sidebar" });
    fireEvent.keyDown(separator, { key: "ArrowRight" });
    expect(separator).toHaveAttribute("aria-valuenow", "264");
    expect(window.localStorage.getItem("pam-sidebar-width")).toBe("264");

    fireEvent.keyDown(window, { key: "2", metaKey: true });
    expect(await screen.findByRole("heading", { name: "Access" })).toBeInTheDocument();
    fireEvent.keyDown(window, { key: "3", metaKey: true });
    expect(await screen.findByRole("heading", { name: "Skills" })).toBeInTheDocument();
    fireEvent.keyDown(window, { key: "4", metaKey: true });
    expect(await screen.findByRole("heading", { name: "Flows" })).toBeInTheDocument();
    fireEvent.keyDown(window, { key: "5", metaKey: true });
    expect(await screen.findByRole("heading", { name: "Activity" })).toBeInTheDocument();
    fireEvent.keyDown(window, { key: "6", metaKey: true });
    expect(await screen.findByRole("heading", { name: "Console" })).toBeInTheDocument();
    fireEvent.keyDown(window, { key: "7", metaKey: true });
    expect(await screen.findByRole("heading", { name: "Connections" })).toBeInTheDocument();
    fireEvent.keyDown(window, { key: "8", metaKey: true });
    expect(await screen.findByRole("heading", { name: "Settings" })).toBeInTheDocument();
    fireEvent.keyDown(window, { key: "1", metaKey: true });
    expect(await screen.findByRole("heading", { name: "Control center" })).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Open queue" }));
    expect(screen.getByRole("dialog", { name: "Project queue" })).toBeInTheDocument();
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Project queue" })).not.toBeInTheDocument());
  });

  it("renders the available Access facts supplied by the active project", async () => {
    render(<App bridge={fixtureBridge("access-available")} initialView="access" />);

    expect(await screen.findByRole("heading", { name: "Access" })).toBeInTheDocument();
    expect(screen.getByText("Model access")).toBeInTheDocument();
    expect(screen.getByText("Access policy")).toBeInTheDocument();
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
    expect(screen.getByText("Access policy")).toBeInTheDocument();
    expect(screen.getByText("policy-gated")).toBeInTheDocument();
    expect(screen.getByText(/Network diagnostics are blocked by the selected project's policy/)).toBeInTheDocument();
    expect(screen.queryByText("Certificates")).not.toBeInTheDocument();
  });

  it("removes the prior project audit in the project-switch commit", async () => {
    const priorAuditSummary =
      "The always-loaded footprint is usable, with one overlapping review pair and one stale candidate to inspect.";
    const user = userEvent.setup();
    const bridge = fixtureBridge();
    const loadFirstAudit = bridge.loadSkillAudit.bind(bridge);
    bridge.loadSkillAudit = vi
      .fn()
      .mockImplementationOnce(loadFirstAudit)
      .mockImplementation(() => new Promise<never>(() => undefined));
    render(<App bridge={bridge} initialView="skills" />);

    await user.click(await screen.findByRole("tab", { name: "Audit" }));
    expect(await screen.findByText(priorAuditSummary)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "payments-api" }));
    await user.click(await screen.findByRole("menuitemradio", { name: /ledger-web/ }));

    // Activation lands on the Control Center, which carries no project claims.
    expect(await screen.findByRole("status")).toHaveTextContent("Now watching ledger-web");
    await waitFor(() => expect(screen.queryByText(priorAuditSummary)).not.toBeInTheDocument());

    await user.click(screen.getByRole("button", { name: "Skills" }));
    expect(await screen.findByRole("button", { name: "ledger-web" })).toBeInTheDocument();
    await user.click(await screen.findByRole("tab", { name: "Audit" }));
    expect(screen.queryByText(priorAuditSummary)).not.toBeInTheDocument();
    expect(await screen.findByText("Loading latest skill audit…")).toBeInTheDocument();
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
    render(<App bridge={bridge} initialView="control-center" />);
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
      await screen.findByRole("heading", { name: "Control center" });
      const trigger = screen.getByRole("button", { name: "Expand sidebar" });
      await user.click(trigger);

      const workspace = document.querySelector<HTMLElement>(".workspace");
      const sidebar = screen.getByRole("complementary", { name: "Daemon navigation" });
      const firstNav = within(sidebar).getByRole("button", { name: "Control Center" });
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
      await screen.findByRole("heading", { name: "Control center" });

      await user.click(screen.getByRole("button", { name: "Refresh project" }));
      await user.click(screen.getByRole("button", { name: "Expand sidebar" }));
      const sidebar = screen.getByRole("complementary", { name: "Daemon navigation" });
      await waitFor(() => expect(within(sidebar).getByRole("button", { name: "Control Center" })).toHaveFocus());

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

  // Radix moves menu focus through a requestAnimationFrame chain that jsdom
  // starves under parallel-suite CPU contention; the test is deterministic in
  // isolation, so environmental misses retry instead of weakening assertions.
  it("supports the complete keyboard project-menu contract from the Access view", { retry: 2 }, async () => {
    const user = userEvent.setup();
    render(<App bridge={fixtureBridge()} initialView="access" />);
    await screen.findByRole("heading", { name: "Access" });

    const switcher = await screen.findByRole("button", { name: "payments-api" });
    switcher.focus();
    await user.keyboard("{ArrowDown}");
    // Radix re-renders the portal while focus settles, so every assertion
    // re-queries instead of holding element references across keystrokes.
    const item = (name: RegExp) => screen.getByRole("menuitemradio", { name });
    await screen.findByRole("menuitemradio", { name: /payments-api/ });
    await waitFor(() => expect(item(/payments-api/)).toHaveFocus(), { timeout: 4000 });
    expect(item(/payments-api/)).toHaveAttribute("tabindex", "0");
    expect(item(/ledger-web/)).toHaveAttribute("tabindex", "-1");

    await user.keyboard("{ArrowDown}");
    await waitFor(() => expect(item(/ledger-web/)).toHaveFocus(), { timeout: 4000 });
    expect(item(/ledger-web/)).toHaveAttribute("tabindex", "0");
    expect(item(/payments-api/)).toHaveAttribute("tabindex", "-1");
    await user.keyboard("{End}");
    await waitFor(() => expect(item(/^docs/)).toHaveFocus(), { timeout: 4000 });
    await user.keyboard("{Home}");
    await waitFor(() => expect(item(/payments-api/)).toHaveFocus(), { timeout: 4000 });
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
    expect(switcher).toHaveFocus();

    await user.keyboard("{ArrowDown}");
    await waitFor(() => expect(item(/payments-api/)).toHaveFocus(), { timeout: 4000 });
    await user.keyboard("{ArrowDown}");
    await waitFor(() => expect(item(/ledger-web/)).toHaveFocus(), { timeout: 4000 });
    await user.keyboard("{Enter}");
    expect(await screen.findByRole("status")).toHaveTextContent("Now watching ledger-web");

    // Activation lands on the Control Center; the switcher returns in Access.
    await user.click(screen.getByRole("button", { name: "Access" }));
    const ledgerSwitcher = await screen.findByRole("button", { name: "ledger-web" });
    ledgerSwitcher.focus();
    await user.keyboard("{ArrowDown}");
    await waitFor(() => expect(screen.getByRole("menuitemradio", { name: /ledger-web/ })).toHaveFocus(), { timeout: 4000 });
    await user.keyboard("{End}");
    await waitFor(() => expect(screen.getByRole("menuitemradio", { name: /^docs/ })).toHaveFocus(), { timeout: 4000 });
    await user.keyboard(" ");
    expect(await screen.findByRole("status")).toHaveTextContent("Now watching docs");
  });

  it("filters and runs command-palette actions with keyboard focus restoration", async () => {
    const user = userEvent.setup();
    render(<App bridge={fixtureBridge()} />);
    await screen.findByRole("heading", { name: "Control center" });
    const commandOpener = screen.getByRole("button", { name: "Open command palette (⌘K)" });
    commandOpener.focus();

    await user.keyboard("{Control>}k{/Control}");
    let palette = await screen.findByRole("dialog", { name: "Command palette" });
    let search = within(palette).getByRole("searchbox", { name: "Search commands" });
    await waitFor(() => expect(search).toHaveFocus());
    await user.type(search, "connections");
    expect(within(palette).getAllByRole("option")).toHaveLength(1);
    const connectionsCommand = within(palette).getByRole("option", { name: /Open Connections/ });
    await user.keyboard("{ArrowDown}");
    await waitFor(() => expect(connectionsCommand).toHaveFocus());
    await user.keyboard("{Enter}");

    expect(await screen.findByRole("heading", { name: "Connections" })).toBeInTheDocument();
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
    render(<App bridge={fixtureBridge("approval")} initialView="control-center" />);
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
    render(<App bridge={bridge} initialView="control-center" />);
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
    render(<App bridge={bridge} initialView="control-center" />);
    await user.click(within(await screen.findByRole("dialog", { name: "Approval required" })).getByRole("button", { name: "Approve exact request" }));

    expect(await screen.findByRole("status")).toHaveTextContent("Approval expired; request a new challenge");
    expect(screen.queryByText("Exact request approved")).not.toBeInTheDocument();
  });

  it("discards stale command success and its toast when project responses reverse", async () => {
    const user = userEvent.setup();
    const bridge = fixtureBridge();
    const activate = bridge.activateProject.bind(bridge);
    const ledgerGate = deferred();
    const docsGate = deferred();
    bridge.activateProject = vi.fn(async (handle, operationId) => {
      if (handle.includes("2222")) await ledgerGate.promise;
      if (handle.includes("3333")) await docsGate.promise;
      return activate(handle, operationId);
    });
    render(<App bridge={bridge} initialView="access" />);
    await screen.findByRole("heading", { name: "Access" });
    await screen.findByRole("button", { name: "payments-api" });

    await user.click(screen.getByRole("button", { name: "payments-api" }));
    await user.click(screen.getByRole("menuitemradio", { name: /ledger-web/ }));
    await user.click(screen.getByRole("button", { name: "payments-api" }));
    await user.click(screen.getByRole("menuitemradio", { name: /^docs/ }));
    await act(async () => { docsGate.resolve(); });
    expect(await screen.findByRole("status")).toHaveTextContent("Now watching docs");
    // The docs activation wins and navigates home to the Control Center.
    expect(await screen.findByRole("heading", { name: "Control center" })).toBeInTheDocument();

    await act(async () => { ledgerGate.resolve(); });
    expect(screen.queryByText("Now watching ledger-web")).not.toBeInTheDocument();
  });

  it("opens, validates, and durably saves a bounded flow document", async () => {
    const user = userEvent.setup();
    const bridge = fixtureBridge();
    const saveFlow = bridge.saveFlow.bind(bridge);
    bridge.saveFlow = vi.fn(saveFlow);
    render(<App bridge={bridge} initialView="control-center" />);
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
    render(<App bridge={bridge} initialView="control-center" />);
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
    render(<App bridge={bridge} initialView="control-center" />);
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
    render(<App bridge={bridge} initialView="control-center" />);
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

  it("discards a flow document opened under the previous project authority", async () => {
    const user = userEvent.setup();
    const bridge = fixtureBridge();
    const originalOpenFlow = bridge.openFlow.bind(bridge);
    const gate = deferred();
    bridge.openFlow = vi.fn(async (fence, flowHandle) => {
      await gate.promise;
      return originalOpenFlow(fence, flowHandle);
    });
    render(<App bridge={bridge} initialView="control-center" />);
    await screen.findByRole("button", { name: "Refresh project" });
    await user.click(screen.getByRole("button", { name: "Flows" }));
    await screen.findByRole("region", { name: "Flow workspace" });
    await user.click(screen.getByRole("button", { name: /after-merge-checks/ }));

    await user.click(screen.getByRole("button", { name: "payments-api" }));
    await user.click(screen.getByRole("menuitemradio", { name: /ledger-web/ }));
    expect(await screen.findByRole("status")).toHaveTextContent("Now watching ledger-web");
    await user.click(screen.getByRole("button", { name: "Flows" }));
    await screen.findByRole("region", { name: "Flow workspace" });
    expect(await screen.findByRole("heading", { name: "Select a definition" })).toBeInTheDocument();

    await act(async () => { gate.resolve(); });
    const source = screen.getByRole("textbox", { name: "Flow TOML source" }) as HTMLTextAreaElement;
    expect(source).toBeDisabled();
    expect(source.value).toBe("");
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
    render(<App bridge={bridge} initialView="control-center" />);
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
    render(<App bridge={bridge} initialView="control-center" />);

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
    render(<App bridge={bridge} initialView="control-center" />);

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
      message: "PAM GUI caller registration failed: PAM's native credential store is unavailable.",
      recovery: "Retry registration or inspect the local PAM data store.",
    });
    render(<App bridge={bridge} initialView="control-center" />);

    await user.click(await screen.findByRole("button", { name: "Register GUI caller" }));

    expect(await screen.findByText(/PAM's native credential store is unavailable/)).toBeInTheDocument();
    expect(screen.queryByText("GUI caller registration could not be completed. Retry from this screen.")).not.toBeInTheDocument();
    expect(bridge.registerGuiCaller).toHaveBeenCalledTimes(1);
  });

  it("never substitutes fixture data after a production bridge failure", async () => {
    const bridge = fixtureBridge();
    bridge.bootstrap = vi.fn().mockRejectedValue(new Error("daemon socket unavailable"));
    render(<App bridge={bridge} />);

    expect(await screen.findByRole("heading", { name: "PAM needs a moment" })).toBeInTheDocument();
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
    await screen.findByRole("heading", { name: "Control center" });

    await user.click(screen.getByRole("button", { name: "Restart PAM" }));

    expect(calls).toEqual(["stop", "start"]);
    expect(await screen.findByText("PAM restarted")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /PAM is on watch|PAM is active/ })).toHaveAttribute("aria-pressed", "true");
  });

  it("hides the restart control while the daemon is stopped", async () => {
    render(<App bridge={fixtureBridge("offline")} />);
    await screen.findByRole("heading", { name: "Control center" });
    await screen.findByText("PAM is paused, so no requests are being served.");

    expect(screen.queryByRole("button", { name: "Restart PAM" })).not.toBeInTheDocument();
  });

  it("chats with the local model in an ephemeral drawer with usage lines", async () => {
    const user = userEvent.setup();
    render(<App bridge={fixtureBridge()} initialView="activity" />);

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
    render(<App bridge={fixtureBridge()} initialView="activity" />);

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
    render(<App bridge={bridge} initialView="activity" />);

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
    render(<App bridge={fixtureBridge("model-infer-blocked")} initialView="activity" />);

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
    await screen.findByRole("heading", { name: "Control center" });

    await user.click(screen.getByRole("button", { name: "Open command palette (⌘K)" }));
    let palette = await screen.findByRole("dialog", { name: "Command palette" });
    await user.type(within(palette).getByRole("searchbox", { name: "Search commands" }), "chat");
    await user.click(await within(palette).findByRole("option", { name: /Chat with the model/ }));
    expect(await screen.findByRole("dialog", { name: "Model chat" })).toBeInTheDocument();
    unmount();

    render(<App bridge={fixtureBridge("offline")} />);
    await screen.findByRole("heading", { name: "Control center" });
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

    expect(await screen.findByRole("heading", { name: "Control center" })).toBeInTheDocument();
    expect(screen.getByRole("region", { name: "Daemon overview" })).toBeInTheDocument();
    expect(screen.getByRole("region", { name: "Requests per caller" })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "PAM needs a moment" })).not.toBeInTheDocument();
    expect(document.querySelector(".breadcrumb")).toHaveTextContent(/^Daemon observatory$/);
    // The daemon-health probe feeds the sidebar pill without any project.
    expect(await screen.findByRole("button", { name: "PAM is on watch" })).toBeEnabled();
  });

  it("keeps the breadcrumb project-free even while a project is active", async () => {
    render(<App bridge={fixtureBridge()} />);
    await screen.findByRole("heading", { name: "Control center" });
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
    await waitFor(() => expect(status).toHaveBeenCalled());
    expect(status.mock.calls[0][0]).toMatchObject({ projectHandle: "daemon", generation: "daemon" });

    await user.click(await screen.findByRole("button", { name: "Chat" }));
    const drawer = await screen.findByRole("dialog", { name: "Model chat" });
    await user.type(within(drawer).getByRole("textbox", { name: "Message the model" }), "hello tide");
    await user.click(within(drawer).getByRole("button", { name: "Send" }));

    expect(await within(drawer).findByText(/You said: hello tide/)).toBeInTheDocument();
    expect(infer.mock.calls[0][0]).toMatchObject({ projectHandle: "daemon", generation: "daemon" });
  });

  it("serves Connections with zero projects under the daemon authority", async () => {
    const bridge = fixtureBridge("global-only");
    const callers = vi.spyOn(bridge, "callerRegistry");
    render(<App bridge={bridge} initialView="callers" />);

    expect(await screen.findByText("gui:pam-desktop")).toBeInTheDocument();
    expect(callers.mock.calls[0][0]).toMatchObject({ projectHandle: "daemon", generation: "daemon" });
  });

  it("shows the discovery hint in every project-shaped view with an empty catalog", async () => {
    const user = userEvent.setup();
    render(<App bridge={fixtureBridge("global-only")} />);
    await screen.findByRole("heading", { name: "Control center" });

    await user.click(screen.getByRole("button", { name: "Access" }));
    expect(await screen.findByRole("heading", { name: "Access" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "No projects discovered yet" })).toBeInTheDocument();

    // Skills and Flows are global first: flow definitions live in one shared
    // library, so both serve the daemon scope instead of the hint.
    await user.click(screen.getByRole("button", { name: "Skills" }));
    expect(await screen.findByRole("heading", { name: "Skill inventory" })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "No projects discovered yet" })).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Flows" }));
    expect(await screen.findByRole("region", { name: "Flow workspace" })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "No projects discovered yet" })).not.toBeInTheDocument();
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

  it("serves Skills under the daemon authority and picks a project for assignment", async () => {
    const user = userEvent.setup();
    const bridge = fixtureBridge();
    const originalBootstrap = bridge.bootstrap.bind(bridge);
    bridge.bootstrap = vi.fn(async () => ({ ...(await originalBootstrap()), snapshot: null }));
    const inventory = vi.spyOn(bridge, "loadSkillInventory");
    render(<App bridge={bridge} initialView="skills" />);

    expect(await screen.findByText("Global review checklist")).toBeInTheDocument();
    expect(inventory.mock.calls[0][0]).toMatchObject({ projectHandle: "daemon", generation: "daemon" });

    await user.click(screen.getByRole("tab", { name: "Library" }));
    expect(await screen.findByText(/Pick a project to manage targets/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Enable target" })).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: /payments-api/ }));
    expect(await screen.findByRole("heading", { name: "Control center" })).toBeInTheDocument();
    expect(await screen.findByRole("status")).toHaveTextContent("Now watching payments-api");
  });

  it("offers an inline picker without an active project and activates the selection", async () => {
    const user = userEvent.setup();
    const bridge = fixtureBridge();
    const originalBootstrap = bridge.bootstrap.bind(bridge);
    bridge.bootstrap = vi.fn(async () => ({ ...(await originalBootstrap()), snapshot: null }));
    render(<App bridge={bridge} initialView="access" />);

    expect(await screen.findByRole("heading", { name: "Pick a project to bring its queue into view." })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /payments-api/ }));

    expect(await screen.findByRole("heading", { name: "Control center" })).toBeInTheDocument();
    expect(await screen.findByRole("status")).toHaveTextContent("Now watching payments-api");
  });

  it("runs the daemon lifecycle from the global pill and re-probes health", async () => {
    const user = userEvent.setup();
    const bridge = fixtureBridge("global-only");
    const stop = vi.spyOn(bridge, "stopDaemon");
    const start = vi.spyOn(bridge, "startDaemon");
    const health = vi.spyOn(bridge, "daemonHealth");
    render(<App bridge={bridge} />);

    await user.click(await screen.findByRole("button", { name: "PAM is on watch" }));
    expect(await screen.findByRole("button", { name: "PAM is paused" })).toBeEnabled();
    expect(stop.mock.calls[0][0]).toMatchObject({ projectHandle: "daemon", generation: "daemon" });

    await user.click(screen.getByRole("button", { name: "PAM is paused" }));
    expect(await screen.findByRole("button", { name: "PAM is on watch" })).toBeEnabled();
    expect(start.mock.calls[0][0]).toMatchObject({ projectHandle: "daemon", generation: "daemon" });
    // Bootstrap probe plus one re-probe per lifecycle action.
    await waitFor(() => expect(health).toHaveBeenCalledTimes(3));
  });
});
