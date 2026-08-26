import { ArrowClockwise, UserCircle } from "@phosphor-icons/react";
import { useCallback, useEffect, useRef, useState } from "react";
import { Tab, TabList, TabPanel, Tabs } from "react-aria-components";
import { withDaemonOperation } from "../bridge";
import type { CallersDto, PamBridge } from "../domain";
import { presentError } from "../state";
import { useMediaQuery, WIDE_VIEWPORT_QUERY } from "../useMediaQuery";
import { ConnectorsPanel } from "./ConnectorsPanel";

function formatDate(registeredAtMs: number): string {
  const date = new Date(registeredAtMs);
  return Number.isNaN(date.valueOf())
    ? "Date unavailable"
    : new Intl.DateTimeFormat(undefined, { dateStyle: "medium" }).format(date);
}

// e.g. "GUI · bfb1974c…" for a caller with a declared kind — production
// caller IDs are UUIDs, so only the first 8 characters are shown, with the
// full ID as a tooltip. Legacy callers with no recorded kind render exactly
// as before: the full ID, no badge.
function CallerLabel({ callerId, kind }: { callerId: string; kind: string | null }) {
  if (!kind) return <strong>{callerId}</strong>;
  return (
    <strong title={callerId}>
      <span className="state-pill state-pill--observed">{kind.toUpperCase()}</span> {callerId.slice(0, 8)}…
    </strong>
  );
}

export interface ConnectionsViewProps {
  bridge: PamBridge;
}

function CallersPanel({ bridge }: ConnectionsViewProps) {
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
        <p className="panel-empty" role="alert">{loadError}</p>
      ) : !callers ? (
        <p className="panel-empty" aria-busy="true" aria-live="polite">Loading the registered callers…</p>
      ) : callers.status !== "ok" ? (
        <p className="panel-empty">
          {[callers.failure.detail, callers.failure.recovery].filter(Boolean).join(" ")}
        </p>
      ) : callers.callers.length === 0 ? (
        <p className="panel-empty">No callers are registered with the daemon yet.</p>
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

export function ConnectionsView({ bridge }: ConnectionsViewProps) {
  const wide = useMediaQuery(WIDE_VIEWPORT_QUERY);
  return (
    <main className="canvas" id="main-content">
      <header className="project-header compact">
        <div><h1>Connections</h1><p>Who PAM listens to, and where it reaches out.</p></div>
      </header>
      {wide ? (
        <div className="project-detail wide-split">
          <CallersPanel bridge={bridge} />
          <ConnectorsPanel bridge={bridge} />
        </div>
      ) : (
        <Tabs className="panel project-detail" defaultSelectedKey="callers">
          <TabList className="flow-inspector-tabs" aria-label="Connection panels">
            <Tab id="callers" className="flow-inspector-tab">Callers</Tab>
            <Tab id="connectors" className="flow-inspector-tab">Connectors</Tab>
          </TabList>
          <TabPanel id="callers" className="project-detail-panel">
            <CallersPanel bridge={bridge} />
          </TabPanel>
          <TabPanel id="connectors" className="project-detail-panel">
            <ConnectorsPanel bridge={bridge} />
          </TabPanel>
        </Tabs>
      )}
    </main>
  );
}
