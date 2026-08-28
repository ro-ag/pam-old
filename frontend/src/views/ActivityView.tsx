import {
  ArrowClockwise,
  CheckCircle,
  FileText,
  Power,
  Pulse,
  Queue,
  WarningCircle,
} from "@phosphor-icons/react";
import { useCallback, useEffect, useRef, useState } from "react";
import { DAEMON_AUTHORITY, withDaemonOperation } from "../bridge";
import type { ActivityDto, ActivityEventDto, PamBridge, ProjectSummaryDto } from "../domain";
import { basename, type DaemonView } from "../selectors";
import { presentError } from "../state";
import { shortCallerId } from "./CallersPanel";
import { ConsolePanel } from "./ConsolePanel";

function formatClock(occurredAtMs: number): string {
  const date = new Date(occurredAtMs);
  return Number.isNaN(date.valueOf())
    ? "Time unavailable"
    : new Intl.DateTimeFormat(undefined, { hour: "numeric", minute: "2-digit" }).format(date);
}

export function formatModelSize(sizeBytes: number): string {
  if (sizeBytes >= 1_000_000_000) return `${(sizeBytes / 1_000_000_000).toFixed(1)} GB`;
  if (sizeBytes >= 1_000_000) return `${Math.round(sizeBytes / 1_000_000)} MB`;
  return `${sizeBytes.toLocaleString()} bytes`;
}

export interface ActivityEvidence {
  projectName: string;
  handles: string[];
  truncated: boolean;
}

export interface ActivityViewProps {
  daemon: DaemonView;
  projects: ProjectSummaryDto[];
  bridge: PamBridge;
  pending: boolean;
  /** Latest terminal-result evidence for the active project, if any. */
  evidence: ActivityEvidence | null;
  onEvidence: (handle: string) => void;
  onStartDaemon: () => void;
}

