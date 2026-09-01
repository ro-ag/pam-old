import { Check, WarningCircle } from "@phosphor-icons/react";
import type { ReactNode } from "react";

export interface ConfirmActionProps {
  /** The question, naming exactly what leaves. */
  question: ReactNode;
  /** The destructive verb. Repeated on the confirm button. */
  actionLabel: string;
  cancelLabel?: string;
  busy?: boolean;
  /** Extra arming condition beyond `busy` — a typed confirmation, say. */
  confirmDisabled?: boolean;
  onConfirm: () => void;
  onCancel: () => void;
  /** Anything between the question and the buttons: a preview, an input. */
  children?: ReactNode;
  /** Hook for tests and fixture walks; lands on the container. */
  testId?: string;
}

/**
 * The one way a destructive action confirms: a full-width block that owns its
 * question, its consequence, and a confirm styled like what it does. Never
 * rendered inside the flex row that triggered it — a confirmation is a
 * distinct moment, not another button in the pile.
 */
export function ConfirmAction({
  question,
  actionLabel,
  cancelLabel = "Keep",
  busy = false,
  confirmDisabled = false,
  onConfirm,
  onCancel,
  children,
  testId,
}: ConfirmActionProps) {
  return (
    <div className="confirm-action" role="group" data-testid={testId} aria-label={typeof question === "string" ? question : undefined}>
      <p className="confirm-action-question">{question}</p>
      {children}
      <div className="confirm-action-buttons">
        <button
          type="button"
          className="button button--danger button--small"
          disabled={busy || confirmDisabled}
          onClick={onConfirm}
        >
          {actionLabel}
        </button>
        <button
          type="button"
          className="button button--secondary button--small"
          disabled={busy}
          onClick={onCancel}
        >
          {cancelLabel}
        </button>
      </div>
    </div>
  );
}

export interface FailureNoticeProps {
  /** What went wrong, in the daemon's own words. */
  detail: ReactNode;
  /** How to get unstuck. Its own line, muted, under the detail. */
  recovery?: string | null;
}

/**
 * A refusal that looks like one. The container carries `role="alert"`, so the
 * detail and its recovery line read as one announcement while rendering as
 * two.
 */
export function FailureNotice({ detail, recovery }: FailureNoticeProps) {
  return (
    <div className="inline-message inline-message--error" role="alert">
      <WarningCircle size={17} aria-hidden="true" />
      <div>
        <p>{detail}</p>
        {recovery && <p className="inline-message-recovery">{recovery}</p>}
      </div>
    </div>
  );
}

/** An outcome worth keeping on screen, distinct from a note and a refusal. */
export function SuccessNotice({ children }: { children: ReactNode }) {
  return (
    <div className="inline-message inline-message--success" role="status">
      <Check size={17} aria-hidden="true" />
      <div><p>{children}</p></div>
    </div>
  );
}
