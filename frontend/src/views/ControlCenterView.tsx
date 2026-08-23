import type { ReactNode } from "react";
import { CurrentView, type CurrentViewProps } from "./ProjectViews";

export type ControlCenterViewProps = CurrentViewProps & { contextBar?: ReactNode };

export function ControlCenterView({ contextBar, ...props }: ControlCenterViewProps) {
  return (
    <main className="canvas" id="main-content">
      <header className="project-header compact">
        <div><h1>Control center</h1><p>The project's queue, runs, and outcomes in one calm place.</p></div>
        {contextBar}
      </header>
      <CurrentView {...props} />
    </main>
  );
}