export function ActivityView({ daemon, projects, bridge, pending, evidence, onEvidence, onStartDaemon }: ActivityViewProps) {
  const [activity, setActivity] = useState<ActivityDto | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const requestSequence = useRef(0);
  const offline = daemon.state === "stopped";

  const load = useCallback(async () => {
    const sequence = ++requestSequence.current;
    setBusy(true);
    setLoadError(null);
    try {
      // The activity feed is daemon-global: always the daemon authority.
      const response = await bridge.daemonActivity(withDaemonOperation());
      if (sequence !== requestSequence.current) return;
      setActivity(response);
    } catch (error) {
      if (sequence === requestSequence.current) setLoadError(presentError(error));
    } finally {
      if (sequence === requestSequence.current) setBusy(false);
    }
  }, [bridge]);

  useEffect(() => {
    if (offline) {
      setActivity(null);
      setLoadError(null);
      return;
    }
    void load();
    return () => { requestSequence.current += 1; };
  }, [load, offline]);

  // Events carry the daemon's own project ID, not the GUI-local catalog
  // handle, so the two never match; the shared identity is the canonical root
  // the daemon remembers per project. Match on that, and degrade honestly.
  const projectLabel = (projectId: string | null, projectRoot: string | null) => {
    if (projectId === null || projectId === DAEMON_AUTHORITY) return { text: "daemon" };
    const known = projectRoot === null
      ? undefined
      : projects.find((project) => project.location === projectRoot);
    if (known) return { text: known.name, title: known.location };
    if (projectRoot) return { text: basename(projectRoot), title: projectRoot };
    return { text: `${projectId.slice(0, 8)}…`, title: projectId };
  };

  const eventRow = (event: ActivityEventDto) => {
    const label = projectLabel(event.projectId, event.projectRoot);
    return (
    <article key={event.sequence}>
      <span className="access-icon" aria-hidden="true">
        {event.decision === "allowed" ? <CheckCircle size={21} /> : <WarningCircle size={21} />}
      </span>
      <div>
        <strong>{event.action}</strong>
        <p title={label.title}>
          {formatClock(event.occurredAtMs)} · {shortCallerId(event.callerId)} · {label.text}
          {event.outcome && ` · ${event.outcome}`}
        </p>
      </div>
      <span className={`state-pill state-pill--${event.decision === "allowed" ? "allowed" : "attention"}`}>
        {event.decision.replaceAll("_", " ")}
      </span>
    </article>
    );
  };

  return (
    <main className="canvas" id="main-content">
      <header className="project-header compact">
        <div><h1>Activity</h1><p>What PAM has seen across the daemon.</p></div>
      </header>
      <section className="project-overview" aria-label="Daemon health">
        <article className="project-stat group flex items-center gap-3">
          <span className={`project-stat-icon${daemon.state === "running" ? "" : " project-stat-icon--attention"}`}><Pulse size={21} weight="bold" /></span>
          <div><small>Watch status</small><strong title={daemon.detail}>{daemon.detail}</strong></div>
        </article>
        <article className="project-stat group flex items-center gap-3">
          <span className="project-stat-icon"><CheckCircle size={21} weight="bold" /></span>
          <div><small>Daemon version</small><strong title={daemon.model ?? "Not reported"}>{daemon.model ?? "Not reported"}</strong></div>
        </article>
        <article className="project-stat group flex items-center gap-3">
          <span className="project-stat-icon"><Queue size={21} weight="bold" /></span>
          <div><small>Queue depth</small><strong>{daemon.queueDepth ?? "Not reported"}</strong></div>
          {daemon.queueDepth !== null && <span className="project-stat-value">{daemon.queueDepth}</span>}
        </article>
      </section>
      {offline ? (
        <section className="empty-state">
          <Power size={38} aria-hidden="true" />
          <h2>PAM is paused</h2>
          <p>The activity feed will pick up where it left off once PAM is back on watch.</p>
          <button type="button" className="button button--primary" disabled={pending} onClick={onStartDaemon}>
            <Power size={18} /> Start PAM
          </button>
        </section>
      ) : (
        <section className="panel activity-feed" aria-labelledby="activity-heading">
          <div className="panel-title">
            <div><span className="eyebrow">Daemon feed</span><h2 id="activity-heading">Recent activity</h2></div>
            <button
              type="button"
              className="button button--secondary button--small"
              aria-label="Refresh activity"
              disabled={busy}
              onClick={() => { void load(); }}
            >
              <ArrowClockwise className={busy ? "is-spinning" : ""} size={17} /> Refresh
            </button>
          </div>
          {loadError ? (
            <p className="panel-empty" role="alert">{loadError}</p>
          ) : !activity ? (
            <p className="panel-empty" aria-busy="true" aria-live="polite">Loading the recent daemon activity…</p>
          ) : activity.status !== "ok" ? (
            <p className="panel-empty">
              {[activity.failure.detail, activity.failure.recovery].filter(Boolean).join(" ")}
            </p>
          ) : activity.events.length === 0 ? (
            <p className="panel-empty">No recent activity. PAM is on watch and new events will appear here.</p>
          ) : (
            <div className="access-list">
              {activity.events.map(eventRow)}
              {activity.truncated && <p className="panel-empty">Older activity was truncated at the bounded feed limit.</p>}
            </div>
          )}
        </section>
      )}
      {evidence && !offline && (
        <section className="panel" aria-labelledby="evidence-heading">
          <div className="panel-title">
            <div>
              <span className="eyebrow">{evidence.projectName}</span>
              <h2 id="evidence-heading">Latest run evidence</h2>
            </div>
          </div>
          {evidence.handles.length === 0 ? (
            <p className="panel-empty">The latest terminal result reported no evidence handles.</p>
          ) : (
            <div className="evidence-handles">
              {evidence.handles.map((handle, index) => (
                <button
                  type="button"
                  aria-label={`Open Evidence ${index + 1}`}
                  aria-description={handle}
                  title={handle}
                  key={handle}
                  onClick={() => onEvidence(handle)}
                >
                  <FileText size={17} aria-hidden="true" />
                  <span>Evidence {index + 1}</span>
                  <code>{handle.slice(0, 8)}…{handle.slice(-4)}</code>
                </button>
              ))}
              {evidence.truncated && (
                <p className="panel-empty">Additional evidence handles were truncated at the bounded limit.</p>
              )}
            </div>
          )}
        </section>
      )}
      {!offline && <ConsolePanel bridge={bridge} />}
    </main>
  );
}
