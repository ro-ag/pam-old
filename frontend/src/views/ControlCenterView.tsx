import { Brain, CalendarBlank, Check, Lightning, Power, Pulse, Queue, UserCircle } from "@phosphor-icons/react";
import { useCallback, useEffect, useRef, useState, type FormEvent, type ReactNode } from "react";
import { withDaemonOperation } from "../bridge";
import type {
  ActivityEventDto,
  ActivityDayDto,
  CallerDto,
  DaemonStatsDto,
  ModelImportParams,
  ModelStatusDto,
  PamBridge,
} from "../domain";
import type { DaemonView } from "../selectors";
import { presentError } from "../state";
import { formatModelSize } from "./ActivityView";

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

type ImportState =
  | { state: "idle" }
  | { state: "running" }
  | { state: "fail"; detail: string };

// The whole import happens here: pick or drop the GGUF, accept its license,
// and PAM verifies, hashes, and registers it — no terminal round-trip.
function ModelImportForm({
  bridge,
  onImported,
}: {
  bridge: PamBridge;
  onImported: () => void;
}) {
  const [form, setForm] = useState({
    model: "",
    path: "",
    licenseId: "",
    licenseUrl: "",
    licenseNoticeText: "",
  });
  const [accepted, setAccepted] = useState(false);
  const [importState, setImportState] = useState<ImportState>({ state: "idle" });
  const busy = importState.state === "running";
  const set = (field: keyof typeof form) => (value: string) =>
    setForm((current) => ({ ...current, [field]: value }));

  // Dropping a GGUF from the file manager fills the path; native shell only.
  useEffect(() => {
    if (bridge.mode !== "native") return;
    let unlisten: (() => void) | null = null;
    let cancelled = false;
    void import("@tauri-apps/api/webview")
      .then(async ({ getCurrentWebview }) => {
        const stop = await getCurrentWebview().onDragDropEvent((event) => {
          if (event.payload.type !== "drop") return;
          const dropped = event.payload.paths.find((path) => path.toLowerCase().endsWith(".gguf"));
          if (dropped) setForm((current) => ({ ...current, path: dropped }));
        });
        if (cancelled) stop();
        else unlisten = stop;
      })
      .catch(() => {
        // Drag and drop is an enhancement; the path field always works.
      });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [bridge]);

  const ready =
    accepted && Object.values(form).every((value) => value.trim().length > 0);

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (!ready || busy) return;
    setImportState({ state: "running" });
    const params: ModelImportParams = {
      model: form.model.trim(),
      path: form.path.trim(),
      licenseId: form.licenseId.trim(),
      licenseUrl: form.licenseUrl.trim(),
      licenseNoticeText: form.licenseNoticeText,
    };
    try {
      const response = await bridge.modelImport(withDaemonOperation(), params);
      if (response.status === "ok") {
        setImportState({ state: "idle" });
        onImported();
      } else {
        setImportState({
          state: "fail",
          detail: [response.failure.detail, response.failure.recovery].filter(Boolean).join(" "),
        });
      }
    } catch (error) {
      setImportState({ state: "fail", detail: presentError(error) });
    }
  };

  const field = (
    label: string,
    key: keyof typeof form,
    placeholder: string,
    type: "text" | "url" = "text",
  ) => (
    <label>
      {label}
      <input
        type={type}
        name={`model-import-${key}`}
        placeholder={placeholder}
        value={form[key]}
        disabled={busy}
        onChange={(event) => set(key)(event.target.value)}
      />
    </label>
  );

  return (
    <form className="model-runtime model-import" onSubmit={(event) => void submit(event)}>
      <p className="model-note">
        No local model is registered yet. Drop a downloaded GGUF here, or point PAM at it — PAM
        verifies and registers it, then starts with it, all from this screen.
      </p>
      {field("GGUF file path", "path", "/absolute/path/to/model.gguf")}
      {field("Model identity", "model", "vendor/name, e.g. qwen/qwen3-4b-instruct-q4")}
      {field("License identifier", "licenseId", "SPDX id, e.g. Apache-2.0")}
      {field("License URL", "licenseUrl", "https://…", "url")}
      <label>
        License notice
        <textarea
          name="model-import-notice"
          placeholder="Paste the exact license notice text you are accepting."
          rows={3}
          value={form.licenseNoticeText}
          disabled={busy}
          onChange={(event) => set("licenseNoticeText")(event.target.value)}
        />
      </label>
      <label className="model-import-consent">
        <input
          type="checkbox"
          checked={accepted}
          disabled={busy}
          onChange={(event) => setAccepted(event.target.checked)}
        />
        I accept this model's license exactly as stated above.
      </label>
      <div className="model-actions">
        <button type="submit" className="button button--primary button--small" disabled={!ready || busy}>
          {busy ? "Verifying and registering…" : "Import model"}
        </button>
        {busy && <small>PAM reads and hashes the whole file; large models take a moment.</small>}
      </div>
      {importState.state === "fail" && (
        <p className="model-verify is-fail" role="alert">{importState.detail}</p>
      )}
    </form>
  );
}

