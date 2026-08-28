import {
  ArrowClockwise,
  FileText,
  FloppyDisk,
  GitBranch,
  ListChecks,
  SidebarSimple,
  WarningCircle,
} from "@phosphor-icons/react";
import { useCallback, useEffect, useId, useMemo, useRef, useState } from "react";
import { Tab, TabList, TabPanel, Tabs } from "react-aria-components";
import { sameFence, withDaemonOperation, withOperation } from "../bridge";
import { PanelError, PanelLoading } from "../components/PanelState";
import type {
  CommandFence,
  FlowDefinitionJson,
  FlowDocumentDataDto,
  FlowReviewDataDto,
  FlowWorkspaceDataDto,
  PamBridge,
} from "../domain";
import { MAX_FLOW_SOURCE } from "../domain";
import { presentError } from "../state";
import { VisualEditor } from "./flow/VisualEditor";

function sameAuthority(left: CommandFence, right: CommandFence): boolean {
  return left.projectHandle === right.projectHandle && left.generation === right.generation;
}

const MAX_HISTORY = 50;
const HISTORY_COALESCE_MS = 800;

interface DefinitionHistory {
  past: FlowDefinitionJson[];
  future: FlowDefinitionJson[];
  lastEditMs: number;
}

const freshHistory = (): DefinitionHistory => ({ past: [], future: [], lastEditMs: 0 });

export interface FlowsViewProps {
  bridge: PamBridge;
  fence: CommandFence | null;
  onError: (message: string) => void;
  onToast: (message: string) => void;
}

