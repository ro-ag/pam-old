import { Brain, CalendarBlank, Check, Copy, Lightning, Power, Pulse, Queue } from "@phosphor-icons/react";
import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { withDaemonOperation } from "../bridge";
import { ProjectPicker } from "../components/Shell";
import type { ActivityDayDto, DaemonStatsDto, ModelStatusDto, PamBridge, ProjectSummaryDto } from "../domain";
import type { DaemonView } from "../selectors";
import { presentError } from "../state";
import { formatModelSize } from "./ActivityView";
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

type VerifyState =
  | { state: "idle" }
  | { state: "running" }
  | { state: "pass"; ms: number; tokens: number }
  | { state: "fail"; detail: string };

const MODEL_IMPORT_COMMAND =
  "pam model import <vendor>/<name> --path /absolute/path/model.gguf "
  + "--digest sha256:<hex> --size-bytes <bytes> --license-id <spdx-id> "
  + "--license-url <url> --license-notice-digest sha256:<hex> --accept-license";

export interface ModelPanelProps {
  bridge: PamBridge;
  daemon: DaemonView;
  modelStatus: ModelStatusDto | null;
  /** True while a daemon lifecycle command is in flight. */
  modelBusy: boolean;
  onOpenModelChat: (modelId: string, returnFocusTarget?: HTMLElement) => void;
  onStartWithModel: (modelId: string) => void;
}