export interface ModelPanelProps {
  bridge: PamBridge;
  daemon: DaemonView;
  modelStatus: ModelStatusDto | null;
  /** True while a daemon lifecycle command is in flight. */
  modelBusy: boolean;
  onOpenModelChat: (modelId: string, returnFocusTarget?: HTMLElement) => void;
  onStartWithModel: (modelId: string) => void;
  onModelImported: () => void;
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
  onModelImported,
}: ModelPanelProps) {
  const [verify, setVerify] = useState<VerifyState>({ state: "idle" });
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
        <ModelImportForm bridge={bridge} onImported={onModelImported} />
      )}
    </section>
  );
}

export interface CallerRequestRow {
  callerId: string;
  requests: number;
  revoked: boolean;
}

// The whole project story on this screen: recent daemon requests, grouped by
// caller. Registered-but-quiet callers stay visible with a zero count.
export function aggregateCallerRequests(
  callers: CallerDto[],
  events: ActivityEventDto[],
): CallerRequestRow[] {
  const rows = new Map<string, CallerRequestRow>();
  for (const caller of callers) {
    rows.set(caller.callerId, {
      callerId: caller.callerId,
      requests: 0,
      revoked: caller.revokedAtMs !== null,
    });
  }
  for (const event of events) {
    const row = rows.get(event.callerId) ?? { callerId: event.callerId, requests: 0, revoked: false };
    row.requests += 1;
    rows.set(event.callerId, row);
  }
  return [...rows.values()].sort(
    (left, right) => right.requests - left.requests || left.callerId.localeCompare(right.callerId),
  );
}

interface CallerRequestsPanelProps {
  bridge: PamBridge;
  daemon: DaemonView;
  /** True when the desktop's own GUI caller still needs registration. */
  registrationNeeded: boolean;
  registrationBusy: boolean;
  onRegisterCaller: () => void;
}

