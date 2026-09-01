import { Brain, CalendarBlank, Lightning, Pulse, Queue } from "@phosphor-icons/react";
import { useCallback, useEffect, useRef, useState, type CSSProperties, type ReactNode } from "react";
import { withDaemonOperation } from "../bridge";
import { PanelEmpty, PanelError } from "../components/PanelState";
import type {
  ActivityDayDto,
  DaemonStatsDto,
  ModelStatusDto,
  PamBridge,
  ProjectSummaryDto,
  ProjectUsageDto,
} from "../domain";
import { basename, type DaemonView } from "../selectors";
import { presentError } from "../state";
import { formatModelSize } from "./ActivityView";

const DAY_MS = 86_400_000;
/**
 * The one knob for the activity window: widen it and the requested days, the
 * grid, the month axis and the copy all follow.
 *
 * Ceiling: the daemon clamps `daemon.stats` at MAX_STATS_DAYS (366) and the
 * store retains MAX_ACTIVITY_DAYS (400), so going past 52 weeks here means
 * raising those first — otherwise the served window is shorter than the grid
 * and the extra columns render permanently empty.
 */
export const HEATMAP_WEEKS = 52;
/** The window the daemon is asked for, so the grid is never wider than its data. */
export const HEATMAP_DAYS = HEATMAP_WEEKS * 7;

export interface HeatmapCell {
  dayStartMs: number;
  events: number;
  /** 0 (none) through 4 (busiest quartile). */
  intensity: number;
  inWindow: boolean;
}

// Lays the trailing window out as GitHub-style columns of seven UTC days,
// newest column last, aligned so every column starts on a Sunday.
export function buildHeatmapWeeks(days: ActivityDayDto[], todayStartMs: number): HeatmapCell[][] {
  const counts = new Map(days.map((day) => [day.dayStartMs, day.events]));
  const busiest = Math.max(1, ...days.map((day) => day.events));
  // Epoch day zero (1970-01-01) was a Thursday: weekday index 4.
  const weekday = (dayStartMs: number) => (Math.floor(dayStartMs / DAY_MS) + 4) % 7;
  const lastColumnStart = todayStartMs - weekday(todayStartMs) * DAY_MS;
  const firstColumnStart = lastColumnStart - (HEATMAP_WEEKS - 1) * 7 * DAY_MS;
  const windowStart = todayStartMs - (HEATMAP_WEEKS * 7 - 1) * DAY_MS;
  const weeks: HeatmapCell[][] = [];
  for (let week = 0; week < HEATMAP_WEEKS; week += 1) {
    const column: HeatmapCell[] = [];
    for (let day = 0; day < 7; day += 1) {
      const dayStartMs = firstColumnStart + (week * 7 + day) * DAY_MS;
      const events = counts.get(dayStartMs) ?? 0;
      const inWindow = dayStartMs >= windowStart && dayStartMs <= todayStartMs;
      column.push({
        dayStartMs,
        events,
        inWindow,
        intensity:
          !inWindow || events === 0 ? 0 : Math.min(4, Math.ceil((events / busiest) * 4)),
      });
    }
    weeks.push(column);
  }
  return weeks;
}

export interface HeatmapMonth {
  label: string;
  /** 1-based grid column of the week this month starts on. */
  column: number;
  span: number;
}

