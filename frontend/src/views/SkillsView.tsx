import { Tab, TabList, TabPanel, Tabs } from "react-aria-components";
import type { CommandFence, PamBridge } from "../domain";
import { SkillAuditReportPanel } from "./SkillAuditReportPanel";
import { SkillInventoryPanel } from "./SkillInventoryPanel";
import { SkillLibraryPanel } from "./SkillLibraryPanel";

export interface SkillsViewProps {
  bridge: PamBridge;
  fence: CommandFence;
}

export function SkillsView({ bridge, fence }: SkillsViewProps) {
  return (
    <main className="canvas" id="main-content">
      <header className="project-header compact">
        <div><h1>Skills</h1><p>What the agents carry with them, kept in view.</p></div>
      </header>
      <Tabs className="panel project-detail" defaultSelectedKey="inventory">
        <TabList className="flow-inspector-tabs" aria-label="Skill panels">
          <Tab id="inventory" className="flow-inspector-tab">Inventory</Tab>
          <Tab id="library" className="flow-inspector-tab">Library</Tab>
          <Tab id="audit" className="flow-inspector-tab">Audit</Tab>
        </TabList>
        <TabPanel id="inventory" className="project-detail-panel">
          <SkillInventoryPanel bridge={bridge} fence={fence} />
        </TabPanel>
        <TabPanel id="library" className="project-detail-panel">
          <SkillLibraryPanel bridge={bridge} fence={fence} />
        </TabPanel>
        <TabPanel id="audit" className="project-detail-panel">
          <SkillAuditReportPanel bridge={bridge} fence={fence} />
        </TabPanel>
      </Tabs>
    </main>
  );
}
