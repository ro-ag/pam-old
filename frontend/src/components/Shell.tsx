import {
  ArrowClockwise,
  BookOpen,
  CaretDown,
  CaretRight,
  Check,
  Circle,
  GitBranch,
  LockSimple,
  MagnifyingGlass,
  MoonStars,
  PlugsConnected,
  Power,
  Pulse,
  Queue,
  SidebarSimple,
  SquaresFour,
  SunHorizon,
  Terminal,
} from "@phosphor-icons/react";
import { DropdownMenu, Tooltip, VisuallyHidden } from "radix-ui";
import {
  Fragment,
  type KeyboardEvent as ReactKeyboardEvent,
  type PointerEvent as ReactPointerEvent,
  type ReactNode,
  type RefObject,
  useEffect,
  useRef,
} from "react";
import type { ProjectSummaryDto, ViewId } from "../domain";
import {
  clampSidebarWidth,
  minimumSidebarWidth,
  sidebarMaximumWidth,
  sidebarWidthFromKey,
} from "../layout";
import type { DaemonView, ProjectView } from "../selectors";
import type { PamDensity, PamTheme, PamThemeMode } from "../theme";

// Mirrors p-track's version label: bare versions gain a "v" prefix and
// anything unset stays a quiet "dev".
export function appVersionLabel(value: unknown): string {
  if (typeof value !== "string") return "dev";
  const version = value.trim();
  if (!version || version.toLowerCase() === "dev") return "dev";
  return version.toLowerCase().startsWith("v") ? version : `v${version}`;
}

export const navItems: ReadonlyArray<{ id: ViewId; label: string; icon: typeof Pulse }> = [
  { id: "control-center", label: "Control Center", icon: SquaresFour },
  { id: "access", label: "Access", icon: LockSimple },
  { id: "skills", label: "Skills", icon: BookOpen },
  { id: "flows", label: "Flows", icon: GitBranch },
  { id: "activity", label: "Activity", icon: Pulse },
  { id: "console", label: "Console", icon: Terminal },
  { id: "callers", label: "Connections", icon: PlugsConnected },
];

export function StatusDot({ state = "coral" }: { state?: "coral" | "aqua" | "muted" }) {
  return <Circle className={`status-dot status-dot--${state}`} size={12} weight="fill" aria-hidden="true" />;
}

function IconTooltip({ label, children }: { label: string; children: ReactNode }) {
  return (
    <Tooltip.Root>
      <Tooltip.Trigger asChild>{children}</Tooltip.Trigger>
      <Tooltip.Portal>
        <Tooltip.Content className="tooltip-content" sideOffset={8}>
          {label}
          <Tooltip.Arrow className="tooltip-arrow" />
        </Tooltip.Content>
      </Tooltip.Portal>
    </Tooltip.Root>
  );
}

export function ProjectMenu({
  active,
  projects,
  open,
  onOpenChange,
  onSelect,
}: {
  active: ProjectView;
  projects: ProjectView[];
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onSelect: (project: ProjectView) => void;
}) {
  const triggerRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (open) {
      window.requestAnimationFrame(() => menuRef.current?.querySelector<HTMLElement>('[data-state="checked"]')?.focus());
    }
  }, [open]);

  return (
    <div className="project-menu-wrap">
      <DropdownMenu.Root open={open} onOpenChange={onOpenChange}>
        <DropdownMenu.Trigger asChild>
          <button
          ref={triggerRef}
          type="button"
          className="project-switcher"
          >
            <GitBranch size={19} aria-hidden="true" />
            <span>{active.name}</span>
            <CaretDown size={16} weight="bold" aria-hidden="true" />
          </button>
        </DropdownMenu.Trigger>
        <DropdownMenu.Portal>
          <DropdownMenu.Content
            ref={menuRef}
            className="project-menu-popover project-menu"
            aria-label="Registered projects"
            align="start"
            sideOffset={8}
            onCloseAutoFocus={(event) => {
              event.preventDefault();
              triggerRef.current?.focus();
            }}
          >
            <DropdownMenu.RadioGroup value={active.handle}>
            {projects.map((project) => (
              <DropdownMenu.RadioItem
                className="project-menu-item"
                key={project.handle}
                textValue={project.name}
                value={project.handle}
                onSelect={() => onSelect(project)}
              >
                <span className={`health-dot health-dot--${project.health}`} aria-hidden="true" />
                <span>
                  <strong>{project.name}</strong>
                  <small>{project.branch ?? project.rootLabel}</small>
                  <VisuallyHidden.Root>Health: {project.health}</VisuallyHidden.Root>
                </span>
                <DropdownMenu.ItemIndicator><Check size={15} weight="bold" aria-hidden="true" /></DropdownMenu.ItemIndicator>
              </DropdownMenu.RadioItem>
            ))}
            </DropdownMenu.RadioGroup>
          </DropdownMenu.Content>
        </DropdownMenu.Portal>
      </DropdownMenu.Root>
    </div>
  );
}

