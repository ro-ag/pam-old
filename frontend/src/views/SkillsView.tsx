import { useCallback, useMemo, useState, type ReactNode } from "react";
import { Tab, TabList, TabPanel, Tabs } from "react-aria-components";
import { withDaemonOperation } from "../bridge";
import type { CommandFence, PamBridge, ProjectSummaryDto } from "../domain";
import { useMediaQuery, WIDE_VIEWPORT_QUERY } from "../useMediaQuery";
import { SkillAuditReportPanel } from "./SkillAuditReportPanel";
import { SkillInventoryPanel } from "./SkillInventoryPanel";
import { SkillLibraryPanel } from "./SkillLibraryPanel";

export interface SkillsViewProps {
  bridge: PamBridge;
  fence: CommandFence | null;
  projects?: ProjectSummaryDto[];
  onSelectProject?: (project: ProjectSummaryDto) => void;
  contextBar?: ReactNode;
}

// Skills is global first: without an active project every panel speaks to the
// daemon authority, and a project is picked on demand for assignment.
export function SkillsView({ bridge, fence, projects, onSelectProject, contextBar }: SkillsViewProps) {
  const wide = useMediaQuery(WIDE_VIEWPORT_QUERY);
  const authority = useMemo(() => fence ?? withDaemonOperation(), [fence]);
  // Library mutations (install, adopt, apply) change what the inventory
  // observes; the tick re-scans it without a full view remount.
  const [inventoryTick, setInventoryTick] = useState(0);
  const invalidateInventory = useCallback(() => setInventoryTick((tick) => tick + 1), []);
  const library = (
    <SkillLibraryPanel bridge={bridge} fence={authority} projects={projects} onSelectProject={onSelectProject} onMutated={invalidateInventory} />
  );
  return (
    <main className="canvas" id="main-content">
      <header className="project-header compact">
        <div><h1>Skills</h1><p>What the agents carry with them, kept in view.</p></div>
        {contextBar}
      </header>
      {wide ? (
        <>
          <div className="project-detail wide-split">
            <SkillInventoryPanel bridge={bridge} fence={authority} refreshTick={inventoryTick} />
            {library}
          </div>
          <SkillAuditReportPanel bridge={bridge} fence={authority} />
        </>
      ) : (
        <Tabs className="panel project-detail" defaultSelectedKey="inventory">
          <TabList className="flow-inspector-tabs" aria-label="Skill panels">
            <Tab id="inventory" className="flow-inspector-tab">Inventory</Tab>
            <Tab id="library" className="flow-inspector-tab">Library</Tab>
            <Tab id="audit" className="flow-inspector-tab">Audit</Tab>
          </TabList>
          <TabPanel id="inventory" className="project-detail-panel">
            <SkillInventoryPanel bridge={bridge} fence={authority} refreshTick={inventoryTick} />
          </TabPanel>
          <TabPanel id="library" className="project-detail-panel">
            {library}
          </TabPanel>
          <TabPanel id="audit" className="project-detail-panel">
            <SkillAuditReportPanel bridge={bridge} fence={authority} />
          </TabPanel>
        </Tabs>
      )}
    </main>
  );
}
