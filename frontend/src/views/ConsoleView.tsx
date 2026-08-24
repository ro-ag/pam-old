import { ArrowClockwise, Copy, Power, Terminal } from "@phosphor-icons/react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { withDaemonOperation } from "../bridge";
import type { DaemonLogEntryDto, DaemonLogsDto, PamBridge } from "../domain";
import type { DaemonView } from "../selectors";
import { presentError } from "../state";

const severityFilters = ["all", "info", "warn", "error"] as const;
export type SeverityFilter = (typeof severityFilters)[number];

export function filterEntries(
  entries: DaemonLogEntryDto[],
  filter: SeverityFilter,
): DaemonLogEntryDto[] {
  return filter === "all" ? entries : entries.filter((entry) => entry.severity === filter);
}

export function formatConsoleLine(entry: DaemonLogEntryDto): string {
  const date = new Date(entry.timestampMs);
  const clock = Number.isNaN(date.valueOf()) ? "--:--:--" : date.toISOString();
  return `${clock} ${entry.severity.toUpperCase()} ${entry.message}`;
}

function formatClock(timestampMs: number): string {
  const date = new Date(timestampMs);
  return Number.isNaN(date.valueOf())
    ? "Time unavailable"
    : new Intl.DateTimeFormat(undefined, {
        hour: "numeric",
        minute: "2-digit",
        second: "2-digit",
      }).format(date);
}

export interface ConsoleViewProps {
  daemon: DaemonView;
  bridge: PamBridge;
  pending: boolean;
  onStartDaemon: () => void;
}

export function ConsoleView({ daemon, bridge, pending, onStartDaemon }: ConsoleViewProps) {
  const [logs, setLogs] = useState<DaemonLogsDto | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [filter, setFilter] = useState<SeverityFilter>("all");
  const [busy, setBusy] = useState(false);
  const [copied, setCopied] = useState(false);
  const requestSequence = useRef(0);
  const offline = daemon.state === "stopped";

  const load = useCallback(async () => {
    const sequence = ++requestSequence.current;
    setBusy(true);
    setLoadError(null);
    try {
      // The diagnostic log is daemon-global: always the daemon authority.
      const response = await bridge.daemonLogs(withDaemonOperation());
      if (sequence !== requestSequence.current) return;
      setLogs(response);
    } catch (error) {
      if (sequence === requestSequence.current) setLoadError(presentError(error));
    } finally {
      if (sequence === requestSequence.current) setBusy(false);
    }
  }, [bridge]);

  useEffect(() => {
    if (offline) {
      setLogs(null);
      setLoadError(null);
      return;
    }
    void load();
    return () => { requestSequence.current += 1; };
  }, [load, offline]);

  const visible = useMemo(
    () => (logs?.status === "ok" ? filterEntries(logs.entries, filter) : []),
    [logs, filter],
  );

  const copyVisible = async () => {
    try {
      await navigator.clipboard.writeText(visible.map(formatConsoleLine).join("\n"));
      setCopied(true);
      window.setTimeout(() => setCopied(false), 2000);
    } catch {
      setLoadError("The console lines could not be copied to the clipboard.");
    }
  };

  const entryRow = (entry: DaemonLogEntryDto, index: number) => (
    <article key={`${entry.timestampMs}:${index}`} className="console-line">
      <time dateTime={new Date(entry.timestampMs).toISOString()}>{formatClock(entry.timestampMs)}</time>
      <span className={`state-pill state-pill--${entry.severity === "error" ? "attention" : entry.severity === "warn" ? "observed" : "healthy"}`}>
        {entry.severity}
      </span>
      <p>{entry.message}</p>
    </article>
  );

  return (
    <main className="canvas" id="main-content">
      <header className="project-header compact">
        <div><h1>Console</h1><p>What the daemon wrote down, most recent last.</p></div>
      </header>
      {offline ? (
        <section className="empty-state">
          <Power size={38} aria-hidden="true" />
          <h2>PAM is paused</h2>
          <p>The console picks up as soon as PAM is back on watch. The last on-disk log lives in the daemon&apos;s logs directory.</p>
          <button type="button" className="button button--primary" disabled={pending} onClick={onStartDaemon}>
            <Power size={18} /> Start PAM
          </button>
        </section>
      ) : (
        <section className="panel activity-feed" aria-labelledby="console-heading">
          <div className="panel-title">
            <div><span className="eyebrow">Daemon diagnostics</span><h2 id="console-heading">Debug console</h2></div>
            <div className="flex items-center gap-2" role="group" aria-label="Severity filter">
              {severityFilters.map((level) => (
                <button
                  key={level}
                  type="button"
                  className={`button button--secondary button--small${filter === level ? " is-active" : ""}`}
                  aria-pressed={filter === level}
                  onClick={() => setFilter(level)}
                >
                  {level}
                </button>
              ))}
              <button
                type="button"
                className="button button--secondary button--small"
                aria-label="Copy visible console lines"
                disabled={visible.length === 0}
                onClick={() => { void copyVisible(); }}
              >
                <Copy size={17} /> {copied ? "Copied" : "Copy"}
              </button>
              <button
                type="button"
                className="button button--secondary button--small"
                aria-label="Refresh console"
                disabled={busy}
                onClick={() => { void load(); }}
              >
                <ArrowClockwise className={busy ? "is-spinning" : ""} size={17} /> Refresh
              </button>
            </div>
          </div>
          {loadError ? (
            <p className="panel-empty" role="alert">{loadError}</p>
          ) : !logs ? (
            <p className="panel-empty" aria-busy="true" aria-live="polite">Loading the daemon diagnostics…</p>
          ) : logs.status !== "ok" ? (
            <p className="panel-empty">
              {[logs.failure.detail, logs.failure.recovery].filter(Boolean).join(" ")}
            </p>
          ) : visible.length === 0 ? (
            <p className="panel-empty">
              <Terminal size={17} aria-hidden="true" />{" "}
              {logs.entries.length === 0
                ? "No diagnostics yet. The daemon logs its startup, warnings, and failures here."
                : "No entries match this severity filter."}
            </p>
          ) : (
            <div className="access-list console-list">{visible.map(entryRow)}</div>
          )}
        </section>
      )}
    </main>
  );
}
