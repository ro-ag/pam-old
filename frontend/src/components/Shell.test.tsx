import { fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createRef } from "react";
import { describe, expect, it, vi } from "vitest";

import type { DaemonState, DaemonView } from "../selectors";
import { ResizeSeparator, Sidebar, Toolbar, appVersionLabel } from "./Shell";

interface CaptureStubs {
  captured: Set<number>;
  setPointerCapture: ReturnType<typeof vi.fn>;
  hasPointerCapture: ReturnType<typeof vi.fn>;
  releasePointerCapture: ReturnType<typeof vi.fn>;
}

function installPointerCaptureStubs(element: HTMLElement): CaptureStubs {
  const captured = new Set<number>();
  const setPointerCapture = vi.fn((pointerId: number) => captured.add(pointerId));
  const hasPointerCapture = vi.fn((pointerId: number) => captured.has(pointerId));
  const releasePointerCapture = vi.fn((pointerId: number) => captured.delete(pointerId));
  Object.assign(element, { setPointerCapture, hasPointerCapture, releasePointerCapture });
  return { captured, setPointerCapture, hasPointerCapture, releasePointerCapture };
}

function dispatchPointer(
  element: HTMLElement,
  type: string,
  properties: Record<string, number | boolean>,
): void {
  const event = new Event(type, { bubbles: true, cancelable: true });
  Object.defineProperties(event, Object.fromEntries(
    Object.entries(properties).map(([key, value]) => [key, { value }]),
  ));
  fireEvent(element, event);
}

function renderSeparator({
  collapsed = false,
  width = 248,
  viewportWidth = 1_400,
  onResizePreview = vi.fn(),
  onResizeCommit = vi.fn(),
  onToggle = vi.fn(),
}: Partial<React.ComponentProps<typeof ResizeSeparator>> = {}) {
  render(
    <ResizeSeparator
      collapsed={collapsed}
      width={width}
      viewportWidth={viewportWidth}
      onResizePreview={onResizePreview}
      onResizeCommit={onResizeCommit}
      onToggle={onToggle}
    />,
  );
  return screen.getByRole("separator", { name: "Resize project sidebar" });
}

