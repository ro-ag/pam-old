import { Plus, TrashSimple } from "@phosphor-icons/react";
import { useEffect, useRef, useState } from "react";
import type {
  FlowConditionJson,
  FlowDefinitionJson,
  FlowSemanticJson,
  FlowStepJson,
} from "../../domain";
import { actionSummary, addStep, deleteStep, renameStep, updateStep } from "./graph";

interface FieldProps {
  label: string;
  hint: string;
  children: React.ReactNode;
}

function Field({ label, hint, children }: FieldProps) {
  return (
    <label className="flow-field">
      <span>{label}</span>
      {children}
      <small>{hint}</small>
    </label>
  );
}

// The id keeps dependents in sync, so it commits on blur or Enter; an empty or
// already-taken id calmly reverts.
function StepIdField({ step, takenIds, onRename }: {
  step: FlowStepJson;
  takenIds: Set<string>;
  onRename: (from: string, to: string) => void;
}) {
  const [value, setValue] = useState(step.id);
  useEffect(() => setValue(step.id), [step.id]);
  const commit = () => {
    const next = value.trim();
    if (!next || next === step.id || takenIds.has(next)) {
      setValue(step.id);
      return;
    }
    onRename(step.id, next);
  };
  return (
    <Field label="Step id" hint="A short name other steps can depend on; renames follow along.">
      <input
        value={value}
        onChange={(event) => setValue(event.target.value)}
        onBlur={commit}
        onKeyDown={(event) => { if (event.key === "Enter") commit(); }}
      />
    </Field>
  );
}

// One argument per line; blank lines fall away when the flow is composed.
function ArgsField({ step, onCommit }: { step: FlowStepJson; onCommit: (args: string[]) => void }) {
  const canonical = step.action.type === "command" ? step.action.args.join("\n") : "";
  const [value, setValue] = useState(canonical);
  const stepId = step.id;
  useEffect(() => setValue(canonical), [stepId, canonical]);
  return (
    <Field label="Arguments" hint="One argument per line, in the order the program expects.">
      <textarea
        rows={3}
        value={value}
        onChange={(event) => {
          setValue(event.target.value);
          onCommit(event.target.value.split("\n").filter((line) => line.length > 0));
        }}
      />
    </Field>
  );
}

export interface StepInspectorProps {
  definition: FlowDefinitionJson;
  selectedStepId: string | null;
  onSelectStep: (id: string | null) => void;
  onChange: (definition: FlowDefinitionJson) => void;
}

