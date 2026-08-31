# PAM desktop layout contract

Status: descriptive of the shipped desktop app. PAM is a macOS desktop-only
Tauri 2 application; the supported window is a 2K-class desktop display or
larger, and no small-screen or mobile layout is part of the product surface.
PAM ships two independent theme families, each with light and dark variants.
Ventisquero provides Mist light and Bedrock dark, with Ice actions and
restrained copper events. Viña del Mar provides Dawn light and Night dark,
with violet actions and restrained coral events. Inter carries interface text;
Ventisquero uses Archivo for display and IBM Plex Mono for data, while Viña
uses Space Grotesk for display and JetBrains Mono for data. The six primary
views and Settings share one shell and one spatial grammar in all four
appearances. A visible surface must never combine tokens from both
families.

## Authorities

- View and content authority: `frontend/src/App.tsx` (the switch over the
  `ViewId` union: six primary views plus Settings), `frontend/src/selectors.ts`,
  and the typed daemon responses.
- Theme authority: `frontend/src/styles.css` owns both families' semantic
  token maps (the `modernization` layer), `frontend/src/theme.ts` owns family,
  variant, and density selection and persistence, and
  `frontend/public/assets/ventisquero-yelcho.png` plus
  `frontend/public/assets/vina-sunset.png` are the canonical source imagery.
- Shell geometry authority: the measured values in this document, implemented
  by `frontend/src/styles.css` and `frontend/src/layout.ts`.

PAM owns identity, visible product concepts, narrative hierarchy, and the
shell geometry below. Typed daemon responses own every displayed fact.

## Views and navigation

The app has exactly six primary views plus Settings, switched in `App.tsx`.
Every one of them is global: none renders a project selector, project name,
breadcrumb, or project-shaped empty state. The only surface that shows project
identity is the activity/caller log, and it does so as a per-row label.

1. **Overview** (`overview`) — daemon health, the 26-week activity heatmap,
   per-project usage, and one read-only local-model tile (identity plus a
   LOADED / ON DECK / UNREACHABLE pill) that navigates to Models. Global-first:
   it renders with zero projects.
2. **Models** (`models`) — the single home for the local model: load state and
   identity, the phase-aware load meter (elapsed time while the artifact is
   verified, a high-water bar while weights map in), Verify and Chat, the
   registered catalog with start/restart rows, and — reachable at all times,
   whatever is registered or loaded — the curated download picker and the
   manual GGUF import. Presets this Mac cannot run stay visible and disabled
   with the reason.
3. **Flows** (`flows`) — the flow catalog beside a source/visual editor with a
   review inspector and a run/history surface. The library is daemon-global,
   so this view never carries project identity; a run's own project appears
   only as a per-row label on that run.
4. **Skills** (`skills`) — inventory, canonical library, and audit as tabs in
   one panel.
5. **Access** (`access`) — daemon-scope capability grants, the observed
   boundary, the registered callers and the connectors (two-up on wide
   viewports, tabbed otherwise), and requests per caller with the GUI-caller
   registration recovery.
6. **Activity** (`activity`) — daemon health summary, the bounded recent
   activity feed, the latest outcome's evidence handles, and the daemon's
   debug console.

**Settings** (`settings`) — storage locations and log clearing.

The sidebar's primary nav lists the six in order, with a separator before
Activity; Settings is a gear item in the sidebar footer. Keyboard: ⌘1–⌘7
select views in the order above (⌘7 is Settings), ⌘K opens the command
palette, ⌘R refreshes, and Escape closes the active overlay.

## Shell geometry and density

The desktop root fills the native viewport and uses three columns:
`sidebar | 5px separator | minmax(0, 1fr) workspace`. The sidebar defaults to
248px and clamps to `180px..min(420px, 45vw)`. Its collapsed state is a 68px
icon rail; collapsing reduces the sidebar column to 68px while the separator
column stays. Sidebar width and collapsed state persist under the
PAM-specific `pam-sidebar-width` and `pam-sidebar-collapsed` storage keys;
invalid or stale widths are clamped on read, and storage failure never breaks
the live layout.

The 5px separator is a focusable vertical `role="separator"` with current,
minimum, and maximum ARIA values. Pointer drag uses pointer capture and
commits once on release. Keyboard behavior is exact: Left/Right changes
∓16px, Page Down/Page Up changes ∓64px, Home selects 180px, and End selects
the clamped maximum. Resize is unavailable while the sidebar is collapsed.

The workspace contains one inset canvas. At comfortable density the inset is
10px on the top, right, and bottom with the left edge immediately after the
separator; compact density — the default — substitutes a 6px inset without
changing the shell structure. The canvas has a 1px theme boundary, an 18px
radius, clipped outer overflow, a soft elevated shadow, and a fixed first row
for the toolbar; only the canvas body scrolls. The root and sidebar share the
theme chrome; the workspace is the only large floating surface. Desktop body
scrolling and horizontal shell scrolling are failures.

