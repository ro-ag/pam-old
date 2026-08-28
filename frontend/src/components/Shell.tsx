import {
  ArrowClockwise,
  BookOpen,
  Brain,
  Check,
  Circle,
  Gear,
  GitBranch,
  LockSimple,
  MagnifyingGlass,
  MoonStars,
  Power,
  Pulse,
  Queue,
  SidebarSimple,
  SquaresFour,
  SunHorizon,
} from "@phosphor-icons/react";
import {
  Button,
  Focusable,
  Header,
  Menu,
  MenuItem,
  MenuSection,
  MenuTrigger,
  OverlayArrow,
  Popover,
  Separator,
  Tooltip,
  TooltipTrigger,
} from "react-aria-components";
import {
  Fragment,
  type ComponentProps,
  type KeyboardEvent as ReactKeyboardEvent,
  type PointerEvent as ReactPointerEvent,
  type RefObject,
  useEffect,
  useRef,
} from "react";
import type { ViewId } from "../domain";
import {
  clampSidebarWidth,
  minimumSidebarWidth,
  sidebarMaximumWidth,
  sidebarWidthFromKey,
} from "../layout";
import type { DaemonView } from "../selectors";
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
  { id: "overview", label: "Overview", icon: SquaresFour },
  { id: "models", label: "Models", icon: Brain },
  { id: "flows", label: "Flows", icon: GitBranch },
  { id: "skills", label: "Skills", icon: BookOpen },
  { id: "access", label: "Access", icon: LockSimple },
  { id: "activity", label: "Activity", icon: Pulse },
];

export function StatusDot({ state = "coral" }: { state?: "coral" | "aqua" | "muted" }) {
  return <Circle className={`status-dot status-dot--${state}`} size={12} weight="fill" aria-hidden="true" />;
}

type FocusableChild = ComponentProps<typeof Focusable>["children"];

function IconTooltip({ label, children }: { label: string; children: FocusableChild }) {
  return (
    <TooltipTrigger delay={350} closeDelay={150}>
      <Focusable>{children}</Focusable>
      <Tooltip className="tooltip-content" offset={8}>
        {label}
        <OverlayArrow className="tooltip-arrow">
          <svg width={10} height={5} viewBox="0 0 30 10" preserveAspectRatio="none" aria-hidden="true">
            <polygon points="0,0 30,0 15,10" />
          </svg>
        </OverlayArrow>
      </Tooltip>
    </TooltipTrigger>
  );
}

// The check that marks the chosen row; Radix rendered this only when selected
// and the menu grid still counts on that.
function MenuCheck() {
  return <span className="menu-item-check"><Check size={15} weight="bold" aria-hidden="true" /></span>;
}

