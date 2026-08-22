import type { FlowDefinitionJson } from "../../domain";
import { FlowCanvas } from "./FlowCanvas";
import { StepInspector } from "./StepInspector";

export interface VisualEditorProps {
  definition: FlowDefinitionJson;
  selectedStepId: string | null;
  onSelectStep: (id: string | null) => void;
  onChange: (definition: FlowDefinitionJson) => void;
}

// The canvas is a spatial view of the same list the inspector edits; below
// 700px the canvas rests and the inspector carries the whole flow.
export function VisualEditor({ definition, selectedStepId, onSelectStep, onChange }: VisualEditorProps) {
  return (
    <div className="flow-visual" aria-label="Visual flow editor">
      <FlowCanvas definition={definition} selectedStepId={selectedStepId} onSelectStep={onSelectStep} />
      <StepInspector
        definition={definition}
        selectedStepId={selectedStepId}
        onSelectStep={onSelectStep}
        onChange={onChange}
      />
    </div>
  );
}