function CallerRequestsPanel({
  bridge,
  daemon,
  registrationNeeded,
  registrationBusy,
  onRegisterCaller,
}: CallerRequestsPanelProps) {
  const [rows, setRows] = useState<CallerRequestRow[] | null>(null);
  const [truncated, setTruncated] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const requestSequence = useRef(0);
  const offline = daemon.state === "stopped";

  const load = useCallback(async () => {
    const sequence = ++requestSequence.current;
    setLoadError(null);
    try {
      // Both slices are daemon-global: always the daemon authority.
      const [callers, activity] = await Promise.all([
        bridge.callerRegistry(withDaemonOperation()),
        bridge.daemonActivity(withDaemonOperation()),
      ]);
      if (sequence !== requestSequence.current) return;
      if (callers.status !== "ok") {
        setLoadError([callers.failure.detail, callers.failure.recovery].filter(Boolean).join(" "));
        return;
      }
      if (activity.status !== "ok") {
        setLoadError([activity.failure.detail, activity.failure.recovery].filter(Boolean).join(" "));
        return;
      }
      setRows(aggregateCallerRequests(callers.callers, activity.events));
      setTruncated(activity.truncated);
    } catch (error) {
      if (sequence === requestSequence.current) setLoadError(presentError(error));
    }
  }, [bridge]);

  useEffect(() => {
    if (offline) {
      setRows(null);
      setLoadError(null);
      return;
    }
    void load();
    return () => {
      requestSequence.current += 1;
    };
  }, [load, offline]);

  return (
    <section className="panel" aria-labelledby="caller-requests-heading">
      <div className="panel-title">
        <div>
          <span className="eyebrow">Daemon requests</span>
          <h2 id="caller-requests-heading">Requests per caller</h2>
        </div>
        {truncated && <small>recent window</small>}
      </div>
      {registrationNeeded && !offline && (
        <div className="access-list">
          <article>
            <span className="access-icon" aria-hidden="true"><UserCircle size={21} /></span>
            <div>
              <strong>This desktop</strong>
              <p>PAM has not registered this GUI as a caller yet.</p>
            </div>
            <button
              type="button"
              className="button button--primary button--small"
              disabled={registrationBusy}
              onClick={onRegisterCaller}
            >
              {registrationBusy ? "Registering…" : "Register GUI caller"}
            </button>
          </article>
        </div>
      )}
      {offline ? (
        <p className="panel-empty">PAM is paused, so no requests are being served.</p>
      ) : loadError ? (
        <p className="panel-empty" role="alert">{loadError}</p>
      ) : !rows ? (
        <p className="panel-empty" aria-busy="true" aria-live="polite">Loading the recent requests…</p>
      ) : rows.length === 0 ? (
        <p className="panel-empty">No caller has talked to the daemon yet.</p>
      ) : (
        <div className="access-list">
          {rows.map((row) => (
            <article key={row.callerId}>
              <span className="access-icon" aria-hidden="true"><UserCircle size={21} /></span>
              <div>
                <strong>{row.callerId}</strong>
                <p>{row.requests.toLocaleString()} request{row.requests === 1 ? "" : "s"} recently</p>
              </div>
              <span className={`state-pill state-pill--${row.revoked ? "attention" : "observed"}`}>
                {row.revoked ? "revoked" : "active"}
              </span>
            </article>
          ))}
        </div>
      )}
    </section>
  );
}

export interface ControlCenterViewProps {
  bridge: PamBridge;
  daemon: DaemonView;
  modelStatus: ModelStatusDto | null;
  modelBusy: boolean;
  onOpenModelChat: (modelId: string, returnFocusTarget?: HTMLElement) => void;
  onStartWithModel: (modelId: string) => void;
  onModelImported: () => void;
  registrationNeeded?: boolean;
  registrationBusy?: boolean;
  onRegisterCaller?: () => void;
}

export function ControlCenterView({
  bridge,
  daemon,
  modelStatus,
  modelBusy,
  onOpenModelChat,
  onStartWithModel,
  onModelImported,
  registrationNeeded = false,
  registrationBusy = false,
  onRegisterCaller = () => {},
}: ControlCenterViewProps) {
  return (
    <main className="canvas" id="main-content">
      <header className="project-header compact">
        <div>
          <h1>Control center</h1>
          <p>Everything PAM watches, at a glance.</p>
        </div>
      </header>
      <OverviewPanel bridge={bridge} daemon={daemon} />
      <ModelPanel
        bridge={bridge}
        daemon={daemon}
        modelStatus={modelStatus}
        modelBusy={modelBusy}
        onOpenModelChat={onOpenModelChat}
        onStartWithModel={onStartWithModel}
        onModelImported={onModelImported}
      />
      <CallerRequestsPanel
        bridge={bridge}
        daemon={daemon}
        registrationNeeded={registrationNeeded}
        registrationBusy={registrationBusy}
        onRegisterCaller={onRegisterCaller}
      />
    </main>
  );
}