// Project context is contextual: this bar sits in the headers of the four
// project-shaped views instead of the global sidebar chrome.
export function ProjectContextBar({
  project,
  projects,
  menuOpen,
  onMenuOpenChange,
  onSelect,
}: {
  project: ProjectView;
  projects: ProjectView[];
  menuOpen: boolean;
  onMenuOpenChange: (open: boolean) => void;
  onSelect: (project: ProjectView) => void;
}) {
  return (
    <div className="project-context-bar">
      <ProjectMenu
        active={project}
        projects={projects}
        open={menuOpen}
        onOpenChange={onMenuOpenChange}
        onSelect={onSelect}
      />
      <span className="project-context-location">{project.rootLabel}</span>
    </div>
  );
}

// The inline project picker, shared by the project-shaped empty state and by
// the panels that only gate part of their surface on an active project.
export function ProjectPicker({
  projects,
  onSelect,
}: {
  projects: ProjectSummaryDto[];
  onSelect: (project: ProjectSummaryDto) => void;
}) {
  return (
    <div className="project-picker">
      {projects.map((project) => (
        <button type="button" className="button button--secondary" key={project.handle} onClick={() => onSelect(project)}>
          <GitBranch size={17} aria-hidden="true" />
          <span>{project.name}</span>
          <small>{project.location}</small>
        </button>
      ))}
    </div>
  );
}

// The calm project-shaped empty state: an inline picker when projects exist,
// and a gentle discovery hint when the catalog is empty.
export function ProjectPlaceholderView({
  title,
  subtitle,
  projects,
  onSelect,
}: {
  title: string;
  subtitle: string;
  projects: ProjectSummaryDto[];
  onSelect: (project: ProjectSummaryDto) => void;
}) {
  return (
    <main className="canvas" id="main-content">
      <header className="project-header compact">
        <div><h1>{title}</h1><p>{subtitle}</p></div>
      </header>
      <section className="empty-state">
        <GitBranch size={38} aria-hidden="true" />
        {projects.length === 0 ? (
          <>
            <h2>No projects discovered yet</h2>
            <p>Open PAM from a Git repository and it will settle in here on its own. The daemon keeps watch either way.</p>
          </>
        ) : (
          <>
            <h2>Pick a project to bring its queue into view.</h2>
            <ProjectPicker projects={projects} onSelect={onSelect} />
          </>
        )}
      </section>
    </main>
  );
}