// react-aria selection speaks in key sets; every menu here is a single-choice
// radio group that can never be empty.
function onlyKey<T extends string>(keys: Iterable<unknown>, apply: (value: T) => void) {
  for (const key of keys) {
    apply(String(key) as T);
    return;
  }
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
              {!collapsed && id === "overview" && queueCount > 0 && (
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
              aria-label="Restart PAM (unloads the loaded model)"
              title="Restart PAM (unloads the loaded model)"
              disabled={pending}
              onClick={onRestartDaemon}
            >
              <ArrowClockwise size={16} weight="bold" aria-hidden="true" />
            </button>
          )}
        </div>
        <button
          type="button"
          className={`nav-item ${activeView === "settings" ? "is-active" : ""}`}
          aria-current={activeView === "settings" ? "page" : undefined}
          aria-label="Settings"
          title={collapsed ? "Settings" : undefined}
          onClick={() => onNavigate("settings")}
        >
          <Gear size={19} weight={activeView === "settings" ? "bold" : "regular"} aria-hidden="true" />
          {!collapsed && <span>Settings</span>}
        </button>
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
    <MenuTrigger>
      <Button
        className="theme-trigger"
        aria-label={`Theme: ${theme === "ventisquero" ? "Ventisquero" : "Viña del Mar"} · ${themeMode}`}
      >
        {themeMode === "light"
          ? <SunHorizon size={19} weight="bold" aria-hidden="true" />
          : <MoonStars size={19} weight="bold" aria-hidden="true" />}
      </Button>
      {/* react-aria floors the computed offset and the toolbar's 51px content
          box leaves the trigger on a half pixel, so 9 lands on the same 8px
          gap Radix rendered. */}
      <Popover className="theme-menu-popover" placement="bottom end" offset={9}>
        <Menu className="menu-list" aria-label="Appearance">
          <MenuSection
            selectionMode="single"
            disallowEmptySelection
            selectedKeys={[theme]}
            onSelectionChange={(keys) => onlyKey<PamTheme>(keys, onThemeChange)}
          >
            <Header className="theme-menu-label">Theme</Header>
            <MenuItem id="ventisquero" className="theme-menu-item" textValue="Ventisquero">
              {({ isSelected }) => (<>
                <span className="theme-swatch theme-swatch--ventisquero" aria-hidden="true" />
                <span><strong>Ventisquero</strong><small>Rock · Ice · Copper · Mist</small></span>
                {isSelected && <MenuCheck />}
              </>)}
            </MenuItem>
            <MenuItem id="vina" className="theme-menu-item" textValue="Viña del Mar">
              {({ isSelected }) => (<>
                <span className="theme-swatch theme-swatch--vina" aria-hidden="true" />
                <span><strong>Viña del Mar</strong><small>Night · Violet · Coral · Surf</small></span>
                {isSelected && <MenuCheck />}
              </>)}
            </MenuItem>
          </MenuSection>
          <Separator className="theme-menu-separator" />
          <MenuSection
            selectionMode="single"
            disallowEmptySelection
            selectedKeys={[themeMode]}
            onSelectionChange={(keys) => onlyKey<PamThemeMode>(keys, onThemeModeChange)}
          >
            <Header className="theme-menu-label">Variant</Header>
            <MenuItem id="light" className="theme-menu-item theme-menu-item--compact" textValue="Light variant">
              {({ isSelected }) => (<>
                <SunHorizon size={19} aria-hidden="true" />
                <span><strong>Light</strong><small>{theme === "ventisquero" ? "Mist" : "Dawn"}</small></span>
                {isSelected && <MenuCheck />}
              </>)}
            </MenuItem>
            <MenuItem id="dark" className="theme-menu-item theme-menu-item--compact" textValue="Dark variant">
              {({ isSelected }) => (<>
                <MoonStars size={19} aria-hidden="true" />
                <span><strong>Dark</strong><small>{theme === "ventisquero" ? "Bedrock" : "Night"}</small></span>
                {isSelected && <MenuCheck />}
              </>)}
            </MenuItem>
          </MenuSection>
          <Separator className="theme-menu-separator" />
          <MenuSection
            selectionMode="single"
            disallowEmptySelection
            selectedKeys={[density]}
            onSelectionChange={(keys) => onlyKey<PamDensity>(keys, onDensityChange)}
          >
            <Header className="theme-menu-label">Density</Header>
            <MenuItem id="comfortable" className="theme-menu-item theme-menu-item--compact" textValue="Comfortable density">
              {({ isSelected }) => (<>
                <SquaresFour size={19} aria-hidden="true" />
                <span><strong>Comfortable</strong><small>Roomier spacing</small></span>
                {isSelected && <MenuCheck />}
              </>)}
            </MenuItem>
            <MenuItem id="compact" className="theme-menu-item theme-menu-item--compact" textValue="Compact density">
              {({ isSelected }) => (<>
                <Queue size={19} aria-hidden="true" />
                <span><strong>Compact</strong><small>More rows on screen</small></span>
                {isSelected && <MenuCheck />}
              </>)}
            </MenuItem>
          </MenuSection>
        </Menu>
      </Popover>
    </MenuTrigger>
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
