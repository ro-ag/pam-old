import { WarningCircle } from "@phosphor-icons/react";
import { AnimatePresence, MotionConfig, motion } from "motion/react";
import { Tooltip } from "radix-ui";
import {
  type CSSProperties,
  useCallback,
  useEffect,
  useReducer,
  useRef,
  useState,
} from "react";
import { answersFence, DAEMON_AUTHORITY, nextOperationId, sameFence, withDaemonOperation, withOperation } from "./bridge";
import type {
  ApprovalDecision,
  CommandFence,
  HealthDto,
  ModelStatusDto,
  PamBridge,
  SnapshotDto,
  ViewId,
} from "./domain";
import { MAX_EVIDENCE_TEXT } from "./domain";
import {
  clampSidebarWidth,
  readPersistedSidebarCollapsed,
  readPersistedSidebarWidth,
  writePersistedSidebarCollapsed,
  writePersistedSidebarWidth,
  type LayoutStorage,
} from "./layout";
import type { AgentBriefView } from "./selectors";
import { selectControlCenter, selectDaemonView } from "./selectors";
import { ProjectContextBar, ProjectPlaceholderView, ResizeSeparator, Sidebar, Toolbar } from "./components/Shell";
import {
  ApprovalDrawer,
  CommandPalette,
  type CommandPaletteCommand,
  EvidenceDrawer,
  LoadingScreen,
  ModelChatDrawer,
  QueueDrawer,
  RecoveryScreen,
} from "./components/Surfaces";
import { ActivityView } from "./views/ActivityView";
import { ConnectionsView } from "./views/ConnectionsView";
import { ControlCenterView } from "./views/ControlCenterView";
import { FlowsView } from "./views/FlowsView";
import { AccessView } from "./views/ProjectViews";
import { SkillsView } from "./views/SkillsView";
import { appReducer, initialState, presentError } from "./state";
import {
  applyPamTheme,
  readPersistedPamTheme,
  readPersistedPamThemeMode,
  writePersistedPamTheme,
  writePersistedPamThemeMode,
  type PamTheme,
  type PamThemeMode,
} from "./theme";
import {
  activeOverlay,
  createOverlayState,
  loadingEvidenceEntry,
  overlayLayer,
  overlayReducer,
  overlayStateForAuthority,
  type OverlayAuthority,
  type OverlayEntry,
} from "./overlays";

interface AppProps {
  bridge: PamBridge;
  initialView?: ViewId;
  initialTheme?: PamTheme;
  initialThemeMode?: PamThemeMode;
}

interface InitialLayout {
  viewportWidth: number;
  compactViewport: boolean;
  desktopSidebarCollapsed: boolean;
  sidebarWidth: number;
  storage: LayoutStorage | null;
}

type ShellStyle = CSSProperties & { "--sidebar-size": string };

function availableStorage(): LayoutStorage | null {
  try {
    return window.localStorage;
  } catch {
    return null;
  }
}

function readInitialLayout(): InitialLayout {
  const viewportWidth = window.innerWidth;
  const compactViewport = window.matchMedia("(max-width: 780px)").matches;
  const storage = availableStorage();
  const desktopSidebarCollapsed = readPersistedSidebarCollapsed(storage);
  return {
    viewportWidth,
    compactViewport,
    desktopSidebarCollapsed,
    sidebarWidth: readPersistedSidebarWidth(storage, viewportWidth),
    storage,
  };
}

function briefText(brief: AgentBriefView): string {
  return [
    brief.title,
    ...brief.sections.map((section) => `${section.label}: ${section.summary}`),
  ].join("\n");
}

const acceptsResponseFence = answersFence;

function sameAuthority(left: CommandFence, right: CommandFence): boolean {
  return left.projectHandle === right.projectHandle && left.generation === right.generation;
}

