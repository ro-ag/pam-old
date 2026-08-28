import { ArrowClockwise, PuzzlePiece, WarningCircle } from "@phosphor-icons/react";
import { QueryClient, QueryClientProvider, useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { sameFence, withOperation } from "../bridge";
import { PanelEmpty, PanelError, PanelLoading } from "../components/PanelState";
import type { CommandFence, PamBridge, SkillInventoryDataDto } from "../domain";
import { presentError } from "../state";

function label(value: string): string {
  return value.replaceAll("_", " ");
}

function driftSummary(data: SkillInventoryDataDto): string {
  const { added, changed, removed, resurrected } = data.drift;
  if (added === 0 && changed === 0 && removed === 0 && resurrected === 0) {
    return "No inventory drift detected.";
  }
  return `Inventory drift: ${added} added, ${changed} changed, ${removed} removed, ${resurrected} restored.`;
}

export interface SkillInventoryPanelProps {
  bridge: PamBridge;
  fence: CommandFence;
  /** Bumped by the library panel after a verified mutation so the observed
   * inventory re-scans instead of showing pre-mutation artifacts. */
  refreshTick?: number;
}

export function SkillInventoryPanel({ bridge, fence, refreshTick = 0 }: SkillInventoryPanelProps) {
  const [queryClient] = useState(() => new QueryClient({
    defaultOptions: { queries: { gcTime: 0, retry: false, staleTime: 0 } },
  }));
  return (
    <QueryClientProvider client={queryClient}>
      <SkillInventoryContent bridge={bridge} fence={fence} refreshTick={refreshTick} />
    </QueryClientProvider>
  );
}

// Global scopes (user home, plugins, system, PAM-managed) always take
// priority over project scopes: a project artifact whose name matches a
// global one is shadowed and never wins the agent's attention.
const GLOBAL_SCOPES = new Set(["managed", "system", "user", "plugin"]);

function renderArtifact(artifact: SkillInventoryDataDto["artifacts"][number], shadowed: boolean) {
  return (
    <article key={artifact.id}>
      <span className="access-icon"><PuzzlePiece size={20} /></span>
      <div>
        <strong>{artifact.name}</strong>
        <p>{artifact.logicalPath}</p>
        <small>{label(artifact.kind)} · {label(artifact.scope)} · {label(artifact.loadSemantics)}{shadowed ? " · shadowed by global" : ""}</small>
      </div>
      <span className="state-pill">{label(artifact.origin)}</span>
    </article>
  );
}

function SkillInventoryContent({ bridge, fence, refreshTick = 0 }: SkillInventoryPanelProps) {
  const inventoryQuery = useQuery({
    queryKey: ["skill-inventory", fence.projectHandle, fence.generation, refreshTick],
    queryFn: async (): Promise<SkillInventoryDataDto> => {
      const requestFence = withOperation(fence);
      const response = await bridge.loadSkillInventory(requestFence);
      if (!sameFence(requestFence, response.fence)) {
        throw new Error("The skill inventory response did not match the active project request. Retry inventory.");
      }
      return response.data;
    },
  });
  const inventory = inventoryQuery.data ?? null;
  const error = inventoryQuery.error ? presentError(inventoryQuery.error) : null;
  const loading = inventoryQuery.isPending;
  const globalArtifacts = inventory?.artifacts.filter((artifact) => GLOBAL_SCOPES.has(artifact.scope)) ?? [];
  const projectArtifacts = inventory?.artifacts.filter((artifact) => !GLOBAL_SCOPES.has(artifact.scope)) ?? [];
  const globalNames = new Set(globalArtifacts.map((artifact) => artifact.name));

  return (
    <section className="panel skill-inventory-panel" aria-labelledby="skill-inventory-heading">
      <div className="panel-title">
        <div><span className="eyebrow">Agent ecosystems</span><h2 id="skill-inventory-heading">Skill inventory</h2></div>
        <PuzzlePiece size={22} />
      </div>
      {loading && !inventory ? (
        <PanelLoading as="div" className="skill-inventory-state">Scanning bounded local agent configuration…</PanelLoading>
      ) : error && !inventory ? (
        <PanelError
          as="div"
          className="skill-inventory-state is-error"
          icon={<WarningCircle size={24} />}
          title="Skill inventory unavailable"
          action={<button type="button" className="button button--secondary" onClick={() => { void inventoryQuery.refetch(); }}><ArrowClockwise size={18} /> Retry inventory</button>}
        >{error}</PanelError>
      ) : inventory ? (
        <>
          <div className="skill-inventory-summary" role="status">
            <span>{driftSummary(inventory)}</span>
            <span>Cursor global rules: {label(inventory.cursorGlobalRulesStatus)}.</span>
          </div>
          {inventory.artifacts.length === 0 ? (
            <PanelEmpty>No supported agent artifacts were found in this scope.</PanelEmpty>
          ) : (
            <div className="skill-inventory-list">
              {globalArtifacts.length > 0 && (
                <>
                  <p className="skill-inventory-group">Global — wins over same-name project skills</p>
                  {globalArtifacts.map((artifact) => renderArtifact(artifact, false))}
                </>
              )}
              {projectArtifacts.length > 0 && (
                <>
                  <p className="skill-inventory-group">This project</p>
                  {projectArtifacts.map((artifact) => renderArtifact(artifact, globalNames.has(artifact.name)))}
                </>
              )}
            </div>
          )}
          {inventory.truncated && <p className="skill-inventory-truncated">Showing {inventory.artifacts.length} of {inventory.total} artifacts. The native response is bounded.</p>}
        </>
      ) : null}
    </section>
  );
}