describe("Toolbar", () => {
  it("changes theme family and variant from the toolbar theme menu", async () => {
    const user = userEvent.setup();
    const onThemeChange = vi.fn();
    const onThemeModeChange = vi.fn();
    render(
      <Toolbar
        projectName="payments-api"
        queueCount={2}
        fixture={true}
        collapsed={false}
        pending={false}
        theme="ventisquero"
        themeMode="light"
        density="compact"
        onThemeChange={onThemeChange}
        onThemeModeChange={onThemeModeChange}
        onDensityChange={vi.fn()}
        onToggleSidebar={vi.fn()}
        onRefresh={vi.fn()}
        onOpenCommand={vi.fn()}
        onOpenQueue={vi.fn()}
        toggleButtonRef={createRef()}
        commandButtonRef={createRef()}
        queueButtonRef={createRef()}
      />,
    );

    await user.click(screen.getByRole("button", { name: "Theme: Ventisquero · light" }));
    await user.click(screen.getByRole("menuitemradio", { name: /^Viña del Mar/ }));
    expect(onThemeChange).toHaveBeenCalledWith("vina");

    await user.click(screen.getByRole("button", { name: "Theme: Ventisquero · light" }));
    await user.click(screen.getByRole("menuitemradio", { name: /^Dark/ }));
    expect(onThemeModeChange).toHaveBeenCalledWith("dark");

    expect(document.querySelector(".breadcrumb")).toHaveTextContent(/^Daemon observatory$/);
  });

  it("shows the global breadcrumb alone and hides the queue opener without a project", () => {
    render(
      <Toolbar
        projectName={null}
        queueCount={null}
        fixture={false}
        collapsed={false}
        pending={false}
        theme="ventisquero"
        themeMode="light"
        density="compact"
        onThemeChange={vi.fn()}
        onThemeModeChange={vi.fn()}
        onDensityChange={vi.fn()}
        onToggleSidebar={vi.fn()}
        onRefresh={vi.fn()}
        onOpenCommand={vi.fn()}
        onOpenQueue={vi.fn()}
        toggleButtonRef={createRef()}
        commandButtonRef={createRef()}
        queueButtonRef={createRef()}
      />,
    );

    expect(document.querySelector(".breadcrumb")).toHaveTextContent(/^Daemon observatory$/);
    expect(screen.queryByRole("button", { name: "Open queue" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Refresh daemon" })).toBeInTheDocument();
  });
});

const daemonViews: Record<DaemonState, DaemonView> = {
  running: { state: "running", detail: "Running", model: "Daemon 0.9.0", modelMemory: null, queueDepth: 0 },
  stopped: { state: "stopped", detail: "Stopped", model: null, modelMemory: null, queueDepth: null },
  unavailable: { state: "unavailable", detail: "Checking on Pam…", model: null, modelMemory: null, queueDepth: null },
};

function renderSidebar({
  state = "running" as DaemonState,
  collapsed = false,
  pending = false,
  onToggleDaemon = vi.fn(),
  onRestartDaemon = vi.fn(),
} = {}) {
  render(
    <Sidebar
      daemon={daemonViews[state]}
      queueCount={0}
      activeView="overview"
      collapsed={collapsed}
      pending={pending}
      trapFocus={false}
      onNavigate={vi.fn()}
      onToggleDaemon={onToggleDaemon}
      onRestartDaemon={onRestartDaemon}
      onDismiss={vi.fn()}
      containerRef={createRef()}
    />,
  );
  return { onToggleDaemon, onRestartDaemon };
}

describe("Sidebar daemon lifecycle", () => {
  it("labels the control with the action and keeps the state as separate status text", () => {
    renderSidebar({ state: "running" });

    const stop = screen.getByRole("button", { name: "Stop Pam" });
    expect(stop).not.toHaveAttribute("aria-pressed");
    expect(screen.queryByRole("button", { name: "Start Pam" })).not.toBeInTheDocument();
    expect(document.querySelector(".daemon-status")).toHaveTextContent("Running");
  });

  it("labels the control Start while the daemon is stopped", () => {
    renderSidebar({ state: "stopped" });

    expect(screen.getByRole("button", { name: "Start Pam" })).toBeEnabled();
    expect(screen.queryByRole("button", { name: "Stop Pam" })).not.toBeInTheDocument();
    expect(document.querySelector(".daemon-status")).toHaveTextContent("Stopped");
  });

  it("starts without asking for confirmation", async () => {
    const user = userEvent.setup();
    const { onToggleDaemon } = renderSidebar({ state: "stopped" });

    await user.click(screen.getByRole("button", { name: "Start Pam" }));

    expect(onToggleDaemon).toHaveBeenCalledTimes(1);
    expect(screen.queryByText(/unloads the loaded model/)).not.toBeInTheDocument();
  });

  it("asks before stopping and leaves the daemon alone when the ask is declined", async () => {
    const user = userEvent.setup();
    const { onToggleDaemon } = renderSidebar({ state: "running" });

    await user.click(screen.getByRole("button", { name: "Stop Pam" }));
    expect(onToggleDaemon).not.toHaveBeenCalled();
    expect(
      screen.getByText("Stop Pam? It unloads the loaded model and drops queued work."),
    ).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Keep running" }));

    expect(onToggleDaemon).not.toHaveBeenCalled();
    expect(screen.queryByText(/unloads the loaded model and drops queued work/)).not.toBeInTheDocument();
  });

  it("stops the daemon once the ask is accepted", async () => {
    const user = userEvent.setup();
    const { onToggleDaemon } = renderSidebar({ state: "running" });

    await user.click(screen.getByRole("button", { name: "Stop Pam" }));
    await user.click(screen.getByRole("button", { name: "Stop" }));

    expect(onToggleDaemon).toHaveBeenCalledTimes(1);
    expect(screen.queryByText(/unloads the loaded model and drops queued work/)).not.toBeInTheDocument();
  });

  it("keeps the restart control in its slot, disabled with a reason, while stopped", async () => {
    const user = userEvent.setup();
    const { onRestartDaemon } = renderSidebar({ state: "stopped" });

    const restart = screen.getByRole("button", { name: "Restart Pam (unavailable while Pam is stopped)" });
    expect(restart).toBeDisabled();

    await user.click(restart);
    expect(onRestartDaemon).not.toHaveBeenCalled();
  });

  it("keeps the verb in the accessible name while the sidebar is collapsed", () => {
    renderSidebar({ state: "stopped", collapsed: true });

    const start = screen.getByRole("button", { name: "Start Pam" });
    expect(start).toHaveAttribute("title", "Start Pam");
    // The status stays in the accessibility tree; only its glyph is visible.
    expect(document.querySelector(".daemon-status")).toHaveTextContent("Stopped");
  });
});

describe("ResizeSeparator", () => {
  it("reports responsive minimum, maximum, and clamped current widths", () => {
    const { rerender } = render(
      <ResizeSeparator
        collapsed={false}
        width={500}
        viewportWidth={1_400}
        onResizePreview={vi.fn()}
        onResizeCommit={vi.fn()}
        onToggle={vi.fn()}
      />,
    );
    const separator = screen.getByRole("separator", { name: "Resize project sidebar" });
    expect(separator).toHaveAttribute("aria-valuemin", "180");
    expect(separator).toHaveAttribute("aria-valuemax", "420");
    expect(separator).toHaveAttribute("aria-valuenow", "420");

    rerender(
      <ResizeSeparator
        collapsed={false}
        width={400}
        viewportWidth={640}
        onResizePreview={vi.fn()}
        onResizeCommit={vi.fn()}
        onToggle={vi.fn()}
      />,
    );
    expect(separator).toHaveAttribute("aria-valuemin", "180");
    expect(separator).toHaveAttribute("aria-valuemax", "288");
    expect(separator).toHaveAttribute("aria-valuenow", "288");
  });

  it("commits exact fine, coarse, and boundary keyboard widths", () => {
    const onResizeCommit = vi.fn();
    const separator = renderSeparator({ onResizeCommit });

    for (const key of ["ArrowLeft", "ArrowRight", "PageDown", "PageUp", "Home", "End"]) {
      fireEvent.keyDown(separator, { key });
    }

    expect(onResizeCommit.mock.calls).toEqual([
      [232],
      [264],
      [184],
      [312],
      [180],
      [420],
    ]);
  });

  it("is disabled and removed from tab order while collapsed", () => {
    const onResizeCommit = vi.fn();
    const separator = renderSeparator({ collapsed: true, onResizeCommit });

    expect(separator).toHaveAttribute("aria-disabled", "true");
    expect(separator).toHaveAttribute("tabindex", "-1");
    fireEvent.keyDown(separator, { key: "ArrowRight" });
    expect(onResizeCommit).not.toHaveBeenCalled();
  });

  it("captures only a primary pointer, previews clamped width, and commits once on pointerup", () => {
    const onResizePreview = vi.fn();
    const onResizeCommit = vi.fn();
    const separator = renderSeparator({ onResizePreview, onResizeCommit });
    const capture = installPointerCaptureStubs(separator);

    dispatchPointer(separator, "pointerdown", { pointerId: 4, isPrimary: false, button: 0, clientX: 248 });
    dispatchPointer(separator, "pointerdown", { pointerId: 5, isPrimary: true, button: 1, clientX: 248 });
    expect(capture.setPointerCapture).not.toHaveBeenCalled();

    dispatchPointer(separator, "pointerdown", { pointerId: 7, isPrimary: true, button: 0, clientX: 248 });
    expect(capture.setPointerCapture).toHaveBeenCalledOnce();
    expect(capture.setPointerCapture).toHaveBeenCalledWith(7);

    dispatchPointer(separator, "pointermove", { pointerId: 7, clientX: -1_000 });
    dispatchPointer(separator, "pointermove", { pointerId: 7, clientX: 1_000 });
    expect(onResizePreview.mock.calls).toEqual([[180], [420]]);

    dispatchPointer(separator, "pointerup", { pointerId: 7 });
    dispatchPointer(separator, "lostpointercapture", { pointerId: 7 });
    dispatchPointer(separator, "pointerup", { pointerId: 7 });
    expect(onResizeCommit).toHaveBeenCalledOnce();
    expect(onResizeCommit).toHaveBeenCalledWith(420);
    expect(capture.releasePointerCapture).toHaveBeenCalledOnce();
    expect(capture.releasePointerCapture).toHaveBeenCalledWith(7);
  });

  it("cleans up pointer cancellation without double commit", () => {
    const onResizeCommit = vi.fn();
    const separator = renderSeparator({ onResizeCommit });
    const capture = installPointerCaptureStubs(separator);

    dispatchPointer(separator, "pointerdown", { pointerId: 8, isPrimary: true, button: 0, clientX: 248 });
    dispatchPointer(separator, "pointermove", { pointerId: 8, clientX: 300 });
    dispatchPointer(separator, "pointercancel", { pointerId: 8 });
    dispatchPointer(separator, "lostpointercapture", { pointerId: 8 });
    dispatchPointer(separator, "pointerup", { pointerId: 8 });

    expect(onResizeCommit).toHaveBeenCalledOnce();
    expect(onResizeCommit).toHaveBeenCalledWith(300);
    expect(capture.releasePointerCapture).toHaveBeenCalledOnce();
    expect(capture.captured).not.toContain(8);
  });

  it("commits lost capture once without trying to release missing capture", () => {
    const onResizeCommit = vi.fn();
    const separator = renderSeparator({ onResizeCommit });
    const capture = installPointerCaptureStubs(separator);

    dispatchPointer(separator, "pointerdown", { pointerId: 9, isPrimary: true, button: 0, clientX: 248 });
    dispatchPointer(separator, "pointermove", { pointerId: 9, clientX: 280 });
    capture.captured.delete(9);
    dispatchPointer(separator, "lostpointercapture", { pointerId: 9 });
    dispatchPointer(separator, "pointercancel", { pointerId: 9 });

    expect(onResizeCommit).toHaveBeenCalledOnce();
    expect(onResizeCommit).toHaveBeenCalledWith(280);
    expect(capture.hasPointerCapture).toHaveBeenCalledWith(9);
    expect(capture.releasePointerCapture).not.toHaveBeenCalled();
  });
});

describe("appVersionLabel", () => {
  it("prefixes bare versions and passes prefixed ones through", () => {
    expect(appVersionLabel("0.1.2")).toBe("v0.1.2");
    expect(appVersionLabel("v0.1.2")).toBe("v0.1.2");
  });

  it("stays a quiet dev for unset or placeholder values", () => {
    expect(appVersionLabel(undefined)).toBe("dev");
    expect(appVersionLabel("  ")).toBe("dev");
    expect(appVersionLabel("dev")).toBe("dev");
  });
});
