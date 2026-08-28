import { ArrowClockwise, UserCircle } from "@phosphor-icons/react";
import { useCallback, useEffect, useRef, useState } from "react";
import { withDaemonOperation } from "../bridge";
import { PanelEmpty, PanelError, PanelLoading } from "../components/PanelState";
import type { ActivityEventDto, CallerDto, CallersDto, PamBridge } from "../domain";
import type { DaemonView } from "../selectors";
import { presentError } from "../state";

function formatDate(registeredAtMs: number): string {
  const date = new Date(registeredAtMs);
  return Number.isNaN(date.valueOf())
    ? "Date unavailable"
    : new Intl.DateTimeFormat(undefined, { dateStyle: "medium" }).format(date);
}

/** Above this an ID is an opaque handle, not a name worth reading in full. */
const READABLE_CALLER_ID = 24;

// e.g. "GUI 8f14e45f…" for a caller with a declared kind. Production caller
// IDs are UUIDs, so a long ID is shortened whether or not a kind badge is
// present — a legacy row (no recorded kind) is otherwise two indistinguishable
// 36-character strings. The full ID is always the tooltip.
/** One shortening rule, so every surface that prints a caller ID agrees. */
export function shortCallerId(callerId: string): string {
  return callerId.length > READABLE_CALLER_ID ? `${callerId.slice(0, 8)}…` : callerId;
}

export function CallerLabel({ callerId, kind }: { callerId: string; kind: string | null }) {
  const label = shortCallerId(callerId);
  return (
    <strong title={callerId}>
      {kind && <span className="state-pill state-pill--observed">{kind.toUpperCase()}</span>}
      {kind ? ` ${label}` : label}
    </strong>
  );
}

export interface CallersPanelProps {
  bridge: PamBridge;
}

export function CallersPanel({ bridge }: CallersPanelProps) {
  const [callers, setCallers] = useState<CallersDto | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const requestSequence = useRef(0);

  const load = useCallback(async () => {
    const sequence = ++requestSequence.current;
    setBusy(true);
    setLoadError(null);
    try {
      // The caller registry is daemon-global: always the daemon authority.
      const response = await bridge.callerRegistry(withDaemonOperation());
      if (sequence !== requestSequence.current) return;
      setCallers(response);
    } catch (error) {
      if (sequence === requestSequence.current) setLoadError(presentError(error));
    } finally {
      if (sequence === requestSequence.current) setBusy(false);
    }
  }, [bridge]);

  useEffect(() => {
    void load();
    return () => { requestSequence.current += 1; };
  }, [load]);

  return (
    <section className="panel" aria-labelledby="callers-heading">
      <div className="panel-title">
        <div><span className="eyebrow">Daemon registry</span><h2 id="callers-heading">Registered callers</h2></div>
        <button
          type="button"
          className="button button--secondary button--small"
          aria-label="Refresh callers"
          disabled={busy}
          onClick={() => void load()}
        >
          <ArrowClockwise className={busy ? "is-spinning" : ""} size={17} /> Refresh
        </button>
      </div>
      {loadError ? (
        <PanelError>{loadError}</PanelError>
      ) : !callers ? (
        <PanelLoading>Loading the registered callers…</PanelLoading>
      ) : callers.status !== "ok" ? (
        <PanelEmpty>
          {[callers.failure.detail, callers.failure.recovery].filter(Boolean).join(" ")}
        </PanelEmpty>
      ) : callers.callers.length === 0 ? (
        <PanelEmpty>No callers are registered with the daemon yet.</PanelEmpty>
      ) : (
        <div className="access-list">
          {callers.callers.map((caller) => (
            <article key={caller.callerId}>
              <span className="access-icon" aria-hidden="true"><UserCircle size={21} /></span>
              <div>
                <CallerLabel callerId={caller.callerId} kind={caller.kind} />
                <p>Registered {formatDate(caller.registeredAtMs)}</p>
              </div>
              <span className={`state-pill state-pill--${caller.revokedAtMs === null ? "observed" : "attention"}`}>
                {caller.revokedAtMs === null ? "active" : "revoked"}
              </span>
            </article>
          ))}
        </div>
      )}
    </section>
  );
}

export interface CallerRequestRow {
  callerId: string;
  requests: number;
  revoked: boolean;
  /** Self-declared local caller surface; null for legacy or unregistered callers. */
  kind: string | null;
}

// Recent daemon requests, grouped by caller. Registered-but-quiet callers stay visible with a zero count.
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
      kind: caller.kind,
    });
  }
  for (const event of events) {
    const row = rows.get(event.callerId) ?? {
      callerId: event.callerId,
      requests: 0,
      revoked: false,
      kind: null,
    };
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
  /** Bumped by ⌘R; re-runs the mount-time fetch without a remount. */
  refreshTick?: number;
}

export function CallerRequestsPanel({
  bridge,
  daemon,
  registrationNeeded,
  registrationBusy,
  onRegisterCaller,
  refreshTick = 0,
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
  }, [load, offline, refreshTick]);

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
        <PanelEmpty>PAM is paused, so no requests are being served.</PanelEmpty>
      ) : loadError ? (
        <PanelError>{loadError}</PanelError>
      ) : !rows ? (
        <PanelLoading>Loading the recent requests…</PanelLoading>
      ) : rows.length === 0 ? (
        <PanelEmpty>No caller has talked to the daemon yet.</PanelEmpty>
      ) : (
        <div className="access-list">
          {rows.map((row) => (
            <article key={row.callerId}>
              <span className="access-icon" aria-hidden="true"><UserCircle size={21} /></span>
              <div>
                <CallerLabel callerId={row.callerId} kind={row.kind} />
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