const MONTH_LABELS = [
  "Jan", "Feb", "Mar", "Apr", "May", "Jun",
  "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/** The weekday rows that carry a label, matching the calendar convention. */
export const HEATMAP_WEEKDAYS: ReadonlyArray<{ row: number; label: string }> = [
  { row: 2, label: "Mon" },
  { row: 4, label: "Wed" },
  { row: 6, label: "Fri" },
];

// One label per month, placed on the column where that month first appears.
// A run too narrow to hold its label is dropped rather than overlapped.
export function buildHeatmapMonths(weeks: HeatmapCell[][]): HeatmapMonth[] {
  const months: HeatmapMonth[] = [];
  weeks.forEach((week, index) => {
    const month = new Date(week[0].dayStartMs).getUTCMonth();
    const previous = months.at(-1);
    if (previous && previous.label === MONTH_LABELS[month]) {
      previous.span += 1;
      return;
    }
    months.push({ label: MONTH_LABELS[month], column: index + 1, span: 1 });
  });
  return months.filter((month) => month.span >= 3);
}

export interface ActivityStreaks {
  totalEvents: number;
  activeDays: number;
  currentStreak: number;
  longestStreak: number;
}

// Streaks over the served window; the current streak tolerates a quiet
// today so an active week does not reset at midnight.
export function computeStreaks(days: ActivityDayDto[], todayStartMs: number): ActivityStreaks {
  const active = new Set(
    days.filter((day) => day.events > 0).map((day) => Math.floor(day.dayStartMs / DAY_MS)),
  );
  const today = Math.floor(todayStartMs / DAY_MS);
  let longest = 0;
  let run = 0;
  for (const day of [...active].sort((left, right) => left - right)) {
    run = active.has(day - 1) ? run + 1 : 1;
    longest = Math.max(longest, run);
  }
  let current = 0;
  let cursor = active.has(today) ? today : today - 1;
  while (active.has(cursor)) {
    current += 1;
    cursor -= 1;
  }
  return {
    totalEvents: days.reduce((sum, day) => sum + day.events, 0),
    activeDays: active.size,
    currentStreak: current,
    longestStreak: longest,
  };
}

function formatDay(dayStartMs: number): string {
  const date = new Date(dayStartMs);
  return Number.isNaN(date.valueOf())
    ? "Unknown day"
    : new Intl.DateTimeFormat(undefined, {
        month: "short",
        day: "numeric",
        timeZone: "UTC",
      }).format(date);
}

function StatTile({
  icon,
  label,
  value,
  hint,
}: {
  icon: ReactNode;
  label: string;
  value: string;
  hint?: string;
}) {
  return (
    <article className="project-stat group flex items-center gap-3">
      <span className="project-stat-icon">{icon}</span>
      <div>
        <small>{label}</small>
        <strong title={value}>{value}</strong>
        {hint && <small>{hint}</small>}
      </div>
    </article>
  );
}

// Daemon-wide activity stats: one fetch shared by every panel that needs
// them (overview heatmap, project usage), each rendering its own slice.
export interface DaemonStatsState {
  stats: DaemonStatsDto | null;
  loadError: string | null;
}

export function useDaemonStats(bridge: PamBridge, daemon: DaemonView, refreshTick = 0): DaemonStatsState {
  const [stats, setStats] = useState<DaemonStatsDto | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const requestSequence = useRef(0);
  const offline = daemon.state === "stopped";

  const load = useCallback(async () => {
    const sequence = ++requestSequence.current;
    setLoadError(null);
    try {
      // Activity statistics are daemon-global: always the daemon authority.
      const response = await bridge.daemonStats(withDaemonOperation(), HEATMAP_DAYS);
      if (sequence !== requestSequence.current) return;
      setStats(response);
    } catch (error) {
      if (sequence === requestSequence.current) setLoadError(presentError(error));
    }
  }, [bridge]);

  useEffect(() => {
    if (offline) {
      setStats(null);
      setLoadError(null);
      return;
    }
    void load();
    return () => {
      requestSequence.current += 1;
    };
  }, [load, offline, refreshTick]);

  return { stats, loadError };
}

// Read-only model presence on the Overview: identity plus one pill, and a
// way through to Models. Everything actionable about the model — the load
// meter, the catalog, verify/chat, downloads, imports — lives on Models.
function ModelStatusTile({
  daemon,
  modelStatus,
  onOpenModels,
}: {
  daemon: DaemonView;
  modelStatus: ModelStatusDto | null;
  onOpenModels: () => void;
}) {
  const loading = modelStatus?.status === "ok" && modelStatus.loading;
  const offline = daemon.state === "stopped" && !loading;
  const loaded = modelStatus?.status === "ok" ? modelStatus.loaded : null;
  const registered = modelStatus?.status === "ok" ? modelStatus.registered : [];
  const onDeck = !loaded && registered.length > 0 ? registered[0] : null;
  const pill = loading
    ? { label: "LOADING", tone: "elevated" }
    : offline || modelStatus?.status !== "ok"
      ? { label: "UNREACHABLE", tone: "attention" }
      : loaded
        ? { label: "LOADED", tone: "healthy" }
        : onDeck
          ? { label: "ON DECK", tone: "observed" }
          : { label: "NONE", tone: "not-reported" };
  const identity = loaded?.modelId ?? onDeck?.modelId ?? "No local model yet";
  // The tile is one cell of the stat strip, so the identity only reads if it
  // gets the cell's full width: the pill shares the label's line instead of
  // competing with it, and the vendor prefix — the same for every model in a
  // catalogue — is dropped in favour of the name. The full id stays the
  // tooltip.
  const name = identity.includes("/") ? identity.slice(identity.indexOf("/") + 1) : identity;

  return (
    <button type="button" className="project-stat model-stat group flex items-center gap-3" onClick={onOpenModels}>
      <span className="project-stat-icon"><Brain size={21} weight="bold" /></span>
      <div>
        <small>
          Local model
          <span className={`state-pill state-pill--${pill.tone}`}>{pill.label}</span>
        </small>
        <strong title={identity}>{name}</strong>
      </div>
    </button>
  );
}

export interface OverviewPanelProps {
  daemon: DaemonView;
  stats: DaemonStatsDto | null;
  loadError: string | null;
  modelStatus: ModelStatusDto | null;
  onOpenModels: () => void;
}

export function OverviewPanel({ daemon, stats, loadError, modelStatus, onOpenModels }: OverviewPanelProps) {
  const offline = daemon.state === "stopped";
  const todayStartMs = Math.floor(Date.now() / DAY_MS) * DAY_MS;
  const days = stats?.status === "ok" ? stats.days : [];
  const streaks = computeStreaks(days, todayStartMs);
  const weeks = buildHeatmapWeeks(days, todayStartMs);
  const months = buildHeatmapMonths(weeks);

  return (
    <>
      <section className="project-overview" aria-label="Daemon overview">
        <article className="project-stat group flex items-center gap-3">
          <span
            className={`project-stat-icon${daemon.state === "running" ? "" : " project-stat-icon--attention"}`}
          >
            <Pulse size={21} weight="bold" />
          </span>
          <div>
            <small>Watch status</small>
            <strong title={daemon.detail}>{daemon.detail}</strong>
          </div>
        </article>
        <StatTile
          icon={<Queue size={21} weight="bold" />}
          label="Queue depth"
          value={daemon.queueDepth === null ? "—" : String(daemon.queueDepth)}
        />
        <StatTile
          icon={<Lightning size={21} weight="bold" />}
          label="Events"
          value={streaks.totalEvents.toLocaleString()}
          hint={`last ${HEATMAP_WEEKS} weeks`}
        />
        <StatTile
          icon={<CalendarBlank size={21} weight="bold" />}
          label="Active days"
          value={String(streaks.activeDays)}
        />
        <StatTile
          icon={<Lightning size={21} weight="bold" />}
          label="Streak"
          value={`${streaks.currentStreak}d`}
          hint={`longest ${streaks.longestStreak}d`}
        />
        <ModelStatusTile daemon={daemon} modelStatus={modelStatus} onOpenModels={onOpenModels} />
      </section>
      <section className="panel heatmap-panel" aria-labelledby="overview-heatmap-heading">
        <div className="panel-title">
          <div>
            <span className="eyebrow">Daemon activity</span>
            <h2 id="overview-heatmap-heading">{`The last ${HEATMAP_WEEKS} weeks`}</h2>
          </div>
          <div className="heatmap-legend" aria-hidden="true">
            <small>Less</small>
            {[0, 1, 2, 3, 4].map((level) => (
              <span className={`heatmap-cell heat-${level}`} key={level} />
            ))}
            <small>More</small>
          </div>
        </div>
        {loadError ? (
          <PanelError>
            {loadError}
          </PanelError>
        ) : offline ? (
          <PanelEmpty>The activity picture returns when Pam is back on watch.</PanelEmpty>
        ) : stats && stats.status !== "ok" ? (
          <PanelEmpty>
            {[stats.failure.detail, stats.failure.recovery].filter(Boolean).join(" ")}
          </PanelEmpty>
        ) : (
          <div className="heatmap-scroll">
            <div
              className="heatmap-figure"
              style={{ "--heatmap-weeks": HEATMAP_WEEKS } as CSSProperties}
            >
              <div className="heatmap-months" aria-hidden="true">
                {months.map((month) => (
                  <span
                    key={`${month.label}-${month.column}`}
                    style={{ gridColumn: `${month.column} / span ${month.span}` }}
                  >
                    {month.label}
                  </span>
                ))}
              </div>
              <div className="heatmap-weekdays" aria-hidden="true">
                {HEATMAP_WEEKDAYS.map((weekday) => (
                  <span key={weekday.label} style={{ gridRow: weekday.row }}>
                    {weekday.label}
                  </span>
                ))}
              </div>
              <div
                className="heatmap"
                role="img"
                aria-label={`Daily daemon activity for the last ${HEATMAP_WEEKS} weeks: ${streaks.totalEvents.toLocaleString()} events across ${streaks.activeDays} active days`}
              >
                {weeks.map((week) => (
                  <div className="heatmap-week" key={week[0].dayStartMs}>
                    {week.map((cell) => (
                      <span
                        key={cell.dayStartMs}
                        className={`heatmap-cell heat-${cell.intensity}${cell.inWindow ? "" : " heatmap-cell--outside"}`}
                        title={`${formatDay(cell.dayStartMs)} · ${cell.events} event${cell.events === 1 ? "" : "s"}`}
                      />
                    ))}
                  </div>
                ))}
              </div>
            </div>
          </div>
        )}
      </section>
    </>
  );
}


function formatLastActive(lastEventMs: number | null): string {
  if (lastEventMs === null) return "No activity yet";
  const date = new Date(lastEventMs);
  return Number.isNaN(date.valueOf())
    ? "Date unavailable"
    : new Intl.DateTimeFormat(undefined, { dateStyle: "medium" }).format(date);
}

export interface ProjectUsageRow {
  projectId: string;
  name: string;
  location: string | null;
  events: number;
  lastEventMs: number | null;
}

// The whole known fleet, not just the active project: every catalog project
// appears (zero-usage ones included), plus any usage row Pam reports for a
// project the catalog does not (yet) know about.
export function aggregateProjectUsage(
  catalog: ProjectSummaryDto[],
  usage: ProjectUsageDto[],
): ProjectUsageRow[] {
  const rows = new Map<string, ProjectUsageRow>();
  for (const project of catalog) {
    rows.set(project.handle, {
      projectId: project.handle,
      name: project.name,
      location: project.location,
      events: 0,
      lastEventMs: null,
    });
  }
  for (const row of usage) {
    const known = catalog.find((project) => project.handle === row.projectId);
    rows.set(row.projectId, {
      projectId: row.projectId,
      name: known ? known.name : row.root ? basename(row.root) : `${row.projectId.slice(0, 8)}…`,
      location: known ? known.location : row.root,
      events: row.events,
      lastEventMs: row.lastEventMs,
    });
  }
  return [...rows.values()].sort(
    (left, right) => right.events - left.events || left.name.localeCompare(right.name),
  );
}

interface ProjectsPanelProps {
  daemon: DaemonView;
  catalog: ProjectSummaryDto[];
  stats: DaemonStatsDto | null;
  loadError: string | null;
}

// The fleet overview: every project Pam knows about, at a glance, with no
// selector — this screen never scopes to one project.
function ProjectsPanel({ daemon, catalog, stats, loadError }: ProjectsPanelProps) {
  const offline = daemon.state === "stopped";

  // Defensive: an older daemon may not report `projects` yet.
  const usage = stats?.status === "ok" ? stats.projects ?? [] : [];
  const rows = aggregateProjectUsage(catalog, usage);
  const maxEvents = Math.max(1, ...rows.map((row) => row.events));

  return (
    <section className="panel projects-panel" aria-labelledby="projects-panel-heading">
      <div className="panel-title">
        <div>
          <span className="eyebrow">Projects</span>
          <h2 id="projects-panel-heading">Usage by project</h2>
        </div>
      </div>
      {loadError ? (
        <PanelError>
          {loadError}
        </PanelError>
      ) : offline ? (
        <PanelEmpty>Project usage returns when Pam is back on watch.</PanelEmpty>
      ) : stats && stats.status !== "ok" ? (
        <PanelEmpty>
          {[stats.failure.detail, stats.failure.recovery].filter(Boolean).join(" ")}
        </PanelEmpty>
      ) : rows.length === 0 ? (
        <PanelEmpty>No projects are known to Pam yet.</PanelEmpty>
      ) : (
        <div className="project-usage-list">
          {rows.map((row) => (
            <article className="project-usage-row" key={row.projectId}>
              <div className="project-usage-identity">
                <strong title={row.name}>{row.name}</strong>
                {row.location && <small title={row.location}>{row.location}</small>}
              </div>
              <div className="project-usage-bar-track" aria-hidden="true">
                <div
                  className="project-usage-bar-fill"
                  style={{ width: `${Math.round((row.events / maxEvents) * 100)}%` }}
                />
              </div>
              <div className="project-usage-meta">
                <strong>
                  {row.events.toLocaleString()} event{row.events === 1 ? "" : "s"}
                </strong>
                <small>{formatLastActive(row.lastEventMs)}</small>
              </div>
            </article>
          ))}
        </div>
      )}
    </section>
  );
}

export interface OverviewViewProps {
  bridge: PamBridge;
  daemon: DaemonView;
  catalog?: ProjectSummaryDto[];
  modelStatus: ModelStatusDto | null;
  /** Opens the Models view; the tile below is the only model surface here. */
  onOpenModels: () => void;
  /** Bumped by ⌘R; re-runs the mount-time loaders without remounting. */
  refreshTick?: number;
}

export function OverviewView({
  bridge,
  daemon,
  catalog = [],
  modelStatus,
  onOpenModels,
  refreshTick = 0,
}: OverviewViewProps) {
  const { stats, loadError } = useDaemonStats(bridge, daemon, refreshTick);
  return (
    <main className="canvas" id="main-content">
      <header className="project-header compact">
        <div>
          <h1>Overview</h1>
          <p>Everything Pam watches, at a glance.</p>
        </div>
      </header>
      <OverviewPanel
        daemon={daemon}
        stats={stats}
        loadError={loadError}
        modelStatus={modelStatus}
        onOpenModels={onOpenModels}
      />
      <ProjectsPanel daemon={daemon} catalog={catalog} stats={stats} loadError={loadError} />
    </main>
  );
}
