import { ArrowClockwise, WarningCircle, X } from "@phosphor-icons/react";
import { Dialog, Modal, ModalOverlay, VisuallyHidden } from "react-aria-components";
import {
  type CSSProperties,
  type ReactNode,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
} from "react";
import { withDaemonOperation } from "../bridge";
import type {
  ApprovalDecision,
  ChatMessageDto,
  EvidenceDataDto,
  ModelUsageDto,
  PamBridge,
} from "../domain";
import { CHAT_CONTEXT_TOKEN_BUDGET, CHAT_MAX_OUTPUT_TOKENS, MAX_CHAT_MESSAGES, MAX_EVIDENCE_TEXT } from "../domain";
import type { ControlCenterView } from "../selectors";
import { presentError } from "../state";
import { PanelEmpty, PanelError, PanelLoading } from "./PanelState";

export interface DrawerProps {
  title: string;
  eyebrow: string;
  onClose: () => void;
  children: ReactNode;
  active?: boolean;
  returnFocusTarget?: HTMLElement | null;
}

export function Drawer({ title, eyebrow, onClose, children, active = true, returnFocusTarget }: DrawerProps) {
  const titleId = useId();
  const descriptionId = useId();
  const returnFocus = useRef<HTMLElement | null>(returnFocusTarget ?? (active && document.activeElement instanceof HTMLElement ? document.activeElement : null));
  const activeRef = useRef(active);
  activeRef.current = active;
  useEffect(() => {
    return () => {
      const target = returnFocus.current;
      if (activeRef.current && target?.isConnected) {
        target.focus();
        requestAnimationFrame(() => {
          if (!target.isConnected) return;
          target.focus();
          requestAnimationFrame(() => {
            if (target.isConnected) target.focus();
          });
        });
      }
    };
  }, []);
  // Radix dismissed on a document-level Escape; react-aria listens on the
  // overlay itself, so an Escape pressed after focus fell to <body> (a button
  // that disabled itself mid-flight) would be lost. React stops propagation
  // when it handled the key, so this only fires for the orphaned case.
  useEffect(() => {
    if (!active) return;
    const dismiss = (event: KeyboardEvent) => {
      if (event.key === "Escape" && !event.defaultPrevented) onClose();
    };
    document.addEventListener("keydown", dismiss);
    return () => document.removeEventListener("keydown", dismiss);
  }, [active, onClose]);
  const content = (
    <>
      <VisuallyHidden><p id={descriptionId}>{eyebrow}</p></VisuallyHidden>
      <header>
        <div><span className="eyebrow">{eyebrow}</span><h2 id={titleId}>{title}</h2></div>
        <button autoFocus={active} className="drawer-close" type="button" aria-label={`Close ${title}`} onClick={onClose}><X size={21} weight="bold" /></button>
      </header>
      {/* Native overflow, not a scroll-area widget: react-aria-components has
          no ScrollArea, and the browser already does this. */}
      <div className="drawer-body">
        <div className="drawer-scroll-viewport">{children}</div>
      </div>
    </>
  );

  if (!active) {
    return (
      <div className="application-overlay application-overlay--drawer" data-application-overlay-layer="underlay" aria-hidden inert>
        <div className="drawer-modal">
          <div className="drawer" role="dialog" aria-labelledby={titleId} aria-describedby={descriptionId}>{content}</div>
        </div>
      </div>
    );
  }

  return (
    <ModalOverlay
      className="application-overlay application-overlay--drawer"
      isOpen
      isDismissable
      onOpenChange={(isOpen) => { if (!isOpen) onClose(); }}
    >
      <Modal className="drawer-modal" data-application-overlay-layer="active">
        <Dialog className="drawer" aria-labelledby={titleId} aria-describedby={descriptionId}>{content}</Dialog>
      </Modal>
    </ModalOverlay>
  );
}

export interface EvidenceDrawerProps {
  document: EvidenceDataDto | null;
  loading: boolean;
  error: string | null;
  onRetry?: () => void;
  onClose: () => void;
  active?: boolean;
}

export function EvidenceDrawer({ document, loading, error, onRetry, onClose, active = true }: EvidenceDrawerProps) {
  return (
    <Drawer title="Evidence" eyebrow="Exact bounded source" active={active} onClose={onClose}>
      {loading && <PanelLoading as="div" className="drawer-message" icon={<ArrowClockwise className="is-spinning" size={23} />}>Loading retained evidence…</PanelLoading>}
      {error && <PanelError as="div" className="drawer-message is-error" icon={<WarningCircle size={23} />} action={onRetry && <button type="button" className="button button--secondary" onClick={onRetry}><ArrowClockwise size={18} /> Retry evidence</button>}>{error}</PanelError>}
      {document && <article className="evidence-document"><code>{document.handle}</code><h3>{document.truth}</h3><p>{document.mediaType} · {document.sizeBytes.toLocaleString()} bytes · {document.digest}{document.truncated ? " · bounded preview" : ""}</p><pre>{(document.body ?? "This evidence has no text preview.").slice(0, MAX_EVIDENCE_TEXT)}</pre></article>}
    </Drawer>
  );
}

