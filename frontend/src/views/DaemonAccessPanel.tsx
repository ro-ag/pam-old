import { LockKey } from "@phosphor-icons/react";
import { useCallback, useEffect, useRef, useState } from "react";
import { withDaemonOperation } from "../bridge";
import { PanelEmpty, PanelError, PanelLoading } from "../components/PanelState";
import type { DaemonCapabilityDto, PamBridge } from "../domain";
import { presentError } from "../state";

export interface DaemonAccessPanelProps {
  bridge: PamBridge;
  /**
   * Called after a grant or revoke lands. The observed boundary next to this
   * panel is a separate daemon read whose verdicts depend on these grants, so
   * without this it keeps its pre-grant answer — telling the owner to run the
   * very CLI command the button they just pressed already ran.
   */
  onGrantsChanged?: () => void;
}

// Daemon-scope grants belong to this PAM window, not to any project, so this
// panel always speaks the daemon authority and carries no project identity.
// Nothing here is written until the owner presses Grant.
export function DaemonAccessPanel({ bridge, onGrantsChanged }: DaemonAccessPanelProps) {
  const [capabilities, setCapabilities] = useState<DaemonCapabilityDto[] | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [rowErrors, setRowErrors] = useState<Record<string, string | null>>({});
  const [pending, setPending] = useState<string | null>(null);
  const requestSequence = useRef(0);

  const load = useCallback(async () => {
    const sequence = ++requestSequence.current;
    setLoadError(null);
    try {
      const response = await bridge.daemonAccess(withDaemonOperation());
      if (sequence !== requestSequence.current) return;
      setCapabilities(response.capabilities);
    } catch (error) {
      if (sequence === requestSequence.current) setLoadError(presentError(error));
    }
  }, [bridge]);

  useEffect(() => {
    void load();
    return () => { requestSequence.current += 1; };
  }, [load]);

  const decide = useCallback(async (capability: string, granted: boolean) => {
    const sequence = ++requestSequence.current;
    setPending(capability);
    setRowErrors((current) => ({ ...current, [capability]: null }));
    try {
      const response = await bridge.setDaemonAccess(withDaemonOperation(), capability, granted);
      if (sequence !== requestSequence.current) return;
      setCapabilities(response.capabilities);
      onGrantsChanged?.();
    } catch (error) {
      if (sequence === requestSequence.current) {
        setRowErrors((current) => ({ ...current, [capability]: presentError(error) }));
      }
    } finally {
      if (sequence === requestSequence.current) setPending(null);
    }
  }, [bridge, onGrantsChanged]);

  return (
    <section className="panel" aria-labelledby="daemon-access-heading">
      <div className="panel-title">
        <div><span className="eyebrow">Daemon scope</span><h2 id="daemon-access-heading">Capabilities this window uses</h2></div>
        <LockKey size={22} />
      </div>
      <PanelEmpty>
        These grants belong to this PAM window across every project. PAM never grants one on its own; revoking returns the capability to denied.
      </PanelEmpty>
      {loadError && <PanelError>{loadError}</PanelError>}
      {capabilities === null && !loadError && <PanelLoading>Loading the capability grants…</PanelLoading>}
      {capabilities !== null && (
        <div className="access-list">
          {capabilities.map((row) => (
            <article key={row.capability} aria-label={row.capability}>
              <span className="access-icon" aria-hidden="true"><LockKey size={21} /></span>
              <div>
                <strong>{row.name}</strong>
                <p>{row.summary}</p>
                <p><code>{row.capability}</code></p>
                {rowErrors[row.capability] && <p role="alert">{rowErrors[row.capability]}</p>}
              </div>
              <span className="daemon-access-action">
                <span className={`state-pill state-pill--${row.granted ? "allowed" : "not-reported"}`}>
                  {row.granted ? "granted" : "not granted"}
                </span>
                <button
                  type="button"
                  className="button button--secondary button--small"
                  disabled={pending === row.capability}
                  onClick={() => void decide(row.capability, !row.granted)}
                >
                  {row.granted ? "Revoke" : "Grant"}
                </button>
              </span>
            </article>
          ))}
        </div>
      )}
    </section>
  );
}