export function Sidebar({
  daemon,
  queueCount,
  activeView,
  collapsed,
  pending,
  trapFocus,
  onNavigate,
  onToggleDaemon,
  onRestartDaemon,
  onDismiss,
  containerRef,
}: {
  daemon: DaemonView;
  queueCount: number;
  activeView: ViewId;
  collapsed: boolean;
  pending: boolean;
  trapFocus: boolean;
  onNavigate: (view: ViewId) => void;
  onToggleDaemon: () => void;
  onRestartDaemon: () => void;
  onDismiss: () => void;
  containerRef: RefObject<HTMLElement | null>;
}) {
  const trapTabFocus = (event: ReactKeyboardEvent<HTMLElement>) => {
    if (trapFocus && event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      onDismiss();
      return;
    }
    if (!trapFocus || event.key !== "Tab") return;
    const sidebar = event.currentTarget;
    const focusable = Array.from(sidebar.querySelectorAll<HTMLElement>([
      "a[href]",
      "button:not(:disabled)",
      "input:not(:disabled)",
      "select:not(:disabled)",
      "textarea:not(:disabled)",
      '[tabindex]:not([tabindex="-1"])',
    ].join(","))).filter((element) => !element.hidden && element.getAttribute("aria-hidden") !== "true");
    if (focusable.length === 0) return;
    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    if ((event.shiftKey && document.activeElement === first) || (!event.shiftKey && document.activeElement === last)) {
      event.preventDefault();
      (event.shiftKey ? last : first).focus();
    }
  };
  return (
    <aside ref={containerRef} className={`sidebar ${collapsed ? "is-collapsed" : ""}`} aria-label="Daemon navigation" onKeyDownCapture={trapTabFocus}>
      <div className="brand" aria-label="PAM" data-tauri-drag-region="deep">
        <img src="/assets/pam-mark.png" alt="" />
        {!collapsed && (
          <div className="brand-identity">
            <span>PAM</span>
            <small className="app-version">
              {appVersionLabel(typeof __APP_VERSION__ === "undefined" ? "dev" : __APP_VERSION__)}
            </small>
          </div>
        )}
      </div>
      <nav className="primary-nav" aria-label="Primary">
        {navItems.map(({ id, label, icon: Icon }) => (
          <Fragment key={id}>
            {id === "activity" && <div className="nav-separator" role="separator" aria-hidden="true" />}
            <button
              type="button"
              className={`nav-item ${activeView === id ? "is-active" : ""}`}
              aria-current={activeView === id ? "page" : undefined}
              aria-label={label}
              title={collapsed ? label : undefined}
              onClick={() => onNavigate(id)}
            >
              <Icon size={21} weight={activeView === id ? "bold" : "regular"} aria-hidden="true" />
              {!collapsed && <span>{label}</span>}
              {!collapsed && id === "control-center" && queueCount > 0 && (
                <span className="nav-count" aria-label={`${queueCount} queued`}>
                  {queueCount}
                </span>
              )}
            </button>
          </Fragment>
        ))}
      </nav>
      <div className="sidebar-footer">
        <div className="daemon-row">
          <button
            type="button"
            className="daemon-control"
            aria-pressed={daemon.state === "running"}
            aria-label={collapsed ? daemon.detail : undefined}
            title={collapsed ? daemon.detail : undefined}
            disabled={pending || ["starting", "stopping", "unavailable"].includes(daemon.state)}
            onClick={onToggleDaemon}
          >
            {daemon.state === "running" ? <StatusDot /> : <Power size={18} weight="bold" aria-hidden="true" />}
            {!collapsed && <span>{daemon.detail}</span>}
          </button>
          {!collapsed && daemon.state === "running" && (
            <button
              type="button"
              className="daemon-restart"
              aria-label="Restart PAM"
              title="Restart PAM"
              disabled={pending}
              onClick={onRestartDaemon}
            >
              <ArrowClockwise size={16} weight="bold" aria-hidden="true" />
            </button>
          )}
        </div>
        <div className="utility-nav">
          <button type="button" aria-label="Documentation unavailable in this preview" title="Documentation unavailable in this preview" disabled><BookOpen size={19} /></button>
        </div>
      </div>
    </aside>
  );
}

export function ResizeSeparator({
  collapsed,
  width,
  viewportWidth,
  onResizePreview,
  onResizeCommit,
  onToggle,
}: {
  collapsed: boolean;
  width: number;
  viewportWidth: number;
  onResizePreview: (width: number) => void;
  onResizeCommit: (width: number) => void;
  onToggle: () => void;
}) {
  const start = useRef<{ pointerId: number; x: number; width: number; latestWidth: number } | null>(null);
  const onPointerDown = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (collapsed || !event.isPrimary || event.button !== 0 || start.current) return;
    event.preventDefault();
    start.current = { pointerId: event.pointerId, x: event.clientX, width, latestWidth: width };
    event.currentTarget.setPointerCapture(event.pointerId);
  };
  const onPointerMove = (event: ReactPointerEvent<HTMLDivElement>) => {
    const active = start.current;
    if (!active || active.pointerId !== event.pointerId || collapsed) return;
    const nextWidth = clampSidebarWidth(active.width + event.clientX - active.x, viewportWidth);
    active.latestWidth = nextWidth;
    onResizePreview(nextWidth);
  };
  const finishPointer = (event: ReactPointerEvent<HTMLDivElement>) => {
    const active = start.current;
    if (!active || active.pointerId !== event.pointerId) return;
    start.current = null;
    if (event.currentTarget.hasPointerCapture(active.pointerId)) {
      event.currentTarget.releasePointerCapture(active.pointerId);
    }
    onResizeCommit(active.latestWidth);
  };
  const onKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    if (collapsed) return;
    if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      onToggle();
      return;
    }
    const nextWidth = sidebarWidthFromKey(width, event.key, viewportWidth);
    if (nextWidth !== null) {
      event.preventDefault();
      onResizeCommit(nextWidth);
    }
  };
  const maximumWidth = sidebarMaximumWidth(viewportWidth);
  const currentWidth = clampSidebarWidth(width, viewportWidth);
  return (
    <div
      className="resize-separator"
      role="separator"
      aria-orientation="vertical"
      aria-valuemin={minimumSidebarWidth}
      aria-valuemax={maximumWidth}
      aria-valuenow={currentWidth}
      aria-disabled={collapsed}
      aria-label="Resize project sidebar"
      tabIndex={collapsed ? -1 : 0}
      onDoubleClick={collapsed ? undefined : onToggle}
      onKeyDown={onKeyDown}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={finishPointer}
      onPointerCancel={finishPointer}
      onLostPointerCapture={finishPointer}
    />
  );
}