// Flow definitions are a daemon-global library, so this view always speaks
// the daemon authority and carries no project identity. The `fence` prop is
// only a refresh signal: its generation rotates on ⌘R, activate, and daemon
// lifecycle changes.
export function FlowsView({ bridge, fence: fenceProp, onError, onToast }: FlowsViewProps) {
  const fence = useMemo(() => withDaemonOperation(), []);
  const [workspace, setWorkspace] = useState<FlowWorkspaceDataDto | null>(null);
  const [selected, setSelected] = useState<FlowDocumentDataDto | null>(null);
  const [draft, setDraft] = useState("");
  const [mode, setMode] = useState<"visual" | "source">("source");
  const [definition, setDefinition] = useState<FlowDefinitionJson | null>(null);
  const [selectedStepId, setSelectedStepId] = useState<string | null>(null);
  const [modeNotice, setModeNotice] = useState<string | null>(null);
  const [review, setReview] = useState<FlowReviewDataDto | null>(null);
  const [reviewedSource, setReviewedSource] = useState<string | null>(null);
  const [reviewPanel, setReviewPanel] = useState<"dry-run" | "diff">("dry-run");
  const [loadError, setLoadError] = useState<string | null>(null);
  const [validationError, setValidationError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [catalogHidden, setCatalogHidden] = useState(false);
  const validationErrorId = useId();
  const fenceRef = useRef(fence);
  const requestSequence = useRef(0);
  const historyRef = useRef<DefinitionHistory>(freshHistory());
  const definitionRef = useRef<FlowDefinitionJson | null>(null);
  fenceRef.current = fence;
  definitionRef.current = definition;

  const isCurrentRequest = useCallback((sequence: number, requestFence: CommandFence) => (
    sequence === requestSequence.current && sameAuthority(requestFence, fenceRef.current)
  ), []);

  const clearReview = () => {
    setReview(null);
    setReviewedSource(null);
    setValidationError(null);
  };

  const resetEditorModes = () => {
    setMode("source");
    setDefinition(null);
    setSelectedStepId(null);
    setModeNotice(null);
    historyRef.current = freshHistory();
  };

  const load = useCallback(async () => {
    const sequence = ++requestSequence.current;
    const requestFence = withOperation(fenceRef.current);
    setBusy(true);
    setLoadError(null);
    setWorkspace(null);
    setSelected(null);
    setDraft("");
    setMode("source");
    setDefinition(null);
    setSelectedStepId(null);
    setModeNotice(null);
    historyRef.current = freshHistory();
    setReview(null);
    setReviewedSource(null);
    setReviewPanel("dry-run");
    setValidationError(null);
    try {
      const response = await bridge.loadFlowWorkspace(requestFence);
      if (!isCurrentRequest(sequence, requestFence)) return;
      if (!sameFence(requestFence, response.fence)) {
        setLoadError("The flow workspace response did not match the daemon request. Retry flows.");
        return;
      }
      setWorkspace(response.data);
      if (response.data.migrated.length > 0) {
        onToast(`Migrated ${response.data.migrated.length === 1 ? "1 flow" : `${response.data.migrated.length} flows`} into the shared library`);
      }
    } catch (error) {
      if (isCurrentRequest(sequence, requestFence)) setLoadError(presentError(error));
    } finally {
      if (isCurrentRequest(sequence, requestFence)) setBusy(false);
    }
  }, [bridge, isCurrentRequest]);

  // A generation rotation (⌘R, activate, daemon lifecycle, project switch)
  // re-fetches the catalog without touching the editor: draft, definition,
  // undo history, and review state all survive. Runs on its own sequence so
  // an in-flight open/validate/save is not invalidated by a background refresh.
  const reloadSequence = useRef(0);
  const reloadWorkspace = useCallback(async () => {
    const sequence = ++reloadSequence.current;
    const requestFence = withOperation(fenceRef.current);
    try {
      const response = await bridge.loadFlowWorkspace(requestFence);
      if (sequence !== reloadSequence.current || !sameAuthority(requestFence, fenceRef.current)) return;
      if (!sameFence(requestFence, response.fence)) return;
      setWorkspace(response.data);
    } catch {
      // A background refresh failure keeps the current workspace on screen;
      // explicit actions surface their own errors.
    }
  }, [bridge]);

  const loaded = useRef(false);
  useEffect(() => {
    // The library is global: the full load runs once, and every later
    // generation rotation is a soft reload that preserves the editor.
    if (loaded.current) void reloadWorkspace();
    else {
      loaded.current = true;
      void load();
    }
    return () => {
      requestSequence.current += 1;
      reloadSequence.current += 1;
    };
  }, [load, reloadWorkspace, fenceProp?.generation]);

  const open = async (flowHandle: string) => {
    const sequence = ++requestSequence.current;
    const requestFence = withOperation(fenceRef.current);
    setBusy(true);
    setSelected(null);
    setDraft("");
    resetEditorModes();
    setReview(null);
    setReviewedSource(null);
    setReviewPanel("dry-run");
    setValidationError(null);
    try {
      const response = await bridge.openFlow(requestFence, flowHandle);
      if (!isCurrentRequest(sequence, requestFence)) return;
      if (!sameFence(requestFence, response.fence)) {
        onError("The flow document response did not match the daemon request.");
        return;
      }
      setSelected(response.data);
      setDraft(response.data.source);
      // Visual is the default view of an opened flow; a source the converter
      // cannot follow simply opens in Source mode with a calm note.
      try {
        // A fresh operation: the daemon replay guard rejects a second command
        // that reuses the operation the open already spent.
        const graph = await bridge.flowGraph(withOperation(fenceRef.current), response.data.source);
        if (!isCurrentRequest(sequence, requestFence)) return;
        if (graph.status === "ok") {
          setDefinition(graph.definition);
          setMode("visual");
        } else {
          setModeNotice(graph.failure.detail);
        }
      } catch (error) {
        if (isCurrentRequest(sequence, requestFence)) setModeNotice(presentError(error));
      }
    } catch (error) {
      if (isCurrentRequest(sequence, requestFence)) onError(presentError(error));
    } finally {
      if (isCurrentRequest(sequence, requestFence)) setBusy(false);
    }
  };

  // Bounded undo history over definition snapshots; rapid edits coalesce so a
  // typing burst stays one undo stop.
  const editDefinition = (next: FlowDefinitionJson) => {
    const history = historyRef.current;
    const current = definitionRef.current;
    const now = Date.now();
    if (current) {
      if (history.past.length === 0 || now - history.lastEditMs > HISTORY_COALESCE_MS) {
        history.past.push(current);
        if (history.past.length > MAX_HISTORY) history.past.shift();
      }
      history.lastEditMs = now;
    }
    history.future = [];
    setDefinition(next);
    setModeNotice(null);
    clearReview();
  };

  const undo = useCallback(() => {
    const history = historyRef.current;
    const current = definitionRef.current;
    const previous = history.past.pop();
    if (!current || !previous) return;
    history.future.push(current);
    history.lastEditMs = 0;
    setDefinition(previous);
    setSelectedStepId((id) => (id && previous.steps.some((step) => step.id === id) ? id : null));
    setReview(null);
    setReviewedSource(null);
    setValidationError(null);
  }, []);

  const redo = useCallback(() => {
    const history = historyRef.current;
    const current = definitionRef.current;
    const next = history.future.pop();
    if (!current || !next) return;
    history.past.push(current);
    if (history.past.length > MAX_HISTORY) history.past.shift();
    history.lastEditMs = 0;
    setDefinition(next);
    setSelectedStepId((id) => (id && next.steps.some((step) => step.id === id) ? id : null));
    setReview(null);
    setReviewedSource(null);
    setValidationError(null);
  }, []);

  const visualActive = mode === "visual" && definition !== null && selected !== null;

  useEffect(() => {
    if (!visualActive) return;
    const onKey = (event: KeyboardEvent) => {
      if (!(event.metaKey || event.ctrlKey) || event.key.toLowerCase() !== "z") return;
      // Native text undo wins inside editable fields; the definition history
      // only answers when the focus is anywhere else.
      const target = event.target;
      if (target instanceof HTMLElement
        && (target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement || target.isContentEditable)) return;
      event.preventDefault();
      if (event.shiftKey) redo();
      else undo();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [visualActive, undo, redo]);

  const switchMode = async (next: "visual" | "source") => {
    if (!selected || next === mode || busy) return;
    setModeNotice(null);
    const sequence = ++requestSequence.current;
    const requestFence = withOperation(fenceRef.current);
    setBusy(true);
    try {
      if (next === "source") {
        const currentDefinition = definitionRef.current;
        // An untouched definition still matches the opened source, so the
        // original document text is kept as-is.
        if (!currentDefinition || historyRef.current.past.length === 0) {
          setMode("source");
          return;
        }
        const composed = await bridge.flowCompose(requestFence, currentDefinition);
        if (!isCurrentRequest(sequence, requestFence)) return;
        if (composed.status === "invalid") {
          setModeNotice(composed.failure.detail);
          return;
        }
        setDraft(composed.source);
        setMode("source");
      } else {
        const graph = await bridge.flowGraph(requestFence, draft);
        if (!isCurrentRequest(sequence, requestFence)) return;
        if (graph.status === "invalid") {
          setModeNotice(graph.failure.detail);
          return;
        }
        setDefinition(graph.definition);
        setSelectedStepId(null);
        historyRef.current = freshHistory();
        setMode("visual");
      }
    } catch (error) {
      if (isCurrentRequest(sequence, requestFence)) setModeNotice(presentError(error));
    } finally {
      if (isCurrentRequest(sequence, requestFence)) setBusy(false);
    }
  };

  const validate = async () => {
    if (!selected) return;
    const documentHandle = selected.handle;
    const sequence = ++requestSequence.current;
    const requestFence = withOperation(fenceRef.current);
    setBusy(true);
    setReview(null);
    setReviewedSource(null);
    setValidationError(null);
    try {
      let source = draft;
      // The visual state composes to source first, so validation and save
      // always run against the exact document that will be written.
      if (mode === "visual" && definitionRef.current) {
        // Composition spends its own operation; the validate below keeps
        // `requestFence` so its response fence check still matches.
        const composed = await bridge.flowCompose(withOperation(fenceRef.current), definitionRef.current);
        if (!isCurrentRequest(sequence, requestFence)) return;
        if (composed.status === "invalid") {
          setValidationError(composed.failure.detail);
          return;
        }
        source = composed.source;
        setDraft(source);
      }
      source = source.slice(0, MAX_FLOW_SOURCE);
      const response = await bridge.validateFlow(requestFence, documentHandle, source);
      if (!isCurrentRequest(sequence, requestFence)) return;
      if (!sameFence(requestFence, response.fence)) {
        setValidationError("The flow validation response did not match the daemon request. Retry validation.");
        return;
      }
      setReview(response.data);
      setReviewedSource(source);
      setReviewPanel("dry-run");
      onToast(response.data.dryRun.daemonDefinitionEligible ? "Flow document is valid and daemon-eligible" : "Flow document is valid with authority limits");
    } catch (error) {
      if (isCurrentRequest(sequence, requestFence)) setValidationError(presentError(error));
    } finally {
      if (isCurrentRequest(sequence, requestFence)) setBusy(false);
    }
  };

  const save = async () => {
    const source = draft.slice(0, MAX_FLOW_SOURCE);
    if (!selected || !review || reviewedSource !== source || !review.diff.changed) return;
    const normalizedSource = review.normalizedToml.slice(0, MAX_FLOW_SOURCE);
    const documentHandle = selected.handle;
    const sequence = ++requestSequence.current;
    const requestFence = withOperation(fenceRef.current);
    setBusy(true);
    try {
      const response = await bridge.saveFlow(requestFence, documentHandle, normalizedSource);
      if (!isCurrentRequest(sequence, requestFence)) return;
      if (!sameFence(requestFence, response.fence)) {
        onError("The flow save response did not match the daemon request.");
        return;
      }
      if (response.data.document !== documentHandle) {
        onError("The flow save response did not match the reviewed document. Reload flows before saving again.");
        return;
      }
      setSelected((current) => current?.handle === documentHandle ? { ...current, identity: response.data.identity, source: normalizedSource } : current);
      setDraft(normalizedSource);
      setWorkspace((current) => current && ({ ...current, definitions: current.definitions.map((definition) => definition.identity.id === response.data.identity.id ? { ...definition, identity: response.data.identity } : definition) }));
      setReview(null);
      setReviewedSource(null);
      onToast(response.data.durabilityConfirmed && response.data.cleanupComplete ? "Flow saved durably in the shared flow library" : "Flow saved; durability confirmation is incomplete");
    } catch (error) {
      if (isCurrentRequest(sequence, requestFence)) onError(presentError(error));
    } finally {
      if (isCurrentRequest(sequence, requestFence)) setBusy(false);
    }
  };

  const acceptedReview = reviewedSource === draft ? review : null;

  return (
    <main className="canvas" id="main-content">
      <header className="project-header compact"><div><h1>Flows</h1><p>One shared flow library — defined once, run from wherever you invoke PAM.</p></div></header>
      {loadError && !workspace ? (
        <PanelError
          as="section"
          className="panel loading-panel is-error"
          icon={<WarningCircle size={25} />}
          title="Flow workspace unavailable"
          action={<button type="button" className="button button--secondary" onClick={() => void load()}><ArrowClockwise size={18} /> Retry flows</button>}
        >{loadError}</PanelError>
      ) : !workspace ? (
        <PanelLoading as="section" className="panel loading-panel" icon={<ArrowClockwise className={busy ? "is-spinning" : ""} size={25} />}>Loading bounded flow workspace…</PanelLoading>
      ) : (
        <section className={`flow-workspace ${catalogHidden ? "is-catalog-hidden" : ""}`} aria-label="Flow workspace">
          <aside className="flow-catalog" hidden={catalogHidden}>
            <div className="panel-title"><div><span className="eyebrow">Flow library</span><h2>Definitions</h2></div><FileText size={20} /></div>
            <div className="flow-list">
              {workspace.definitions.map((flow) => (
                <button type="button" className={selected?.identity?.id === flow.identity.id ? "is-active" : ""} aria-pressed={selected?.identity?.id === flow.identity.id} key={flow.handle} onClick={() => void open(flow.handle)}>
                  <GitBranch size={18} />
                  <span><strong title={flow.identity.id}>{flow.identity.id}</strong><small title={flow.identity.fileName}>{flow.identity.fileName}</small></span>
                  <span className="state-pill state-pill--ready">r{flow.identity.revision}</span>
                </button>
              ))}
            </div>
          </aside>
          <section className="flow-editor">
            <div className="panel-title editor-title">
              <div className="editor-title-lead">
                <button
                  type="button"
                  className="flow-catalog-toggle"
                  aria-pressed={catalogHidden}
                  aria-label={catalogHidden ? "Show flow catalog" : "Hide flow catalog"}
                  title={catalogHidden ? "Show catalog" : "Hide catalog for more canvas"}
                  onClick={() => setCatalogHidden((hidden) => !hidden)}
                >
                  <SidebarSimple size={17} weight="bold" />
                </button>
                <div><span className="eyebrow">Editing</span><h2>{selected?.identity?.fileName ?? "Select a definition"}</h2></div>
              </div>
              <div>
                {selected && (
                  <div className="flow-mode-toggle" role="group" aria-label="Editor mode">
                    <button type="button" aria-pressed={mode === "visual"} disabled={busy} onClick={() => void switchMode("visual")}>Visual</button>
                    <button type="button" aria-pressed={mode === "source"} disabled={busy} onClick={() => void switchMode("source")}>Source</button>
                  </div>
                )}
                <button type="button" className="button button--secondary button--small" disabled={busy || !selected} onClick={() => void validate()}><ListChecks size={17} /> Validate</button>
                <button type="button" className="button button--primary button--small" disabled={busy || !acceptedReview?.diff.changed} onClick={() => void save()}><FloppyDisk size={17} /> Save</button>
              </div>
            </div>
            {visualActive && definition ? (
              <VisualEditor
                definition={definition}
                selectedStepId={selectedStepId}
                onSelectStep={setSelectedStepId}
                onChange={editDefinition}
              />
            ) : (
              <textarea
                aria-label="Flow TOML source"
                aria-invalid={validationError ? true : undefined}
                aria-describedby={validationError ? validationErrorId : undefined}
                spellCheck={false}
                value={draft}
                maxLength={MAX_FLOW_SOURCE}
                disabled={!selected}
                onChange={(event) => {
                  requestSequence.current += 1;
                  setBusy(false);
                  setDraft(event.target.value);
                  setReview(null);
                  setReviewedSource(null);
                  setValidationError(null);
                  setModeNotice(null);
                }}
              />
            )}
            {modeNotice && <div className="mode-notice" role="status"><p>{modeNotice}</p></div>}
            {validationError && <div className="validation-errors" id={validationErrorId} role="alert"><p><WarningCircle size={16} aria-hidden="true" />{validationError}</p></div>}
            <div className="editor-status" role="status">
              <span>{visualActive && definition ? `${definition.steps.length} steps` : `${draft.length.toLocaleString()} / ${MAX_FLOW_SOURCE.toLocaleString()} characters`}</span>
              {acceptedReview && <span className={acceptedReview.dryRun.daemonDefinitionEligible ? "is-valid" : "is-invalid"}>{acceptedReview.dryRun.daemonDefinitionEligible ? `Valid · ${acceptedReview.dryRun.steps.length} dry-run steps` : "Valid · outside daemon authority"}</span>}
            </div>
            {acceptedReview && (
              <Tabs
                className="flow-inspector"
                selectedKey={reviewPanel}
                onSelectionChange={(key) => {
                  if (key === "dry-run" || key === "diff") setReviewPanel(key);
                }}
              >
                <TabList className="flow-inspector-tabs" aria-label="Flow review">
                  <Tab id="dry-run" className="flow-inspector-tab">Dry run</Tab>
                  <Tab id="diff" className="flow-inspector-tab">Version diff{acceptedReview.diff.changed ? " · changed" : " · clean"}</Tab>
                </TabList>
                <TabPanel id="dry-run" className="flow-review">
                  {acceptedReview.dryRun.steps.slice(0, 5).map((step) => <p key={`${step.index}:${step.id}`}><span>{step.index + 1}</span><strong>{step.id}</strong><small>{step.semanticRole} · {step.daemonAuthority}</small></p>)}
                </TabPanel>
                <TabPanel id="diff" className="flow-diff">
                  {acceptedReview.diff.lines.length === 0
                    ? <p>No versioned source changes were reported.</p>
                    : acceptedReview.diff.lines.map((line, index) => <pre className={`is-${line.kind}`} key={`${index}:${line.kind}`}>{line.kind === "added" ? "+" : line.kind === "removed" ? "−" : " "} {line.text}</pre>)}
                  {acceptedReview.diff.truncated && <p className="bounded-note">The bounded version diff was truncated.</p>}
                </TabPanel>
              </Tabs>
            )}
          </section>
        </section>
      )}
    </main>
  );
}