The canvas body is a vertical flex column: content blocks size to content and
the last block stretches so tall windows keep no dead tail. Content blocks
are `.panel` surfaces with a 12px radius, a 1px `--pam-line` boundary, and no
elevation; hierarchy comes from grouping and whitespace. `.panel` carries no
padding of its own — `.panel-title` and `.access-list article` supply the 20px
gutter, and loose content inside a panel is wrapped in `.panel-body`.

The six spacing tokens `--pam-space-050` through `--pam-space-300` carry
`4 / 8 / 12 / 16 / 20 / 24px` at comfortable density. Compact density is a real
alternate scale, not another name for comfortable: `3 / 6 / 9 / 12 / 14 / 17px`.
Density is a single `--pam-density` factor (1 comfortable, 0.8 compact);
component vertical metrics consume the tokens or the factor, while font sizes,
borders, radii, column widths, and the fixed shell geometry
(248/5/68/52/34/300/360) never scale, and interactive targets clamp at a 28px
floor. The density toggle lives in the toolbar theme menu beside family and
variant and persists the same way.

## Sidebar, toolbar, and canvas anatomy

The sidebar order is PAM identity (mark, name, and app version), primary
navigation, then the footer with daemon control/restart, utility buttons, and
the Settings gear. There is no project switcher. Active state, labels, and
counts must not depend on hover. Long labels truncate in the shell but expose
their full accessible name. The daemon control reflects the probed daemon
lifecycle and can pause/resume PAM.

The toolbar is the canvas's 52px top row; its icon controls are 34px square.
The left group holds the sidebar toggle and the breadcrumb; the right group
holds the command palette button, the bounded queue button (only while a queue
count is reported), refresh, and the theme/density menu. Theme family and
variant persist independently, restore before the first React render, and
apply at the document root so portalled menus and dialogs share the selected
tokens. On macOS the window uses an overlay
titlebar with a hidden native title, explicit drag regions, and a
traffic-light-safe sidebar inset.

Each view renders inside `.canvas`, the scrollable canvas body. Overview
opens with a `.project-overview` stat strip (one bordered strip with internal
dividers; six tiles, the last of which is the navigating model tile) followed
by the full-width heatmap panel; Activity opens with the same strip carrying
three tiles. At the supported width the strip is a single row of exactly as
many equal cells as it has tiles (`grid-auto-flow: column`), never an auto-fit
reflow with an orphan second row; below the wide breakpoint it degrades to
auto-fit `minmax(176px, 1fr)` and may wrap. The heatmap covers the trailing 52 weeks, the
window HEATMAP_WEEKS in `OverviewView.tsx` requests from `daemon.stats` and
sizes the grid from; widening it past 52 weeks means raising the daemon's
MAX_STATS_DAYS (366) and the store's MAX_ACTIVITY_DAYS (400) first. A month
axis sits above the grid and Mon/Wed/Fri labels beside it, both sharing the
cell tracks so they stay aligned; the intensity key sits in the panel header,
outside the plot. Columns stretch from an 11px floor, so a year fills the panel
on a 2K window and stays legible on a narrow one, and each cell takes its
height from its own width to stay square at any column width.

The two workspace-internal panes are fixed-width, not percentage-elastic: a
percentage track grows with the window, so the same catalog reflowed at every
width. The Flows workspace is a two-column grid — a **300px** catalog column
and the editor — and the visual editor splits into canvas plus a **360px**
step inspector. 300px holds a catalog row (27px icon plus a 10px mono flow id)
without ellipsis; 360px holds the widest step field plus its gutters. Both
keep their existing narrow collapses: at 960px and below the workspace stacks
to a single column, and at 700px and below the graph canvas is hidden and the
step inspector takes the row. The flow review and diff inspector panels cap
at 130px and scroll; the run and history panels below them cap at 168px and
scroll, and at 960px and below the run header and each history row collapse to
one column.

## Drawers, dialogs, and overlays

Application overlays form a single stack (`frontend/src/overlays.ts`): the
project menu, command palette, queue drawer, approval drawer, evidence
drawer, and model-chat drawer. Only the most recently opened overlay is
active; earlier overlays and the whole shell become inert and `aria-hidden`,
and layering is deterministic. Drawers and dialogs trap Tab/Shift+Tab, Escape
closes only the active overlay, and focus returns to the exact opener.
Loading, empty, failure, truncated, and retry states render inside the same
bounded surface without changing its geometry. Toasts are status
announcements, never the only evidence of success or failure.