export function StepInspector({ definition, selectedStepId, onSelectStep, onChange }: StepInspectorProps) {
  const steps = definition.steps;
  const selected = steps.find((step) => step.id === selectedStepId) ?? null;
  const listRefs = useRef<(HTMLButtonElement | null)[]>([]);
  const focusStepId = selectedStepId ?? steps[0]?.id;
  const otherIds = selected ? steps.map((step) => step.id).filter((id) => id !== selected.id) : [];

  const patch = (change: Partial<FlowStepJson>) => {
    if (selected) onChange(updateStep(definition, selected.id, change));
  };

  const onListKeyDown = (event: React.KeyboardEvent, index: number) => {
    if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
    event.preventDefault();
    const next = event.key === "ArrowDown" ? Math.min(index + 1, steps.length - 1) : Math.max(index - 1, 0);
    onSelectStep(steps[next].id);
    listRefs.current[next]?.focus();
  };

  return (
    <div className="flow-step-inspector">
      <div className="flow-step-list" role="group" aria-label="Flow steps">
        {steps.map((step, index) => (
          <button
            type="button"
            key={step.id}
            ref={(element) => { listRefs.current[index] = element; }}
            tabIndex={step.id === focusStepId ? 0 : -1}
            className={step.id === selectedStepId ? "is-active" : ""}
            aria-pressed={step.id === selectedStepId}
            onClick={() => onSelectStep(step.id)}
            onKeyDown={(event) => onListKeyDown(event, index)}
          >
            <strong>{step.id}</strong>
            <small>{step.semantic ?? "unspecified"} · {step.effect === "read_only" ? "read only" : "stateful"}</small>
          </button>
        ))}
        <button
          type="button"
          className="button button--secondary button--small"
          onClick={() => {
            const added = addStep(definition);
            onChange(added.definition);
            onSelectStep(added.id);
          }}
        >
          <Plus size={15} /> Add step
        </button>
      </div>
      {selected ? (
        <div className="flow-step-fields" role="group" aria-label={`Step ${selected.id} details`}>
          <StepIdField
            step={selected}
            takenIds={new Set(otherIds)}
            onRename={(from, to) => {
              onChange(renameStep(definition, from, to));
              onSelectStep(to);
            }}
          />
          <Field label="Description" hint="One sentence on what this step observes, verifies, or changes.">
            <input
              value={selected.description}
              onChange={(event) => patch({ description: event.target.value })}
            />
          </Field>
          <Field label="Semantic role" hint="The honest meaning of a success: observe, verify, or change.">
            <select
              value={selected.semantic ?? ""}
              onChange={(event) => patch({ semantic: (event.target.value || null) as FlowSemanticJson | null })}
            >
              <option value="">unspecified</option>
              <option value="observe">observe</option>
              <option value="verify">verify</option>
              <option value="change">change</option>
            </select>
          </Field>
          <Field label="Effect" hint="Read-only steps leave the project exactly as they found it.">
            <select
              value={selected.effect}
              onChange={(event) => patch({ effect: event.target.value as FlowStepJson["effect"] })}
            >
              <option value="read_only">read only</option>
              <option value="stateful">stateful</option>
            </select>
          </Field>
          <Field label="Timeout (seconds)" hint="Between 1 and 3600 seconds; the step stops calmly at the limit.">
            <input
              type="number"
              min={1}
              max={3600}
              value={selected.timeout_seconds}
              onChange={(event) => {
                const seconds = Number(event.target.value);
                if (Number.isFinite(seconds) && seconds > 0) patch({ timeout_seconds: seconds });
              }}
            />
          </Field>
          <Field label="Condition" hint="When this step is allowed to run, based on an earlier step's outcome.">
            <select
              value={selected.condition.kind}
              onChange={(event) => {
                const kind = event.target.value as FlowConditionJson["kind"];
                patch({
                  condition: kind === "always"
                    ? { kind }
                    : { kind, step: selected.condition.kind !== "always" ? selected.condition.step : (selected.depends_on[0] ?? otherIds[0] ?? "") },
                });
              }}
            >
              <option value="always">always</option>
              <option value="succeeded" disabled={otherIds.length === 0}>after a step succeeded</option>
              <option value="failed" disabled={otherIds.length === 0}>after a step failed</option>
            </select>
          </Field>
          {selected.condition.kind !== "always" && (
            <Field label="Condition step" hint="The earlier step whose outcome this condition watches.">
              <select
                value={selected.condition.step}
                onChange={(event) => patch({ condition: { ...selected.condition, step: event.target.value } as FlowConditionJson })}
              >
                {otherIds.map((id) => <option key={id} value={id}>{id}</option>)}
              </select>
            </Field>
          )}
          <fieldset className="flow-field flow-depends">
            <legend>Depends on</legend>
            {otherIds.length === 0 && <small>Steps this one waits for appear here once the flow has more steps.</small>}
            {otherIds.map((id) => (
              <label key={id}>
                <input
                  type="checkbox"
                  checked={selected.depends_on.includes(id)}
                  onChange={(event) => patch({
                    depends_on: event.target.checked
                      ? [...selected.depends_on, id]
                      : selected.depends_on.filter((dep) => dep !== id),
                  })}
                />
                {id}
              </label>
            ))}
            {otherIds.length > 0 && <small>Steps that must finish before this one starts.</small>}
          </fieldset>
          <Field label="Action type" hint="A bounded command, or a named connector capability.">
            <select
              value={selected.action.type}
              onChange={(event) => patch({
                action: event.target.value === "command"
                  ? { type: "command", program: "", args: [], working_directory: "." }
                  : { type: "connector", connector: "", capability: "", resource: { kind: "", id: "" } },
              })}
            >
              <option value="command">command</option>
              <option value="connector">connector</option>
            </select>
          </Field>
          {selected.action.type === "command" ? (
            <>
              <Field label="Program" hint="The executable name alone; arguments live below.">
                <input
                  value={selected.action.program}
                  onChange={(event) => selected.action.type === "command" && patch({ action: { ...selected.action, program: event.target.value } })}
                />
              </Field>
              <ArgsField
                step={selected}
                onCommit={(args) => selected.action.type === "command" && patch({ action: { ...selected.action, args } })}
              />
              <Field label="Working directory" hint="Relative to the project root; “.” is the root itself.">
                <input
                  value={selected.action.working_directory}
                  onChange={(event) => selected.action.type === "command" && patch({ action: { ...selected.action, working_directory: event.target.value } })}
                />
              </Field>
            </>
          ) : (
            <>
              <Field label="Connector" hint="The connector this step speaks through.">
                <input
                  value={selected.action.connector}
                  onChange={(event) => selected.action.type === "connector" && patch({ action: { ...selected.action, connector: event.target.value } })}
                />
              </Field>
              <Field label="Capability" hint="The single capability the step exercises.">
                <input
                  value={selected.action.capability}
                  onChange={(event) => selected.action.type === "connector" && patch({ action: { ...selected.action, capability: event.target.value } })}
                />
              </Field>
              <Field label="Resource kind" hint="What kind of thing the capability touches.">
                <input
                  value={selected.action.resource.kind}
                  onChange={(event) => selected.action.type === "connector" && patch({ action: { ...selected.action, resource: { ...selected.action.resource, kind: event.target.value } } })}
                />
              </Field>
              <Field label="Resource id" hint="The exact resource, named so a reviewer can find it.">
                <input
                  value={selected.action.resource.id}
                  onChange={(event) => selected.action.type === "connector" && patch({ action: { ...selected.action, resource: { ...selected.action.resource, id: event.target.value } } })}
                />
              </Field>
            </>
          )}
          <p className="flow-action-summary">{actionSummary(selected.action)}</p>
          <button
            type="button"
            className="button button--secondary button--small"
            onClick={() => {
              onChange(deleteStep(definition, selected.id));
              onSelectStep(null);
            }}
          >
            <TrashSimple size={15} /> Delete step
          </button>
        </div>
      ) : (
        <div className="flow-step-fields flow-step-fields--empty">
          <p>Select a step to edit its details, or add one to grow the flow.</p>
        </div>
      )}
    </div>
  );
}