export interface QueueDrawerProps {
  data: ControlCenterView;
  onClose: () => void;
  active?: boolean;
  returnFocusTarget?: HTMLElement | null;
}

export function QueueDrawer({ data, onClose, active = true, returnFocusTarget }: QueueDrawerProps) {
  return (
    <Drawer title="Project queue" eyebrow={`${data.current.queue.length} retained request${data.current.queue.length === 1 ? "" : "s"}`} active={active} returnFocusTarget={returnFocusTarget} onClose={onClose}>
      <div className="queue-list">
        {data.current.queue.length === 0 ? <PanelEmpty>Nothing is queued for this project.</PanelEmpty> : data.current.queue.map((item, index) => (
          <article key={item.requestId}><span>{index + 1}</span><div><strong>{item.operationKind}</strong><code>{item.requestId}</code></div><span className={`state-pill state-pill--${item.state}`}>{item.state}</span></article>
        ))}
        {data.current.queueTruncated && <p className="bounded-note">Only the bounded queue window is shown.</p>}
      </div>
    </Drawer>
  );
}

interface ChatTurn {
  role: "user" | "assistant";
  content: string;
  usage?: ModelUsageDto;
}

// The same ~4-chars-per-token heuristic the fixtures use; good enough to
// keep the request inside the model's context window.
const estimateTokens = (text: string) => Math.max(1, Math.ceil(text.length / 4));

// Keep the newest turns whose estimated tokens (plus the reserved reply
// budget) fit the context window; the just-sent message is always kept.
function trimToContextBudget(turns: ChatTurn[]): ChatTurn[] {
  let tokens = CHAT_MAX_OUTPUT_TOKENS;
  const kept: ChatTurn[] = [];
  for (let index = turns.length - 1; index >= 0; index -= 1) {
    const turn = turns[index];
    if (kept.length > 0 && tokens + estimateTokens(turn.content) > CHAT_CONTEXT_TOKEN_BUDGET) break;
    tokens += estimateTokens(turn.content);
    kept.unshift(turn);
  }
  return kept;
}

export interface ModelChatDrawerProps {
  modelId: string;
  bridge: PamBridge;
  onClose: () => void;
  active?: boolean;
  returnFocusTarget?: HTMLElement | null;
}