Drawers (queue, approval, evidence, model chat) are right-side panels no
wider than `min(520px, calc(100vw - 28px))`, full viewport height, inset with
a 24px left radius, with a fixed header and an independently scrollable body.
The command palette is a centered modal of `min(640px, calc(100vw - 40px))`,
top-aligned at `min(14vh, 120px)`, viewport-bounded in height. The overlay
scrim uses the theme's `--pam-overlay` token with a backdrop blur.

## Wide-viewport (2K) rules

The layout assumes a 2K-class desktop window; nothing in the shipping UI
requires a narrow viewport. 1360px is the single wide breakpoint: one
`@media (min-width: 1360px)` block in `styles.css` owns every wide rule, and
`WIDE_VIEWPORT_QUERY` in `frontend/src/useMediaQuery.ts` is its JavaScript
half — the views that swap tabs for panels (Skills, Access) read that constant
rather than declaring a breakpoint of their own.

At 1360px and above, wide viewports use a list+detail grammar instead of
stacking: Skills, and Access for its callers/connectors pair, place their
paired panels in a two-column `wide-split` grid
(`minmax(360px, 0.9fr) | minmax(420px, 1.1fr)`). Every row list that owns the
full canvas width flows two-up (`repeat(2, minmax(380px, 1fr))`, odd rows
carrying the divider, empty and status rows spanning both columns with
`grid-column: 1 / -1`): the Access daemon-scope grants, the Access
authorized-capability list, the Access requests-per-caller list, the Activity
feed, and the Overview project-usage list. A list that already sits inside a `wide-split` column — registered
callers, connectors, skill inventory — keeps one row per item, and the debug
console keeps its log lines single-file. The Overview and Activity stat strips
pin to one row here. This widens content only; the shell geometry is
unchanged.

Narrow-viewport media queries remain in `styles.css` only as graceful
degradation for an undersized window; they are not product targets and must
not grow new surface area.

## Accessibility rules

Every interactive element has a visible `:focus-visible` treatment using the
selected theme's focus token; clipped controls use a 2px inward outline.
Focus order follows visual order, current navigation uses `aria-current`,
toggle state is named, and status changes use appropriate polite status or
assertive alert regions. Reduced-motion `always` and the system
`prefers-reduced-motion` path reduce transitions and animations to 0.01ms and
disable smooth scrolling; an explicit `never` setting may opt out of the
media query. In forced colors, focus uses `Highlight`, disabled controls use
`GrayText`, structural boundaries use system colors, and meaning is never
communicated by color alone.

## Truth and content constraints

The approved imagery is a composition reference, not permission to invent
live facts. Production UI obeys these rules:

- Daemon identity may show only the returned daemon version and an accurate
  lifecycle phrase. Never display a mock model name, model memory,
  token/latency telemetry, or model availability that the protocol did not
  report.
- Project branch, request status, evidence handles, queue counts, approvals,
  and access facts come only from the typed active-project response. Do not
  infer grants from configuration or turn unavailable data into optimistic
  copy.
- Access distinguishes observed, policy-gated, disabled, and unavailable
  facts. Approval copy states the exact bounded effect, project,
  capability/policy, expiry, and opaque handle supplied by the protocol.
- Access keeps four skill facts separate. **Observed** means the bounded
  project inventory detected an artifact; it does not enable or claim
  ownership. **Enabled** is the returned exact entry/version/agent selection
  for the active project. **Managed** means PAM recorded ownership only after
  verified publication. **Drift** is unknown until an explicit read-only
  inspection and then renders only the returned closed state (`clean`,
  `missing`, `modified`, or a typed conflict). The UI never derives one fact
  from another.
- Library additions and target mutations are explicit forms or buttons. Every
  request carries a fresh operation UUID plus the active opaque project
  handle and generation. A preview is applyable only for the same entry,
  version, and agent; selection or project changes discard it. Mutation
  success appears only after an exact fenced result and a second verified
  library load. Source bytes, local source paths, and Git URLs exist only in
  the explicit install form and outbound request, are cleared after verified
  success, and never return in the library snapshot or appear as provenance.
  Destination roots and internal project keys never cross the desktop
  response boundary.
- Solved, unresolved, blocked, cancelled, loading, offline,
  credential-recovery, and stale-project states retain their truthful
  terminal meaning. A decorative or fixture success must never leak into
  production.

## Verification

The deterministic Vitest suite owns the typed state matrix and the Playwright
suite in `frontend/e2e/pam.spec.ts` owns the spatial and interaction
contract: shell geometry, theme appearances, drawers/dialogs, keyboard
navigation, reduced motion, and forced colors. Layout changes must keep both
suites green; a visual change that invalidates a screenshot baseline requires
a reviewed baseline re-record in the same change.