export function ThemeMenu({
  theme,
  themeMode,
  density,
  onThemeChange,
  onThemeModeChange,
  onDensityChange,
}: {
  theme: PamTheme;
  themeMode: PamThemeMode;
  density: PamDensity;
  onThemeChange: (theme: PamTheme) => void;
  onThemeModeChange: (mode: PamThemeMode) => void;
  onDensityChange: (density: PamDensity) => void;
}) {
  return (
    <DropdownMenu.Root>
      <DropdownMenu.Trigger asChild>
        <button
          type="button"
          className="theme-trigger"
          aria-label={`Theme: ${theme === "ventisquero" ? "Ventisquero" : "Viña del Mar"} · ${themeMode}`}
          title="Choose appearance theme"
        >
          {themeMode === "light"
            ? <SunHorizon size={19} weight="bold" aria-hidden="true" />
            : <MoonStars size={19} weight="bold" aria-hidden="true" />}
        </button>
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content className="theme-menu-popover" align="end" sideOffset={8}>
          <DropdownMenu.Label className="theme-menu-label">Theme</DropdownMenu.Label>
          <DropdownMenu.RadioGroup value={theme} onValueChange={(value) => onThemeChange(value as PamTheme)}>
            <DropdownMenu.RadioItem className="theme-menu-item" value="ventisquero" textValue="Ventisquero">
              <span className="theme-swatch theme-swatch--ventisquero" aria-hidden="true" />
              <span><strong>Ventisquero</strong><small>Rock · Ice · Copper · Mist</small></span>
              <DropdownMenu.ItemIndicator><Check size={15} weight="bold" aria-hidden="true" /></DropdownMenu.ItemIndicator>
            </DropdownMenu.RadioItem>
            <DropdownMenu.RadioItem className="theme-menu-item" value="vina" textValue="Viña del Mar">
              <span className="theme-swatch theme-swatch--vina" aria-hidden="true" />
              <span><strong>Viña del Mar</strong><small>Night · Violet · Coral · Surf</small></span>
              <DropdownMenu.ItemIndicator><Check size={15} weight="bold" aria-hidden="true" /></DropdownMenu.ItemIndicator>
            </DropdownMenu.RadioItem>
          </DropdownMenu.RadioGroup>
          <DropdownMenu.Separator className="theme-menu-separator" />
          <DropdownMenu.Label className="theme-menu-label">Variant</DropdownMenu.Label>
          <DropdownMenu.RadioGroup value={themeMode} onValueChange={(value) => onThemeModeChange(value as PamThemeMode)}>
            <DropdownMenu.RadioItem className="theme-menu-item theme-menu-item--compact" value="light" textValue="Light variant">
              <SunHorizon size={19} aria-hidden="true" />
              <span><strong>Light</strong><small>{theme === "ventisquero" ? "Mist" : "Dawn"}</small></span>
              <DropdownMenu.ItemIndicator><Check size={15} weight="bold" aria-hidden="true" /></DropdownMenu.ItemIndicator>
            </DropdownMenu.RadioItem>
            <DropdownMenu.RadioItem className="theme-menu-item theme-menu-item--compact" value="dark" textValue="Dark variant">
              <MoonStars size={19} aria-hidden="true" />
              <span><strong>Dark</strong><small>{theme === "ventisquero" ? "Bedrock" : "Night"}</small></span>
              <DropdownMenu.ItemIndicator><Check size={15} weight="bold" aria-hidden="true" /></DropdownMenu.ItemIndicator>
            </DropdownMenu.RadioItem>
          </DropdownMenu.RadioGroup>
          <DropdownMenu.Separator className="theme-menu-separator" />
          <DropdownMenu.Label className="theme-menu-label">Density</DropdownMenu.Label>
          <DropdownMenu.RadioGroup value={density} onValueChange={(value) => onDensityChange(value as PamDensity)}>
            <DropdownMenu.RadioItem className="theme-menu-item theme-menu-item--compact" value="comfortable" textValue="Comfortable density">
              <SquaresFour size={19} aria-hidden="true" />
              <span><strong>Comfortable</strong><small>Roomier spacing</small></span>
              <DropdownMenu.ItemIndicator><Check size={15} weight="bold" aria-hidden="true" /></DropdownMenu.ItemIndicator>
            </DropdownMenu.RadioItem>
            <DropdownMenu.RadioItem className="theme-menu-item theme-menu-item--compact" value="compact" textValue="Compact density">
              <Queue size={19} aria-hidden="true" />
              <span><strong>Compact</strong><small>More rows on screen</small></span>
              <DropdownMenu.ItemIndicator><Check size={15} weight="bold" aria-hidden="true" /></DropdownMenu.ItemIndicator>
            </DropdownMenu.RadioItem>
          </DropdownMenu.RadioGroup>
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}

export function Toolbar({
  projectName,
  queueCount,
  fixture,
  collapsed,
  pending,
  theme,
  themeMode,
  density,
  onThemeChange,
  onThemeModeChange,
  onDensityChange,
  onToggleSidebar,
  onRefresh,
  onOpenCommand,
  onOpenQueue,
  toggleButtonRef,
  commandButtonRef,
  queueButtonRef,
}: {
  projectName: string | null;
  queueCount: number | null;
  fixture: boolean;
  collapsed: boolean;
  pending: boolean;
  theme: PamTheme;
  themeMode: PamThemeMode;
  density: PamDensity;
  onThemeChange: (theme: PamTheme) => void;
  onThemeModeChange: (mode: PamThemeMode) => void;
  onDensityChange: (density: PamDensity) => void;
  onToggleSidebar: () => void;
  onRefresh: () => void;
  onOpenCommand: (returnFocusTarget?: HTMLElement) => void;
  onOpenQueue: (returnFocusTarget?: HTMLElement) => void;
  toggleButtonRef: RefObject<HTMLButtonElement | null>;
  commandButtonRef: RefObject<HTMLButtonElement | null>;
  queueButtonRef: RefObject<HTMLButtonElement | null>;
}) {
  return (
    <header className="toolbar" data-tauri-drag-region="deep">
      <button ref={toggleButtonRef} type="button" aria-label={collapsed ? "Expand sidebar" : "Collapse sidebar"} onClick={onToggleSidebar}>
        <SidebarSimple size={19} weight="bold" />
      </button>
      <div className="breadcrumb">
        {projectName !== null && (
          <>
            <span>{projectName}</span>
            <CaretRight size={12} aria-hidden="true" />
          </>
        )}
        <strong>Daemon observatory</strong>
      </div>
      {import.meta.env.DEV && fixture && <span className="fixture-badge">Design fixture</span>}
      <div className="toolbar-actions">
        <IconTooltip label="Search commands · ⌘K">
          <button ref={commandButtonRef} type="button" aria-label="Open command palette (⌘K)" onClick={(event) => onOpenCommand(event.currentTarget)}>
            <MagnifyingGlass size={18} />
          </button>
        </IconTooltip>
        {queueCount !== null && (
          <IconTooltip label="Open project queue">
            <button ref={queueButtonRef} type="button" aria-label="Open queue" onClick={(event) => onOpenQueue(event.currentTarget)}>
              <Queue size={19} />
              {queueCount > 0 && <span>{queueCount}</span>}
            </button>
          </IconTooltip>
        )}
        <IconTooltip label={projectName !== null ? "Refresh project · ⌘R" : "Refresh daemon · ⌘R"}>
          <button type="button" aria-label={projectName !== null ? "Refresh project" : "Refresh daemon"} disabled={pending} onClick={onRefresh}>
            <ArrowClockwise className={pending ? "is-spinning" : ""} size={18} weight="bold" />
          </button>
        </IconTooltip>
        <ThemeMenu
          theme={theme}
          themeMode={themeMode}
          density={density}
          onThemeChange={onThemeChange}
          onThemeModeChange={onThemeModeChange}
          onDensityChange={onDensityChange}
        />
      </div>
    </header>
  );
}
