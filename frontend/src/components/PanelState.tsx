import type { ElementType, ReactNode } from "react";

export interface PanelStateProps {
  /** The message. Wrapped in a `<p>` whenever `icon`, `title` or `action` is set. */
  children?: ReactNode;
  /** The state's own class. Defaults to the one-line `panel-empty` row. */
  className?: string;
  /** The element to render. Defaults to the one-line `<p>`. */
  as?: ElementType;
  /** Leading glyph for the block form. */
  icon?: ReactNode;
  /** Bold headline above the message, block form only. */
  title?: ReactNode;
  /** Trailing control — a retry or a run button. */
  action?: ReactNode;
}

// One-line states render their message bare; the block states repeat the same
// icon / heading / message / action skeleton the panels already used.
function stateBody({ children, icon, title, action }: PanelStateProps): ReactNode {
  if (!icon && !title && !action) return children;
  return (
    <>
      {icon}
      {title ? <div><strong>{title}</strong><p>{children}</p></div> : <p>{children}</p>}
      {action}
    </>
  );
}

/**
 * A panel that is still fetching. Carries the full loading contract:
 * `role="status"`, a polite live region, and `aria-busy`.
 */
export function PanelLoading({ as: As = "p", className = "panel-empty", ...state }: PanelStateProps) {
  return <As className={className} role="status" aria-live="polite" aria-busy="true">{stateBody(state)}</As>;
}

/** A panel with nothing to show. Not announced: nothing happened. */
export function PanelEmpty({ as: As = "p", className = "panel-empty", ...state }: PanelStateProps) {
  return <As className={className}>{stateBody(state)}</As>;
}

/** A panel that could not load. Announced assertively. */
export function PanelError({ as: As = "p", className = "panel-empty", ...state }: PanelStateProps) {
  return <As className={className} role="alert">{stateBody(state)}</As>;
}
