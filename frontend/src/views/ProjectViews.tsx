import { FileText, GitBranch, LockSimple, Pulse, WarningCircle } from "@phosphor-icons/react";
import type { ReactNode } from "react";
import type { ControlCenterView } from "../selectors";

export interface AccessViewProps {
  data: ControlCenterView;
  contextBar?: ReactNode;
}

export function AccessView({ data, contextBar }: AccessViewProps) {
  const accessIcon = (id: string) => id === "model"
    ? Pulse
    : id === "policy"
      ? LockSimple
      : id === "certificates"
        ? FileText
        : id === "network"
          ? GitBranch
          : WarningCircle;
  return (
    <main className="canvas" id="main-content">
      <section className="project-detail-view">
        <header className="project-header compact"><div><h1>Access</h1><p>Narrow capabilities, visible to the developer.</p></div>{contextBar}</header>
        <section className="panel access-panel" aria-labelledby="access-heading">
          <div className="panel-title"><div><span className="eyebrow">Project boundary</span><h2 id="access-heading">Authorized capabilities</h2></div><LockSimple size={22} /></div>
          <div className="access-list">
            {data.access.length === 0 ? <p className="panel-empty">No access grants are configured for this project.</p> : data.access.map((grant) => {
                const Icon = accessIcon(grant.id);
                return <article key={grant.id}>
                <span className="access-icon"><Icon size={21} /></span>
                <div><strong>{grant.name}</strong><p>{grant.summary}</p></div>
                <span className={`state-pill state-pill--${grant.state}`}>{grant.state}</span>
              </article>;
            })}
          </div>
        </section>
      </section>
    </main>
  );
}