export function App({ bridge, initialView = "control-center", initialTheme, initialThemeMode }: AppProps) {
  const [initialLayout] = useState(readInitialLayout);
  const [theme, setTheme] = useState<PamTheme>(() => initialTheme ?? readPersistedPamTheme(initialLayout.storage));
  const [themeMode, setThemeMode] = useState<PamThemeMode>(() => initialThemeMode ?? readPersistedPamThemeMode(initialLayout.storage));
  const [viewportWidth, setViewportWidth] = useState(initialLayout.viewportWidth);
  const [compactViewport, setCompactViewport] = useState(initialLayout.compactViewport);
  const [state, dispatch] = useReducer(appReducer, {
    ...initialState,
    activeView: initialView,
    sidebarWidth: initialLayout.sidebarWidth,
    sidebarCollapsed: compactViewport ? true : initialLayout.desktopSidebarCollapsed,
  });
  const [overlays, dispatchOverlay] = useReducer(overlayReducer, null, createOverlayState);
  const [toast, setToast] = useState("");
  const [modelStatus, setModelStatus] = useState<ModelStatusDto | null>(null);
  const [daemonHealth, setDaemonHealth] = useState<HealthDto | null>(null);
  const [daemonBusy, setDaemonBusy] = useState(false);
  const [refreshTick, setRefreshTick] = useState(0);
  const toastTimer = useRef<number | null>(null);
  const evidenceRequestSequence = useRef(0);
  const modelRequestSequence = useRef(0);
  const healthRequestSequence = useRef(0);
  const modelChatReturnFocusRef = useRef<HTMLElement | null>(null);
  const dataCommandSequence = useRef(0);
  const bootstrapRequestSequence = useRef(0);
  const sidebarRef = useRef<HTMLElement>(null);
  const sidebarToggleRef = useRef<HTMLButtonElement>(null);
  const commandButtonRef = useRef<HTMLButtonElement>(null);
  const queueButtonRef = useRef<HTMLButtonElement>(null);
  const commandReturnFocusRef = useRef<HTMLElement | null>(null);
  const queueReturnFocusRef = useRef<HTMLElement | null>(null);
  const storageRef = useRef(initialLayout.storage);
  const desktopSidebarCollapsedRef = useRef(initialLayout.desktopSidebarCollapsed);
  const previousCompactViewportRef = useRef(initialLayout.compactViewport);
  const fenceRef = useRef(state.activeFence);
  fenceRef.current = state.activeFence;
  // With no active project the overlays run under the daemon authority, so
  // global surfaces (palette, model chat) stay available with zero projects.
  const overlayAuthority: OverlayAuthority | null = state.activeFence
    ? {
        projectHandle: state.activeFence.projectHandle,
        generation: state.activeFence.generation,
      }
    : state.catalog
      ? { projectHandle: DAEMON_AUTHORITY, generation: DAEMON_AUTHORITY }
      : null;
  const approvalHandle = state.data?.current.status === "approval_required"
    ? state.data.current.approval
    : null;
  const approvalKey = overlayAuthority && approvalHandle
    ? `${overlayAuthority.projectHandle}:${overlayAuthority.generation}:${approvalHandle}`
    : null;
  const effectiveOverlays = overlayStateForAuthority(overlays, overlayAuthority);

  useEffect(() => {
    dispatchOverlay({ type: "authority-changed", authority: overlayAuthority });
  }, [overlayAuthority?.generation, overlayAuthority?.projectHandle]);

  useEffect(() => {
    dispatchOverlay({
      type: "sync-approval",
      entry: overlayAuthority && approvalKey
        ? {
            id: `approval:${approvalKey}`,
            kind: "approval",
            authority: overlayAuthority,
            approvalKey,
          }
        : null,
    });
  }, [approvalKey, overlayAuthority?.generation, overlayAuthority?.projectHandle]);

  useEffect(() => () => {
    if (toastTimer.current) window.clearTimeout(toastTimer.current);
  }, []);

  useEffect(() => applyPamTheme(theme, themeMode), [theme, themeMode]);

  useEffect(() => {
    const query = window.matchMedia("(max-width: 780px)");
    const update = () => {
      setViewportWidth(window.innerWidth);
      setCompactViewport(query.matches);
    };
    window.addEventListener("resize", update);
    query.addEventListener("change", update);
    return () => {
      window.removeEventListener("resize", update);
      query.removeEventListener("change", update);
    };
  }, []);

  useEffect(() => {
    const wasCompact = previousCompactViewportRef.current;
    let reconciledCollapsed = state.sidebarCollapsed;
    if (!wasCompact && compactViewport) reconciledCollapsed = true;
    else if (wasCompact && !compactViewport) reconciledCollapsed = desktopSidebarCollapsedRef.current;

    const clampedWidth = clampSidebarWidth(state.sidebarWidth, viewportWidth);
    if (clampedWidth !== state.sidebarWidth) {
      dispatch({ type: "resizeSidebar", width: clampedWidth, viewportWidth });
      dispatch({ type: "setSidebarCollapsed", collapsed: reconciledCollapsed });
    } else if (reconciledCollapsed !== state.sidebarCollapsed) {
      dispatch({ type: "setSidebarCollapsed", collapsed: reconciledCollapsed });
    }
    previousCompactViewportRef.current = compactViewport;
  }, [compactViewport, state.sidebarCollapsed, state.sidebarWidth, viewportWidth]);

  const showToast = useCallback((message: string) => {
    setToast(message);
    if (toastTimer.current) window.clearTimeout(toastTimer.current);
    toastTimer.current = window.setTimeout(() => setToast(""), 2600);
  }, []);

  const selectTheme = useCallback((nextTheme: PamTheme) => {
    applyPamTheme(nextTheme, themeMode);
    setTheme(nextTheme);
    writePersistedPamTheme(storageRef.current, nextTheme);
  }, [themeMode]);

  const selectThemeMode = useCallback((nextMode: PamThemeMode) => {
    applyPamTheme(theme, nextMode);
    setThemeMode(nextMode);
    writePersistedPamThemeMode(storageRef.current, nextMode);
  }, [theme]);

  // The global daemon-health slice: probed at bootstrap, after lifecycle
  // actions, and on toolbar refresh. The daemon authority never uses a
  // project fence, so this works with zero projects.
  const loadDaemonHealth = useCallback(async () => {
    const sequence = ++healthRequestSequence.current;
    try {
      const health = await bridge.daemonHealth(withDaemonOperation());
      if (sequence === healthRequestSequence.current) setDaemonHealth(health);
    } catch (error) {
      if (sequence === healthRequestSequence.current) {
        setDaemonHealth({ status: "degraded", detail: presentError(error), recovery: null });
      }
    }
  }, [bridge]);

  const bootstrap = useCallback(async () => {
    const sequence = ++bootstrapRequestSequence.current;
    dataCommandSequence.current += 1;
    dispatch({ type: "retry" });
    try {
      const response = await bridge.bootstrap();
      if (sequence !== bootstrapRequestSequence.current) return;
      dispatch({ type: "bootstrapSucceeded", response });
      void loadDaemonHealth();
    } catch (error) {
      if (sequence !== bootstrapRequestSequence.current) return;
      const syntheticFence = { projectHandle: "bootstrap", generation: "", operationId: "bootstrap" };
      dispatch({ type: "commandStarted", fence: syntheticFence });
      dispatch({ type: "commandFailed", fence: syntheticFence, message: presentError(error) });
    }
  }, [bridge, loadDaemonHealth]);

  useEffect(() => { void bootstrap(); }, [bootstrap]);

  const executeDataCommand = useCallback(async (
    fence: CommandFence,
    command: () => Promise<SnapshotDto>,
    successMessage?: string,
  ) => {
    const sequence = ++dataCommandSequence.current;
    dispatch({ type: "commandStarted", fence });
    try {
      const response = await command();
      if (sequence !== dataCommandSequence.current) return false;
      if (!acceptsResponseFence(fence, response.fence)) {
        dispatch({ type: "commandFailed", fence, message: "The command response did not match the latest project operation. Retry safely." });
        return false;
      }
      dispatch({ type: "commandSucceeded", response });
      if (successMessage) showToast(successMessage);
      return true;
    } catch (error) {
      if (sequence !== dataCommandSequence.current) return false;
      dispatch({ type: "commandFailed", fence, message: presentError(error) });
      return false;
    }
  }, [showToast]);

  // Model status is daemon-global: always the daemon authority.
  const loadModelStatus = useCallback(async () => {
    const sequence = ++modelRequestSequence.current;
    try {
      const response = await bridge.modelStatus(withDaemonOperation());
      if (sequence === modelRequestSequence.current) setModelStatus(response);
    } catch (error) {
      if (sequence === modelRequestSequence.current) {
        setModelStatus({ status: "unavailable", failure: { code: null, detail: presentError(error), recovery: null } });
      }
    }
  }, [bridge]);

  const reloadModelStatus = useCallback(() => { void loadModelStatus(); }, [loadModelStatus]);

  const ready = state.catalog !== null;
  useEffect(() => {
    if (ready) void loadModelStatus();
  }, [ready, loadModelStatus]);

  // ⌘R refreshes the global health slice and the active view's loaders; the
  // project snapshot refresh happens only while a project is active.
  const refresh = useCallback(() => {
    void loadDaemonHealth();
    void loadModelStatus();
    setRefreshTick((tick) => tick + 1);
    if (!fenceRef.current) return;
    const fence = withOperation(fenceRef.current);
    void executeDataCommand(fence, () => bridge.refreshProject(fence), "Project state refreshed");
  }, [bridge, executeDataCommand, loadDaemonHealth, loadModelStatus]);

  const mobileSidebarOpen = compactViewport && !state.sidebarCollapsed;
  const toggleSidebar = useCallback(() => {
    const opening = state.sidebarCollapsed;
    const nextCollapsed = !state.sidebarCollapsed;
    dispatch({ type: "toggleSidebar" });
    if (!compactViewport) {
      desktopSidebarCollapsedRef.current = nextCollapsed;
      writePersistedSidebarCollapsed(storageRef.current, nextCollapsed);
      return;
    }
    window.requestAnimationFrame(() => {
      if (opening) sidebarRef.current?.querySelector<HTMLElement>("button:not(:disabled)")?.focus();
      else sidebarToggleRef.current?.focus();
    });
  }, [compactViewport, state.sidebarCollapsed]);

  const previewSidebarWidth = useCallback((width: number) => {
    dispatch({ type: "resizeSidebar", width, viewportWidth });
  }, [viewportWidth]);

  const commitSidebarWidth = useCallback((width: number) => {
    dispatch({ type: "resizeSidebar", width, viewportWidth });
    writePersistedSidebarWidth(storageRef.current, width, viewportWidth);
  }, [viewportWidth]);

  const closeActiveOverlay = useCallback(() => {
    const current = activeOverlay(effectiveOverlays);
    if (current?.kind === "evidence") evidenceRequestSequence.current += 1;
    dispatchOverlay({ type: "close-top" });
  }, [effectiveOverlays]);

  const openOverlay = useCallback((entry: OverlayEntry, replaceTop = false, reopenApproval = false) => {
    const current = activeOverlay(effectiveOverlays);
    if (replaceTop || current?.kind === "project") {
      dispatchOverlay({ type: "replace-top", entry });
    } else {
      dispatchOverlay({ type: "open", entry, reopenApproval });
    }
  }, [effectiveOverlays]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && !event.shiftKey && !event.altKey) {
        if (event.key.toLowerCase() === "k") {
          event.preventDefault();
          if (overlayAuthority && !activeOverlay(effectiveOverlays)) {
            commandReturnFocusRef.current = commandButtonRef.current;
            openOverlay({ id: "command", kind: "command", authority: overlayAuthority });
          }
          return;
        }
        if (activeOverlay(effectiveOverlays)) return;
        const view = event.key === "1"
          ? "control-center"
          : event.key === "2"
            ? "access"
            : event.key === "3"
              ? "skills"
              : event.key === "4"
                ? "flows"
                : event.key === "5"
                  ? "activity"
                  : event.key === "6"
                    ? "callers"
                    : null;
        if (view) { event.preventDefault(); dispatch({ type: "navigate", view }); }
        if (event.key.toLowerCase() === "r") { event.preventDefault(); refresh(); }
      }
      if (event.key === "Escape" && !activeOverlay(effectiveOverlays) && mobileSidebarOpen) toggleSidebar();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [effectiveOverlays, mobileSidebarOpen, openOverlay, overlayAuthority, refresh, toggleSidebar]);

  // The shell renders as soon as bootstrap returns: the LoadingScreen only
  // exists pre-bootstrap and the RecoveryScreen only for real bootstrap
  // errors. A missing snapshot is the global-first mode, not a failure.
  if (!state.catalog) {
    if (state.loadState === "recovering") {
      return <RecoveryScreen message={state.error ?? "The daemon observatory is unavailable."} onRetry={() => void bootstrap()} />;
    }
    return <LoadingScreen />;
  }

  const projectData = state.data ? selectControlCenter(state.data, state.catalog, bridge.mode === "fixture") : null;
  const projectActive = projectData !== null && state.activeFence !== null;
  const daemon = projectData?.daemon ?? selectDaemonView(daemonHealth);
  const pending = state.pendingFence !== null;
  const busy = pending || daemonBusy;
  const selectProject = (project: { handle: string; name: string }) => {
    if (project.handle === state.data?.project.handle) return;
    const operationId = nextOperationId();
    const pendingFence = { projectHandle: project.handle, generation: "", operationId };
    void executeDataCommand(pendingFence, () => bridge.activateProject(project.handle, operationId), `Now watching ${project.name}`)
      .then((activated) => { if (activated) dispatch({ type: "navigate", view: "control-center" }); });
    if (mobileSidebarOpen) toggleSidebar();
  };
  // Daemon lifecycle always runs under the daemon authority. The response is
  // no snapshot, so we re-probe daemon_health; with a project active, the
  // project refresh keeps the snapshot fresh too.
  const runDaemonLifecycle = async (command: () => Promise<unknown>, successMessage: string) => {
    setDaemonBusy(true);
    try {
      await command();
      showToast(successMessage);
    } catch (error) {
      showToast(presentError(error));
    } finally {
      setDaemonBusy(false);
    }
    void loadDaemonHealth();
    if (fenceRef.current) {
      const fence = withOperation(fenceRef.current);
      void executeDataCommand(fence, () => bridge.refreshProject(fence));
    }
  };
  const toggleDaemon = () => {
    const stopping = daemon.state === "running";
    void runDaemonLifecycle(
      () => stopping ? bridge.stopDaemon(withDaemonOperation()) : bridge.startDaemon(withDaemonOperation()),
      stopping ? "PAM is paused" : "PAM is back on watch",
    );
  };
  const restartDaemon = () => {
    void runDaemonLifecycle(async () => {
      await bridge.stopDaemon(withDaemonOperation());
      await bridge.startDaemon(withDaemonOperation());
    }, "PAM restarted");
  };
  const registerGuiCaller = () => {
    const fence = withOperation(state.activeFence!);
    void executeDataCommand(
      fence,
      () => bridge.registerGuiCaller(fence).catch(() => {
        throw new Error("GUI caller registration could not be completed. Retry from this screen.");
      }),
      "GUI caller registered",
    );
  };
  const loadEvidence = async (handle: string) => {
    if (!fenceRef.current || !overlayAuthority) return;
    const requestId = ++evidenceRequestSequence.current;
    const fence = withOperation(fenceRef.current);
    const entryId = `evidence:${handle}`;
    const identity = { entryId, requestId, handle, authority: overlayAuthority };
    openOverlay(loadingEvidenceEntry({ id: entryId, requestId, handle, authority: overlayAuthority }));
    try {
      const response = await bridge.loadEvidence(fence, handle);
      if (requestId !== evidenceRequestSequence.current || !fenceRef.current || !sameAuthority(fence, fenceRef.current)) return;
      if (!sameFence(fence, response.fence)) {
        dispatchOverlay({
          type: "evidence-failed",
          ...identity,
          error: "The active project changed. Reopen this evidence from the refreshed outcome.",
          retryable: false,
        });
        return;
      }
      dispatchOverlay({
        type: "evidence-loaded",
        ...identity,
        document: { ...response.data, body: response.data.body?.slice(0, MAX_EVIDENCE_TEXT) ?? null },
      });
    } catch (error) {
      if (requestId !== evidenceRequestSequence.current || !fenceRef.current || !sameAuthority(fence, fenceRef.current)) return;
      dispatchOverlay({ type: "evidence-failed", ...identity, error: presentError(error) });
    }
  };
  const copyBrief = async (brief: AgentBriefView) => {
    try {
      await navigator.clipboard.writeText(briefText(brief));
      showToast("Agent brief copied");
    } catch {
      showToast("Clipboard access is unavailable");
    }
  };
  const decide = async (decision: ApprovalDecision) => {
    const approval = projectData?.current.approval;
    if (!approval || !state.activeFence) return;
    const fence = withOperation(state.activeFence!);
    const sequence = ++dataCommandSequence.current;
    dispatch({ type: "commandStarted", fence });
    try {
      const response = await bridge.decideApproval(fence, approval.approvalHandle, decision);
      if (sequence !== dataCommandSequence.current) return;
      if (!acceptsResponseFence(fence, response.snapshot.fence)) {
        dispatch({ type: "commandFailed", fence, message: "The approval response did not match the latest project operation. Retry safely." });
        return;
      }
      dispatch({ type: "commandSucceeded", response: response.snapshot });
      showToast(response.disposition === "approved"
        ? "Exact request approved"
        : response.disposition === "denied"
          ? "Request denied"
          : "Approval expired; request a new challenge");
      closeActiveOverlay();
    } catch (error) {
      if (sequence !== dataCommandSequence.current) return;
      dispatch({ type: "commandFailed", fence, message: presentError(error) });
    }
  };

  const currentOverlay = activeOverlay(effectiveOverlays);
  const applicationOverlayOpen = currentOverlay !== null && currentOverlay.kind !== "project";
  const openQueue = (returnFocusTarget?: HTMLElement) => {
    if (!overlayAuthority || !projectActive) return;
    const activeElement = document.activeElement instanceof HTMLElement && document.activeElement !== document.body
      ? document.activeElement
      : null;
    queueReturnFocusRef.current = returnFocusTarget ?? activeElement ?? queueButtonRef.current;
    openOverlay({ id: "queue", kind: "queue", authority: overlayAuthority });
  };
  const chatModelId = modelStatus?.status === "ok"
    ? (modelStatus.loaded ?? modelStatus.registered[0])?.modelId ?? null
    : null;
  const openModelChat = (modelId: string, returnFocusTarget?: HTMLElement) => {
    if (!overlayAuthority) return;
    const activeElement = document.activeElement instanceof HTMLElement && document.activeElement !== document.body
      ? document.activeElement
      : null;
    modelChatReturnFocusRef.current = returnFocusTarget ?? activeElement;
    openOverlay({ id: "model-chat", kind: "model-chat", authority: overlayAuthority, modelId });
  };
  const openCommandPalette = (returnFocusTarget?: HTMLElement) => {
    if (!overlayAuthority || currentOverlay) return;
    commandReturnFocusRef.current = returnFocusTarget ?? commandButtonRef.current;
    openOverlay({ id: "command", kind: "command", authority: overlayAuthority });
  };
  const setProjectMenuOpen = (open: boolean) => {
    if (!overlayAuthority) return;
    if (open && !currentOverlay) {
      openOverlay({ id: "project", kind: "project", authority: overlayAuthority });
    } else if (!open) {
      dispatchOverlay({ type: "remove", entryId: "project" });
    }
  };
  const commands: CommandPaletteCommand[] = [
    { id: "view-control-center", label: "Open Control Center", description: "Show the project's runs, requests, and outcomes.", shortcut: "⌘1" },
    { id: "view-access", label: "Open Access", description: "Show the project's authorized capabilities.", shortcut: "⌘2" },
    { id: "view-skills", label: "Open Skills", description: "Show the skill inventory, library, and audit.", shortcut: "⌘3" },
    { id: "view-flows", label: "Open Flows", description: "Show bounded project flow definitions.", shortcut: "⌘4" },
    { id: "view-activity", label: "Open Activity", description: "Show daemon health and the recent activity feed.", shortcut: "⌘5" },
    { id: "view-callers", label: "Open Connections", description: "Show the callers and connectors linked to the daemon.", shortcut: "⌘6" },
    ...(projectActive
      ? [{ id: "open-queue", label: "Open project queue", description: "Inspect the bounded retained request window." }]
      : []),
    ...(chatModelId
      ? [{ id: "model-chat", label: "Chat with the model", description: "Review the local model in an ephemeral chat." }]
      : []),
    projectActive
      ? { id: "refresh", label: "Refresh project", description: "Request current state from PAM.", shortcut: "⌘R" }
      : { id: "refresh", label: "Refresh daemon", description: "Probe daemon health and reload the global views.", shortcut: "⌘R" },
  ];
  const runCommand = (id: string) => {
    const view = id === "view-control-center"
      ? "control-center"
      : id === "view-access"
        ? "access"
        : id === "view-skills"
          ? "skills"
          : id === "view-flows"
            ? "flows"
            : id === "view-activity"
              ? "activity"
              : id === "view-callers"
                ? "callers"
                : null;
    if (view) {
      dispatch({ type: "navigate", view });
      closeActiveOverlay();
    } else if (id === "open-queue" && overlayAuthority) {
      queueReturnFocusRef.current = commandReturnFocusRef.current?.isConnected ? commandReturnFocusRef.current : null;
      openOverlay({ id: "queue", kind: "queue", authority: overlayAuthority }, true);
    } else if (id === "model-chat" && overlayAuthority && chatModelId) {
      modelChatReturnFocusRef.current = commandReturnFocusRef.current?.isConnected ? commandReturnFocusRef.current : null;
      openOverlay({ id: "model-chat", kind: "model-chat", authority: overlayAuthority, modelId: chatModelId }, true);
    } else if (id === "refresh") {
      closeActiveOverlay();
      refresh();
    }
  };

  // Project context is contextual: this bar renders in the headers of the
  // four project-shaped views, never in the global sidebar chrome.
  const projectContextBar = projectData ? (
    <ProjectContextBar
      project={projectData.project}
      projects={projectData.catalog}
      menuOpen={effectiveOverlays.stack.some(({ kind }) => kind === "project")}
      onMenuOpenChange={setProjectMenuOpen}
      onSelect={selectProject}
    />
  ) : null;
  const projectPlaceholder = (title: string, subtitle: string) => (
    <ProjectPlaceholderView
      title={title}
      subtitle={subtitle}
      projects={state.catalog?.projects ?? []}
      onSelect={selectProject}
    />
  );

  const shellWidth = state.sidebarCollapsed ? 68 : state.sidebarWidth;
  const shellStyle: ShellStyle = { "--sidebar-size": `${shellWidth}px` };
  return (
    <MotionConfig reducedMotion="user">
      <Tooltip.Provider delayDuration={350} skipDelayDuration={150}>
      <div className="app-root" data-theme={theme} data-mode={themeMode}>
      <div className="app-shell" style={shellStyle} inert={applicationOverlayOpen || undefined} aria-hidden={applicationOverlayOpen || undefined}>
        <div className="atmosphere" aria-hidden="true" />
        <a className="skip-link" href="#main-content" tabIndex={mobileSidebarOpen ? -1 : undefined}>Skip to content</a>
        <Sidebar
          containerRef={sidebarRef}
          daemon={daemon}
          queueCount={projectData?.current.queue.length ?? 0}
          activeView={state.activeView}
          collapsed={state.sidebarCollapsed}
          pending={busy}
          trapFocus={mobileSidebarOpen}
          onNavigate={(view) => { dispatch({ type: "navigate", view }); if (mobileSidebarOpen) toggleSidebar(); }}
          onToggleDaemon={toggleDaemon}
          onRestartDaemon={restartDaemon}
          onDismiss={toggleSidebar}
        />
        {mobileSidebarOpen && <button type="button" className="sidebar-scrim" aria-label="Close project sidebar" tabIndex={-1} onClick={toggleSidebar} />}
        <ResizeSeparator collapsed={state.sidebarCollapsed || compactViewport} width={state.sidebarWidth} viewportWidth={viewportWidth} onResizePreview={previewSidebarWidth} onResizeCommit={commitSidebarWidth} onToggle={toggleSidebar} />
        <section className="workspace" inert={mobileSidebarOpen || undefined} aria-hidden={mobileSidebarOpen || undefined}>
          <Toolbar toggleButtonRef={sidebarToggleRef} commandButtonRef={commandButtonRef} queueButtonRef={queueButtonRef} projectName={projectData?.project.name ?? null} queueCount={projectData ? projectData.current.queue.length : null} fixture={projectData?.fixture ?? bridge.mode === "fixture"} collapsed={state.sidebarCollapsed} pending={busy} theme={theme} themeMode={themeMode} onThemeChange={selectTheme} onThemeModeChange={selectThemeMode} onToggleSidebar={toggleSidebar} onOpenCommand={openCommandPalette} onRefresh={refresh} onOpenQueue={openQueue} />
          <div className="workspace-body">
          {state.loadState === "recovering" && state.error && <div className="inline-recovery" role="alert"><WarningCircle size={18} /><span>{state.error}</span><button type="button" onClick={refresh}>Retry</button></div>}
          <AnimatePresence mode="wait" initial={false}>
            <motion.div
              className="view-transition"
              key={state.activeView}
              initial={{ opacity: 0, y: 8 }}
              animate={{ opacity: 1, y: 0 }}
              exit={{ opacity: 0, y: -6 }}
              transition={{ duration: 0.24, ease: [0.33, 1, 0.68, 1] }}
            >
              {state.activeView === "control-center" && (projectData && state.activeFence ? (
                <ControlCenterView
                  contextBar={projectContextBar}
                  data={projectData}
                  onCopy={(brief) => void copyBrief(brief)}
                  onEvidence={(handle) => void loadEvidence(handle)}
                  onContinue={() => { dispatch({ type: "navigate", view: "flows" }); showToast("Flow workspace opened"); }}
                  onOpenQueue={openQueue}
                  onOpenApproval={() => { if (approvalKey && overlayAuthority) openOverlay({ id: `approval:${approvalKey}`, kind: "approval", authority: overlayAuthority, approvalKey }, false, true); }}
                  onRecoverDaemon={toggleDaemon}
                  onRefresh={refresh}
                  onRegisterCaller={registerGuiCaller}
                  registrationBusy={busy}
                />
              ) : projectPlaceholder("Control center", "The project's queue, runs, and outcomes in one calm place."))}
              {state.activeView === "access" && (projectData
                ? <AccessView contextBar={projectContextBar} data={projectData} />
                : projectPlaceholder("Access", "Narrow capabilities, visible to the developer."))}
              {state.activeView === "skills" && (projectData && state.activeFence ? (
                <SkillsView
                  key={`${state.activeFence.projectHandle}:${state.activeFence.generation}`}
                  bridge={bridge}
                  fence={state.activeFence}
                  contextBar={projectContextBar}
                />
              ) : projectPlaceholder("Skills", "What the agents carry with them, kept in view."))}
              {state.activeView === "flows" && (projectData && state.activeFence
                ? <FlowsView bridge={bridge} fence={state.activeFence} contextBar={projectContextBar} onError={showToast} onToast={showToast} />
                : projectPlaceholder("Flows", "Repeatable work, with meaningful feedback."))}
              {state.activeView === "activity" && (
                <ActivityView
                  key={`activity:${refreshTick}`}
                  daemon={daemon}
                  projects={state.catalog.projects}
                  bridge={bridge}
                  pending={busy}
                  modelStatus={modelStatus}
                  onReloadModel={reloadModelStatus}
                  onOpenModelChat={openModelChat}
                  onStartDaemon={toggleDaemon}
                />
              )}
              {state.activeView === "callers" && (
                <ConnectionsView key={`connections:${refreshTick}`} bridge={bridge} />
              )}
            </motion.div>
          </AnimatePresence>
          </div>
        </section>
      </div>
      {effectiveOverlays.stack.map((entry) => {
        if (entry.kind === "project") return null;
        const active = overlayLayer(effectiveOverlays, entry.id) === "active";
        if (entry.kind === "queue") {
          if (!projectData) return null;
          return <QueueDrawer active={active} data={projectData} key={entry.id} returnFocusTarget={queueReturnFocusRef.current} onClose={closeActiveOverlay} />;
        }
        if (entry.kind === "approval") {
          if (!projectData?.current.approval || entry.approvalKey !== approvalKey) return null;
          return <ApprovalDrawer active={active} busy={pending} data={projectData} error={state.error} key={entry.id} onDecision={(decision) => { void decide(decision); }} onClose={closeActiveOverlay} />;
        }
        if (entry.kind === "evidence") {
          return <EvidenceDrawer active={active} document={entry.document} loading={entry.loading} error={entry.error} key={entry.id} onRetry={entry.retryable ? () => { void loadEvidence(entry.handle); } : undefined} onClose={closeActiveOverlay} />;
        }
        if (entry.kind === "model-chat") {
          return <ModelChatDrawer active={active} bridge={bridge} modelId={entry.modelId} key={entry.id} returnFocusTarget={modelChatReturnFocusRef.current} onClose={closeActiveOverlay} />;
        }
        return <CommandPalette active={active} commands={commands} key={entry.id} returnFocusTarget={commandReturnFocusRef.current} onAction={runCommand} onClose={closeActiveOverlay} />;
      })}
      {toast && <div className="toast" role="status">{toast}</div>}
      </div>
      </Tooltip.Provider>
    </MotionConfig>
  );
}
