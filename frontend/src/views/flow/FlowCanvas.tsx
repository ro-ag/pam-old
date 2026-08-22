import {
  Handle,
  Position,
  ReactFlow,
  type Node,
  type NodeChange,
  type NodeProps,
} from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import { useMemo, useState } from "react";
import type { FlowDefinitionJson, FlowStepJson } from "../../domain";
import { actionSummary, layoutSteps, stepEdges, type StepPosition } from "./graph";

type StepNodeType = Node<{ step: FlowStepJson }, "step">;

function StepNode({ data }: NodeProps<StepNodeType>) {
  const { step } = data;
  return (
    <div className="flow-node">
      <Handle type="target" position={Position.Left} />
      <strong>{step.id}</strong>
      <span className="flow-node-badges">
        {step.semantic && <span className={`state-pill flow-badge flow-badge--${step.semantic}`}>{step.semantic}</span>}
        <span className={`state-pill flow-badge flow-badge--${step.effect}`}>{step.effect === "read_only" ? "read only" : "stateful"}</span>
      </span>
      <small>{actionSummary(step.action)}</small>
      <Handle type="source" position={Position.Right} />
    </div>
  );
}

const nodeTypes = { step: StepNode };

export interface FlowCanvasProps {
  definition: FlowDefinitionJson;
  selectedStepId: string | null;
  onSelectStep: (id: string | null) => void;
}

// Node positions are view-state only: dragging never touches the definition.
export function FlowCanvas({ definition, selectedStepId, onSelectStep }: FlowCanvasProps) {
  const [dragged, setDragged] = useState<ReadonlyMap<string, StepPosition>>(new Map());
  const layout = useMemo(() => layoutSteps(definition.steps), [definition.steps]);
  const nodes: StepNodeType[] = definition.steps.map((step) => ({
    id: step.id,
    type: "step",
    position: dragged.get(step.id) ?? layout.get(step.id) ?? { x: 0, y: 0 },
    selected: step.id === selectedStepId,
    data: { step },
  }));
  const edges = useMemo(() => stepEdges(definition.steps), [definition.steps]);
  const onNodesChange = (changes: NodeChange<StepNodeType>[]) => {
    setDragged((current) => {
      const moved = changes.filter((change) => change.type === "position" && change.position);
      if (moved.length === 0) return current;
      const next = new Map(current);
      for (const change of moved) {
        if (change.type === "position" && change.position) next.set(change.id, change.position);
      }
      return next;
    });
  };
  return (
    <div className="flow-canvas" aria-label="Flow step canvas">
      <ReactFlow
        nodes={nodes}
        edges={edges}
        nodeTypes={nodeTypes}
        onNodesChange={onNodesChange}
        onNodeClick={(_event, node) => onSelectStep(node.id)}
        onPaneClick={() => onSelectStep(null)}
        fitView
      />
    </div>
  );
}