export function ModelChatDrawer({ modelId, bridge, onClose, active = true, returnFocusTarget }: ModelChatDrawerProps) {
  const [turns, setTurns] = useState<ChatTurn[]>([]);
  const [input, setInput] = useState("");
  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState<string | null>(null);
  const requestSequence = useRef(0);
  const transcriptRef = useRef<HTMLDivElement>(null);

  // Closing the drawer discards any in-flight reply via the sequence guard.
  useEffect(() => () => { requestSequence.current += 1; }, []);

  useEffect(() => {
    const viewport = transcriptRef.current;
    if (viewport) viewport.scrollTop = viewport.scrollHeight;
  }, [turns, busy, note]);

  const send = async () => {
    const content = input.trim();
    if (!content || busy) return;
    if (turns.length >= MAX_CHAT_MESSAGES) {
      setNote("This transcript has reached its bounded window. Clear it to keep chatting.");
      return;
    }
    const nextTurns: ChatTurn[] = [...turns, { role: "user", content }];
    // The request carries the token-budgeted window, not the raw transcript:
    // long old messages drop first instead of counting toward a message cap.
    const messages: ChatMessageDto[] = trimToContextBudget(nextTurns)
      .map((turn) => ({ role: turn.role, content: turn.content }));
    setTurns(nextTurns);
    setInput("");
    setNote(null);
    setBusy(true);
    const sequence = ++requestSequence.current;
    try {
      // Model chat is daemon-global: always the daemon authority.
      const response = await bridge.modelInfer(withDaemonOperation(), modelId, messages, CHAT_MAX_OUTPUT_TOKENS);
      if (sequence !== requestSequence.current) return;
      if (response.status === "ok") {
        setTurns([...nextTurns, { role: "assistant", content: response.text, usage: response.usage }]);
      } else {
        setNote([response.failure.detail, response.failure.recovery].filter(Boolean).join(" "));
      }
    } catch (error) {
      if (sequence === requestSequence.current) setNote(presentError(error));
    } finally {
      if (sequence === requestSequence.current) setBusy(false);
    }
  };

  const clear = () => {
    requestSequence.current += 1;
    setTurns([]);
    setNote(null);
    setBusy(false);
  };

  return (
    <Drawer title="Model chat" eyebrow={modelId} active={active} returnFocusTarget={returnFocusTarget} onClose={onClose}>
      <div className="model-chat">
        <p className="bounded-note">Nothing here is kept — close the drawer and the transcript drifts away.</p>
        <div className="chat-transcript" ref={transcriptRef}>
          {turns.length === 0 && !busy && !note && (
            <PanelEmpty>Say hello to the local model. Replies stay in this drawer.</PanelEmpty>
          )}
          {turns.map((turn, index) => (
            <article className={`chat-bubble chat-bubble--${turn.role}`} key={index}>
              <p>{turn.content}</p>
              {turn.usage && (
                <small className="chat-usage">in {turn.usage.inputTokens} · out {turn.usage.emittedOutputTokens} tokens</small>
              )}
            </article>
          ))}
          {busy && (
            <p className="chat-waiting" role="status" aria-live="polite">
              <ArrowClockwise className="is-spinning" size={17} /> The model is thinking — longer replies can take a couple of minutes.
            </p>
          )}
          {note && <p className="chat-note" role="status">{note}</p>}
        </div>
        <form className="chat-composer" onSubmit={(event) => { event.preventDefault(); void send(); }}>
          <textarea
            className="chat-input"
            aria-label="Message the model"
            placeholder="Ask the local model… (⌘Enter to send)"
            rows={3}
            value={input}
            onChange={(event) => setInput(event.currentTarget.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
                event.preventDefault();
                void send();
              }
            }}
          />
          <div className="chat-actions">
            <button type="button" className="button button--secondary button--small" disabled={turns.length === 0 && !note} onClick={clear}>Clear</button>
            <button type="submit" className="button button--primary button--small" disabled={busy || input.trim().length === 0}>Send</button>
          </div>
        </form>
      </div>
    </Drawer>
  );
}

export interface ApprovalDrawerProps {
  data: ControlCenterView;
  busy: boolean;
  error: string | null;
  onDecision: (decision: ApprovalDecision) => void;
  onClose: () => void;
  active?: boolean;
}

export function ApprovalDrawer({ data, busy, error, onDecision, onClose, active = true }: ApprovalDrawerProps) {
  const approval = data.current.approval;
  if (!approval) return null;
  return (
    <Drawer title="Approval required" eyebrow="Bounded project effect" active={active} onClose={onClose}>
      <article className="approval-card" aria-busy={busy}><WarningCircle size={28} /><h3>{approval.title}</h3><p>{approval.reason}</p>{error && <p className="approval-error" role="alert">{error}</p>}<dl><div><dt>Effect</dt><dd>{approval.effect}</dd></div><div><dt>Project</dt><dd>{approval.projectName}</dd></div><div><dt>Policy / capability</dt><dd>{approval.policyCapability}</dd></div><div><dt>Expires</dt><dd>{approval.expiresAt}</dd></div><div><dt>Request handle</dt><dd><code>{approval.approvalHandle}</code></dd></div></dl>{busy && <p role="status">Applying the exact decision…</p>}<div><button type="button" className="button button--secondary" disabled={busy} onClick={() => onDecision("deny")}>Deny</button><button type="button" className="button button--primary" disabled={busy} onClick={() => onDecision("approve")}>Approve exact request</button></div></article>
    </Drawer>
  );
}

export interface CommandPaletteCommand {
  id: string;
  label: string;
  description: string;
  shortcut?: string;
}

export interface CommandPaletteProps {
  commands: CommandPaletteCommand[];
  active: boolean;
  returnFocusTarget?: HTMLElement | null;
  onAction: (id: string) => void;
  onClose: () => void;
}

