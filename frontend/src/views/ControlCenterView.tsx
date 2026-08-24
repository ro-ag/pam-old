import { CalendarBlank, Lightning, Pulse, Queue } from "@phosphor-icons/react";
import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { withDaemonOperation } from "../bridge";
import { ProjectPicker } from "../components/Shell";
import type { ActivityDayDto, DaemonStatsDto, PamBridge, ProjectSummaryDto } from "../domain";
import type { DaemonView } from "../selectors";
import { presentError } from "../state";
import { CurrentView, type CurrentViewProps } from "./ProjectViews";

const DAY_MS = 86_400_000;
export const HEATMAP_WEEKS = 26;

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

export interface OverviewPanelProps {
  bridge: PamBridge;
  daemon: DaemonView;
}

export function OverviewPanel({ bridge, daemon }: OverviewPanelProps) {
  const [stats, setStats] = useState<DaemonStatsDto | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const requestSequence = useRef(0);
  const offline = daemon.state === "stopped";

  const load = useCallback(async () => {
    const sequence = ++requestSequence.current;
    setLoadError(null);
    try {
      // Activity statistics are daemon-global: always the daemon authority.
      const response = await bridge.daemonStats(withDaemonOperation());
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
  }, [load, offline]);

  const todayStartMs = Math.floor(Date.now() / DAY_MS) * DAY_MS;
  const days = stats?.status === "ok" ? stats.days : [];
  const streaks = computeStreaks(days, todayStartMs);
  const weeks = buildHeatmapWeeks(days, todayStartMs);

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
          hint="last 26 weeks"
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
      </section>
      <section className="panel heatmap-panel" aria-labelledby="overview-heatmap-heading">
        <div className="panel-title">
          <div>
            <span className="eyebrow">Daemon activity</span>
            <h2 id="overview-heatmap-heading">The last 26 weeks</h2>
          </div>
        </div>
        {loadError ? (
          <p className="panel-empty" role="alert">
            {loadError}
          </p>
        ) : offline ? (
          <p className="panel-empty">The activity picture returns when PAM is back on watch.</p>
        ) : stats && stats.status !== "ok" ? (
          <p className="panel-empty">
            {[stats.failure.detail, stats.failure.recovery].filter(Boolean).join(" ")}
          </p>
        ) : (
          <div className="heatmap-scroll">
            <div className="heatmap" role="img" aria-label="Daily daemon activity for the last 26 weeks">
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
            <div className="heatmap-legend" aria-hidden="true">
              <small>Less</small>
              {[0, 1, 2, 3, 4].map((level) => (
                <span className={`heatmap-cell heat-${level}`} key={level} />
              ))}
              <small>More</small>
            </div>
          </div>
        )}
      </section>
    </>
  );
}

export interface ControlCenterViewProps {
  bridge: PamBridge;
  daemon: DaemonView;
  projects: ProjectSummaryDto[];
  onSelectProject: (project: ProjectSummaryDto) => void;
  contextBar?: ReactNode;
  /** Present while a project is active; the project keeps a calm, second row. */
  project: CurrentViewProps | null;
}

export function ControlCenterView({
  bridge,
  daemon,
  projects,
  onSelectProject,
  contextBar,
  project,
}: ControlCenterViewProps) {
  return (
    <main className="canvas" id="main-content">
      <header className="project-header compact">
        <div>
          <h1>Control center</h1>
          <p>Everything PAM watches, at a glance.</p>
        </div>
        {contextBar}
      </header>
      <OverviewPanel bridge={bridge} daemon={daemon} />
      {project ? (
        <section className="project-detail" aria-label="Active project">
          <CurrentView {...project} />
        </section>
      ) : (
        <section className="panel project-detail" aria-label="Projects">
          <div className="panel-title">
            <div>
              <span className="eyebrow">Projects</span>
              <h2>Bring a queue into view</h2>
            </div>
          </div>
          {projects.length === 0 ? (
            <p className="panel-empty">
              Open PAM from a Git repository and it will settle in here on its own. The daemon keeps
              watch either way.
            </p>
          ) : (
            <ProjectPicker projects={projects} onSelect={onSelectProject} />
          )}
        </section>
      )}
    </main>
  );
}