// The local model runtime, one panel on the launch view: state, identity,
// a live verification round-trip, and the path from "no model" to "loaded".
export function ModelPanel({
  bridge,
  daemon,
  modelStatus,
  modelBusy,
  onOpenModelChat,
  onStartWithModel,
}: ModelPanelProps) {
  const [verify, setVerify] = useState<VerifyState>({ state: "idle" });
  const [copied, setCopied] = useState(false);
  const offline = daemon.state === "stopped";
  const loaded = modelStatus?.status === "ok" ? modelStatus.loaded : null;
  const registered = modelStatus?.status === "ok" ? modelStatus.registered : [];
  const restartLabel = offline ? "Start PAM with this model" : "Restart PAM with this model";

  const runVerify = async (modelId: string) => {
    setVerify({ state: "running" });
    const startedAt = performance.now();
    try {
      const response = await bridge.modelInfer(
        withDaemonOperation(),
        modelId,
        [{ role: "user", content: "Reply with a single word: ready." }],
        16,
      );
      const ms = Math.round(performance.now() - startedAt);
      if (response.status === "ok") {
        setVerify({ state: "pass", ms, tokens: response.usage.emittedOutputTokens });
      } else {
        setVerify({
          state: "fail",
          detail: [response.failure.detail, response.failure.recovery].filter(Boolean).join(" "),
        });
      }
    } catch (error) {
      setVerify({ state: "fail", detail: presentError(error) });
    }
  };

  const copyImportCommand = async () => {
    try {
      await navigator.clipboard.writeText(MODEL_IMPORT_COMMAND);
      setCopied(true);
    } catch {
      // Clipboard access is optional; the command stays selectable on screen.
    }
  };

  const pill = offline || !modelStatus || modelStatus.status !== "ok"
    ? { label: offline ? "unreachable" : !modelStatus ? "checking" : "unreachable", tone: offline || modelStatus ? "attention" : "not-reported" }
    : loaded
      ? { label: "loaded", tone: "healthy" }
      : registered.length > 0
        ? { label: "on deck", tone: "observed" }
        : { label: "none", tone: "not-reported" };

  const restartRow = (model: { modelId: string; sizeBytes: number }) => (
    <article key={model.modelId}>
      <span className="access-icon"><Brain size={21} /></span>
      <div>
        <strong title={model.modelId}>{model.modelId}</strong>
        <p>{formatModelSize(model.sizeBytes)} on disk</p>
      </div>
      <button
        type="button"
        className="button button--secondary button--small"
        disabled={modelBusy}
        onClick={() => onStartWithModel(model.modelId)}
      >
        <Power size={17} /> {restartLabel}
      </button>
    </article>
  );

  return (
    <section className="panel model-panel" aria-labelledby="model-panel-heading">
      <div className="panel-title">
        <div>
          <span className="eyebrow">Local model</span>
          <h2 id="model-panel-heading">Model runtime</h2>
        </div>
        <span className={`state-pill state-pill--${pill.tone}`}>{pill.label}</span>
      </div>
      {offline ? (
        <p className="panel-empty">PAM is paused, so the local model runtime is not reachable. Start PAM to check on it.</p>
      ) : !modelStatus ? (
        <p className="panel-empty">Checking the local model…</p>
      ) : modelStatus.status !== "ok" ? (
        <p className="panel-empty">
          {[modelStatus.failure.detail, modelStatus.failure.recovery].filter(Boolean).join(" ")}
        </p>
      ) : loaded ? (
        <div className="model-runtime">
          <div className="model-identity">
            <strong title={loaded.modelId}>{loaded.modelId}</strong>
            <small>{formatModelSize(loaded.sizeBytes)} on disk · loads fully into memory</small>
          </div>
          <div className="model-actions">
            <button
              type="button"
              className="button button--secondary button--small"
              disabled={verify.state === "running"}
              onClick={() => void runVerify(loaded.modelId)}
            >
              {verify.state === "running" ? "Verifying…" : "Verify"}
            </button>
            <button
              type="button"
              className="button button--primary button--small"
              onClick={(event) => onOpenModelChat(loaded.modelId, event.currentTarget)}
            >
              Chat
            </button>
          </div>
          {verify.state === "pass" && (
            <p className="model-verify is-pass" role="status">
              <Check size={16} aria-hidden="true" /> Verified · {verify.ms} ms · {verify.tokens} token{verify.tokens === 1 ? "" : "s"} back
            </p>
          )}
          {verify.state === "fail" && (
            <p className="model-verify is-fail" role="alert">{verify.detail}</p>
          )}
          {registered.some((model) => model.modelId !== loaded.modelId) && (
            <div className="access-list model-rows">
              {registered.filter((model) => model.modelId !== loaded.modelId).map(restartRow)}
            </div>
          )}
        </div>
      ) : registered.length > 0 ? (
        <div className="model-runtime">
          <p className="model-note">A model is registered but not loaded. Bring it into memory to chat and verify.</p>
          <div className="access-list model-rows">{registered.map(restartRow)}</div>
        </div>
      ) : (
        <div className="model-runtime model-guide">
          <p className="model-note">No local model is registered yet. Three steps bring one on watch:</p>
          <ol>
            <li>Download a GGUF build of the model you want to run locally.</li>
            <li>
              Register it with the daemon:
              <pre><code>{MODEL_IMPORT_COMMAND}</code></pre>
              <button type="button" className="button button--secondary button--small" onClick={() => void copyImportCommand()}>
                {copied ? <Check size={17} /> : <Copy size={17} />} {copied ? "Copied" : "Copy command"}
              </button>
            </li>
            <li>Return here and start PAM with the registered model.</li>
          </ol>
        </div>
      )}
    </section>
  );
}

export interface ControlCenterViewProps {
  bridge: PamBridge;
  daemon: DaemonView;
  projects: ProjectSummaryDto[];
  onSelectProject: (project: ProjectSummaryDto) => void;
  contextBar?: ReactNode;
  modelStatus: ModelStatusDto | null;
  modelBusy: boolean;
  onOpenModelChat: (modelId: string, returnFocusTarget?: HTMLElement) => void;
  onStartWithModel: (modelId: string) => void;
  /** Present while a project is active; the project keeps a calm, second row. */
  project: CurrentViewProps | null;
}

export function ControlCenterView({
  bridge,
  daemon,
  projects,
  onSelectProject,
  contextBar,
  modelStatus,
  modelBusy,
  onOpenModelChat,
  onStartWithModel,
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
      <ModelPanel
        bridge={bridge}
        daemon={daemon}
        modelStatus={modelStatus}
        modelBusy={modelBusy}
        onOpenModelChat={onOpenModelChat}
        onStartWithModel={onStartWithModel}
      />
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