export function CommandPalette({ commands, active, returnFocusTarget, onAction, onClose }: CommandPaletteProps) {
  const [query, setQuery] = useState("");
  const descriptionId = useId();
  const commandListRef = useRef<HTMLDivElement>(null);
  const returnFocus = useRef<HTMLElement | null>(returnFocusTarget ?? (active && document.activeElement instanceof HTMLElement ? document.activeElement : null));
  useEffect(() => () => {
    const target = returnFocus.current;
    if (!target?.isConnected) return;
    target.focus();
    requestAnimationFrame(() => { if (target.isConnected) target.focus(); });
  }, []);
  const filteredCommands = useMemo(() => {
    const needle = query.trim().toLocaleLowerCase();
    if (!needle) return commands;
    return commands.filter((command) => (
      [command.id, command.label, command.description, command.shortcut]
        .some((value) => value?.toLocaleLowerCase().includes(needle))
    ));
  }, [commands, query]);
  const run = (command: CommandPaletteCommand) => {
    onAction(command.id);
  };

  const moveOptionFocus = (current: HTMLElement | null, direction: 1 | -1) => {
    const options = Array.from(commandListRef.current?.querySelectorAll<HTMLElement>('[role="option"]') ?? []);
    if (options.length === 0) return;
    const index = current ? options.indexOf(current) : direction > 0 ? -1 : 0;
    options[(index + direction + options.length) % options.length]?.focus();
  };

  const dialog = (
    <Dialog className="command-dialog" aria-label="Command palette" aria-describedby={descriptionId}>
      <VisuallyHidden><p id={descriptionId}>Search and run a PAM command.</p></VisuallyHidden>
      <input
        autoFocus
        className="command-input"
        type="search"
        value={query}
        placeholder="Search commands…"
        aria-label="Search commands"
        onChange={(event) => setQuery(event.currentTarget.value)}
        onKeyDown={(event) => {
          if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
          event.preventDefault();
          moveOptionFocus(null, event.key === "ArrowDown" ? 1 : -1);
        }}
      />
      <div ref={commandListRef} className="command-options" role="listbox" aria-label="Commands">
        {filteredCommands.length === 0 ? <p className="command-empty">No matching commands.</p> : filteredCommands.map((command) => (
          <button
            className="command-option"
            type="button"
            role="option"
            aria-selected="false"
            key={command.id}
            onClick={() => run(command)}
            onKeyDown={(event) => {
              if (event.key === "ArrowDown" || event.key === "ArrowUp") {
                event.preventDefault();
                moveOptionFocus(event.currentTarget, event.key === "ArrowDown" ? 1 : -1);
              } else if (event.key === "Home" || event.key === "End") {
                event.preventDefault();
                const options = commandListRef.current?.querySelectorAll<HTMLElement>('[role="option"]');
                (event.key === "Home" ? options?.[0] : options?.[options.length - 1])?.focus();
              }
            }}
          >
            <span className="command-option-copy">
              <strong>{command.label}</strong>
              <small>{command.description}</small>
            </span>
            {command.shortcut && <kbd>{command.shortcut}</kbd>}
          </button>
        ))}
      </div>
    </Dialog>
  );

  return (
    <ModalOverlay
      className="application-overlay application-overlay--command"
      data-application-overlay-layer={active ? "active" : "underlay"}
      isOpen={active}
      isDismissable
      onOpenChange={(isOpen) => { if (!isOpen && active) onClose(); }}
    >
      <Modal className="command-modal">{dialog}</Modal>
    </ModalOverlay>
  );
}

export interface StartupShellProps {
  children: ReactNode;
}

type StartupShellStyle = CSSProperties & { "--sidebar-size": string };

const startupShellStyle: StartupShellStyle = { "--sidebar-size": "68px" };

export function StartupShell({ children }: StartupShellProps) {
  return (
    <div className="app-shell startup-shell" style={startupShellStyle}>
      <div className="atmosphere" aria-hidden="true" />
      <aside className="sidebar is-collapsed startup-sidebar" aria-label="PAM identity">
        <div className="brand" aria-label="PAM"><img src="/assets/pam-mark.png" alt="" /></div>
      </aside>
      <div className="resize-separator startup-separator" aria-hidden="true" />
      <section className="workspace startup-workspace">
        <header className="toolbar startup-toolbar"><div className="breadcrumb"><strong>PAM</strong></div></header>
        <main className="canvas startup-body" id="main-content">{children}</main>
      </section>
    </div>
  );
}

export type LoadingScreenProps = Record<string, never>;

export function LoadingScreen(_props: LoadingScreenProps) {
  return (
    <StartupShell>
      <PanelLoading as="section" className="empty-state state-card startup-state-card">
        <h1>PAM</h1>
        <p>Finding the last registered project…</p>
      </PanelLoading>
    </StartupShell>
  );
}

export interface RecoveryScreenProps {
  message: string;
  onRetry: () => void;
}

export function RecoveryScreen({ message, onRetry }: RecoveryScreenProps) {
  return (
    <StartupShell>
      <PanelError as="section" className="empty-state state-card startup-state-card is-attention">
        <WarningCircle size={38} />
        <h1>PAM needs a moment</h1>
        <p>{message}</p>
        <button type="button" className="button button--primary" onClick={onRetry}><ArrowClockwise size={18} /> Retry safely</button>
      </PanelError>
    </StartupShell>
  );
}
