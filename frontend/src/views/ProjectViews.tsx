import { FileText, GitBranch, LockSimple, Pulse, WarningCircle } from "@phosphor-icons/react";
import { useCallback, useEffect, useRef, useState } from "react";
import { Tab, TabList, TabPanel, Tabs } from "react-aria-components";
import { withDaemonOperation } from "../bridge";
import { PanelEmpty, PanelError, PanelLoading } from "../components/PanelState";
import type { AccessGrantView, DaemonView } from "../selectors";
import { accessView } from "../selectors";
import type { PamBridge } from "../domain";
import { presentError } from "../state";
import { useMediaQuery, WIDE_VIEWPORT_QUERY } from "../useMediaQuery";
import { CallerRequestsPanel, CallersPanel } from "./CallersPanel";
import { ConnectorsPanel } from "./ConnectorsPanel";
import { DaemonAccessPanel } from "./DaemonAccessPanel";

export interface AccessViewProps {
  bridge: PamBridge;
  daemon: DaemonView;
  /** True when the desktop's own GUI caller still needs registration. */
  registrationNeeded?: boolean;
  registrationBusy?: boolean;
  onRegisterCaller?: () => void;
}

// Access is global: every panel describes this PAM window, never a project.
// The observed boundary is daemon TLS/proxy truth, so it is read over the
// daemon authority and renders with or without an active project.
export function AccessView({
  bridge,
  daemon,
  registrationNeeded = false,
  registrationBusy = false,
  onRegisterCaller = () => {},
}: AccessViewProps) {
  const wide = useMediaQuery(WIDE_VIEWPORT_QUERY);
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
        <header className="project-header compact"><div><h1>Access</h1><p>Who PAM listens to, what it may do, and where it reaches out.</p></div></header>
        <DaemonAccessPanel bridge={bridge} onGrantsChanged={() => void load()} />
        <section className="panel access-panel" aria-labelledby="access-heading">
          <div className="panel-title"><div><span className="eyebrow">Observed boundary</span><h2 id="access-heading">Authorized capabilities</h2></div><LockSimple size={22} /></div>
          {loadError && <PanelError>{loadError}</PanelError>}
          {grants === null && !loadError && <PanelLoading>Reading the observed boundary…</PanelLoading>}
          {grants !== null && (
            <div className="access-list">
              {grants.length === 0 ? <PanelEmpty>The daemon has not reported an access boundary yet.</PanelEmpty> : grants.map((grant) => {
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
        <CallerRequestsPanel
          bridge={bridge}
          daemon={daemon}
          registrationNeeded={registrationNeeded}
          registrationBusy={registrationBusy}
          onRegisterCaller={onRegisterCaller}
        />
      </section>
    </main>
  );
}
