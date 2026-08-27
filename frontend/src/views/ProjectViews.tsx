import { FileText, GitBranch, LockSimple, Pulse, WarningCircle } from "@phosphor-icons/react";
import { useCallback, useEffect, useRef, useState } from "react";
import { withDaemonOperation } from "../bridge";
import type { AccessGrantView } from "../selectors";
import { accessView } from "../selectors";
import type { PamBridge } from "../domain";
import { presentError } from "../state";
import { DaemonAccessPanel } from "./DaemonAccessPanel";

export interface AccessViewProps {
  bridge: PamBridge;
}

// Access is global: both panels describe this PAM window, never a project.
// The observed boundary is daemon TLS/proxy truth, so it is read over the
// daemon authority and renders with or without an active project.
export function AccessView({ bridge }: AccessViewProps) {
  const accessIcon = (id: string) => id === "model"
    ? Pulse
    : id === "policy"
      ? LockSimple
      : id === "certificates"
        ? FileText
        : id === "network"
          ? GitBranch
          : WarningCircle;
  const [grants, setGrants] = useState<AccessGrantView[] | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const requestSequence = useRef(0);

  const load = useCallback(async () => {
    const sequence = ++requestSequence.current;
    setLoadError(null);
    try {
      // A fresh operation per call: the daemon authority rejects a replayed one.
      const access = await bridge.daemonAccessConfig(withDaemonOperation());
      if (sequence !== requestSequence.current) return;
      setGrants(accessView(access));
    } catch (error) {
      if (sequence === requestSequence.current) setLoadError(presentError(error));
    }
  }, [bridge]);

  useEffect(() => {
    void load();
    return () => { requestSequence.current += 1; };
  }, [load]);

  return (
    <main className="canvas" id="main-content">
      <section className="project-detail-view">
        <header className="project-header compact"><div><h1>Access</h1><p>Narrow capabilities, visible to the developer.</p></div></header>
        <DaemonAccessPanel bridge={bridge} />
        <section className="panel access-panel" aria-labelledby="access-heading">
          <div className="panel-title"><div><span className="eyebrow">Observed boundary</span><h2 id="access-heading">Authorized capabilities</h2></div><LockSimple size={22} /></div>
          {loadError && <p className="panel-empty" role="alert">{loadError}</p>}
          {grants === null && !loadError && <p className="panel-empty" aria-busy="true" aria-live="polite">Reading the observed boundary…</p>}
          {grants !== null && (
            <div className="access-list">
              {grants.length === 0 ? <p className="panel-empty">The daemon has not reported an access boundary yet.</p> : grants.map((grant) => {
                  const Icon = accessIcon(grant.id);
                  return <article key={grant.id}>
                  <span className="access-icon"><Icon size={21} /></span>
                  <div><strong>{grant.name}</strong><p>{grant.summary}</p></div>
                  <span className={`state-pill state-pill--${grant.state}`}>{grant.state}</span>
                </article>;
              })}
            </div>
          )}
        </section>
      </section>
    </main>
  );
}
