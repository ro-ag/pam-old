import type { FlowActionJson, FlowDefinitionJson, FlowStepJson } from "../../domain";

export interface StepPosition {
  x: number;
  y: number;
}

// Deterministic layered layout: a step's column is its longest dependency
// depth, its row is the order of appearance within that column.
export function layoutSteps(steps: FlowStepJson[]): Map<string, StepPosition> {
  const byId = new Map(steps.map((step) => [step.id, step]));
  const depths = new Map<string, number>();
  const depthOf = (id: string, trail: Set<string>): number => {
    const known = depths.get(id);
    if (known !== undefined) return known;
    if (trail.has(id)) return 0; // Cycles settle in the first column; validate_flow names them.
    trail.add(id);
    const parents = byId.get(id)?.depends_on.filter((dep) => byId.has(dep)) ?? [];
    const depth = parents.length === 0 ? 0 : Math.max(...parents.map((dep) => depthOf(dep, trail))) + 1;
    trail.delete(id);
    depths.set(id, depth);
    return depth;
  };
  const rows = new Map<number, number>();
  const positions = new Map<string, StepPosition>();
  for (const step of steps) {
    const depth = depthOf(step.id, new Set());
    const row = rows.get(depth) ?? 0;
    rows.set(depth, row + 1);
    positions.set(step.id, { x: depth * 320, y: row * 160 });
  }
  return positions;
}

export interface StepEdge {
  id: string;
  source: string;
  target: string;
}

export function stepEdges(steps: FlowStepJson[]): StepEdge[] {
  const ids = new Set(steps.map((step) => step.id));
  return steps.flatMap((step) =>
    step.depends_on
      .filter((dep) => ids.has(dep))
      .map((dep) => ({ id: `${dep}->${step.id}`, source: dep, target: step.id })));
}

export function actionSummary(action: FlowActionJson): string {
  return action.type === "command"
    ? [action.program, ...action.args].join(" ").trim() || "command"
    : `${action.connector} · ${action.capability} · ${action.resource.kind}/${action.resource.id}`;
}

export function updateStep(definition: FlowDefinitionJson, id: string, patch: Partial<FlowStepJson>): FlowDefinitionJson {
  return {
    ...definition,
    steps: definition.steps.map((step) => (step.id === id ? { ...step, ...patch } : step)),
  };
}

// Renaming a step keeps every dependent edge and condition pointing at it.
export function renameStep(definition: FlowDefinitionJson, from: string, to: string): FlowDefinitionJson {
  return {
    ...definition,
    steps: definition.steps.map((step) => {
      const renamed = step.id === from ? { ...step, id: to } : step;
      return {
        ...renamed,
        depends_on: renamed.depends_on.map((dep) => (dep === from ? to : dep)),
        condition: renamed.condition.kind !== "always" && renamed.condition.step === from
          ? { ...renamed.condition, step: to }
          : renamed.condition,
      };
    }),
  };
}

// Deleting a step also strips it from every other step's depends_on, and lets
// conditions that watched it fall back to running always.
export function deleteStep(definition: FlowDefinitionJson, id: string): FlowDefinitionJson {
  return {
    ...definition,
    steps: definition.steps
      .filter((step) => step.id !== id)
      .map((step) => ({
        ...step,
        depends_on: step.depends_on.filter((dep) => dep !== id),
        condition: step.condition.kind !== "always" && step.condition.step === id
          ? { kind: "always" as const }
          : step.condition,
      })),
  };
}

export function addStep(definition: FlowDefinitionJson): { definition: FlowDefinitionJson; id: string } {
  const ids = new Set(definition.steps.map((step) => step.id));
  let counter = definition.steps.length + 1;
  let id = `step-${counter}`;
  while (ids.has(id)) id = `step-${++counter}`;
  const step: FlowStepJson = {
    id,
    description: "",
    depends_on: [],
    condition: { kind: "always" },
    retry: { max_attempts: 1, initial_backoff_ms: 0, max_backoff_ms: 0 },
    approval: "none",
    idempotency_key: null,
    timeout_seconds: 60,
    effect: "read_only",
    semantic: "observe",
    action: { type: "command", program: "", args: [], working_directory: "." },
  };
  return { definition: { ...definition, steps: [...definition.steps, step] }, id };
}
