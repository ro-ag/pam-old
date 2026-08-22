import { CurrentView, type CurrentViewProps } from "./ProjectViews";

export type ControlCenterViewProps = CurrentViewProps;

export function ControlCenterView(props: ControlCenterViewProps) {
  return (
    <main className="canvas" id="main-content">
      <header className="project-header compact">
        <div><h1>Control center</h1><p>The project's queue, runs, and outcomes in one calm place.</p></div>
      </header>
      <CurrentView {...props} />
    </main>
  );
}
