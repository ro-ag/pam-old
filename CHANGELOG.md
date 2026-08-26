# Changelog

All notable changes to PAM are documented in this file. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and PAM adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.5.2] - 2026-08-26

### Added

- The manual GGUF import gained a native Browse… button and pre-import
  inspection: choosing, dropping, or typing a path reads the model's own
  header — no hashing, no waiting — and shows the file name, size, and the
  model's declared architecture and name, prefilling the vendor/name
  identity when the field is empty. Files below the recommended minimum
  size show the floor warning up front, pointing at the Advanced override.

## [0.5.1] - 2026-08-25

### Changed

- The curated model catalog is now Qwen3-Coder-30B-A3B-Instruct only — the
  smallest model PAM's flows were validated on — offered at three
  quantizations: Q4_K_S "minimum" (17.5 GB), Q4_K_M "balanced" (18.6 GB), and
  Q6_K "high fidelity" (25.1 GB), each pinned to an exact size and SHA-256.
  The previous 8B/14B presets sat below the validated quality bar and are
  gone.
- The manual GGUF import floor rose from 3.5 GB to 17 GB — just under the
  validated minimum quant. The "allow smaller models" override under
  Advanced remains.

### Added

- PAM now states its supported system minimum: local AI needs a machine with
  32 GB of memory or more. The host memory probe reports that minimum, and
  the model setup panel shows a calm notice on smaller machines, with RAM
  formatted in binary GiB the way machines are sold.

## [0.5.0] - 2026-08-25

### Added

- The Control Center shows the whole known fleet: a "Usage by project" panel
  lists every catalog project (and any project the daemon reports usage for)
  with event counts, relative usage bars, and last-activity dates.
  `daemon.stats` gained bounded per-project totals to feed it — a grouped
  scan over the audit trail within the same stats window.
- Local model setup now leads with curated presets: Qwen3 8B, Qwen3 14B
  (Apache-2.0), and Llama 3.1 8B Instruct, each pinned to an exact download
  size and SHA-256. Picking one shows its size, license, and a memory-fit
  hint against this machine; accepting the license starts a resumable,
  hash-verified download with live progress that registers the model on
  completion — the entire flow stays on the Control Center screen.
- Manual GGUF imports enforce a recommended minimum model size (3.5 GB):
  smaller files are refused with a clear explanation unless the new
  "allow smaller models" override under Advanced is checked.

### Changed

- The toolbar breadcrumb is project-free: it reads "Daemon observatory"
  everywhere, since the Control Center is a global surface. Project identity
  stays contextual in the project-shaped views.
- The manual import form tucks its license fields behind an
  "Advanced — license details" disclosure; presets prefill their license
  metadata, so the happy path never asks for SPDX input.

## [0.4.1] - 2026-08-25

### Added

- The flow visual editor grew real canvas chrome: dotted background, themed
  zoom in/out/fit controls, elbow (smoothstep) edges, pan-on-scroll with
  bounded zoom, a roomier auto-layout, and a catalog toggle that collapses
  the definitions column for a wider canvas. Styling follows both theme
  families through xyflow's theme variables.

### Fixed

- The window can actually be moved by its title bar again: the main-window
  capability never granted `core:window:allow-start-dragging`, so drag
  attempts were silently denied and fell through to text selection. The
  toolbar and the sidebar brand are now deep drag regions (buttons stay
  clickable), and double-click maximize is permitted alongside.
- Interface chrome text is no longer selectable; inputs, code, diffs, and
  console output stay copyable.
- At phone-narrow widths (≤600px) the stacked sidebar no longer overflows
  the viewport horizontally: the navigation row scrolls inside the bar
  instead of pushing the whole page wide.
- The Playwright visual contract was repaired for the project-free shell
  and every macOS snapshot regenerated; the suite is green again (24/24).

## [0.4.0] - 2026-08-25

### Added

- Fully GUI-owned local model import: the Control Center's empty model state
  is now an import form. Drop or point PAM at a downloaded GGUF, name it
  `vendor/name`, provide the license identifier, URL, and exact notice text,
  and accept — PAM computes the file digest, size, and notice digest itself,
  verifies the artifact through the shared import path, and registers it
  durably (new `model_import` desktop command). The existing
  restart-with-model action carries it into memory, so the whole setup
  happens without a terminal.
- Requests-per-caller panel on the Control Center: recent daemon requests
  aggregated per registered caller with active/revoked state — the complete
  project story on the main page. GUI caller registration recovery lives in
  this panel.
- Latest-run evidence panel on the Activity page: the retained evidence
  handles of the active project's latest terminal result open the bounded
  evidence drawer from the same page as the daemon feed.

### Changed

- The Control Center no longer offers project selection: the header project
  switcher and the project picker are gone from the main page. Project
  switching remains in the Access, Skills, and Flows headers.
- The Activity page's empty model card points at the Control Center import
  instead of naming a CLI command.

### Removed

- The project control-center row (durable timeline, handoff panel,
  approval-reopen chip, and outcome-brief copy). Project tracking belongs to
  the separate `ptrack` tool, not PAM; approval requests still open
  automatically and resurface on refresh.
- The copyable `pam model import` instruction block, replaced by the in-app
  import flow.

## [0.3.0] - 2026-08-24

### Added

- Model runtime panel on the Control Center launch view: shows the local
  model's state (loaded, on deck, none, unreachable) with its identity and
  size, verifies the runtime with a live inference round-trip reporting
  latency and returned tokens, opens the model chat in one click, and can
  restart PAM carrying a registered model (`start_daemon` now accepts a
  protocol-validated model key and passes `--model` to the daemon). When no
  model is registered the panel walks through the import steps with a
  copyable `pam model import` command.

### Changed

- The release pipeline reuses the packages built and verified by the tag
  commit's main CI run instead of rebuilding them; macOS signing and
  notarization happen only on release tags, and release validation refuses
  to run without a green CI run whose artifacts are still available. Weekly
  dependabot updates keep the pinned GitHub Actions current.
- The Control Center overview drops the redundant Projects count tile (the
  sidebar switcher already lists projects) and keeps a single watch-status
  tile per screen.

### Fixed

- The project hero no longer clips its status line at compact density or
  narrow widths.
- The activity heatmap is legible in dark themes: five measured intensity
  steps per theme and mode, with clear empty-versus-filled contrast in the
  grid and the legend.
- Stat tiles no longer truncate their values at production widths; long
  values expose the full text via tooltips.
- Views fill tall windows instead of leaving dead canvas below Access,
  Skills, Console, Connections, Activity, and the control-center project
  section.
- Flow catalog names at narrow widths expose their full value via tooltips.

## [0.2.0] - 2026-08-24

### Added

- Control Center now opens on a daemon-wide overview: stat tiles for
  projects, watch status, queue depth, events, active days, and streak, plus
  a 26-week daily-activity heatmap. The active project keeps its queue and
  outcomes in a second section, and without an active project a compact
  picker panel replaces the full-page placeholder.
- New `daemon.stats` capability serving per-day activity totals from a
  durable daily rollup (store migration 0014) that survives audit-event
  pruning.
- New Console view (⌘6) tailing the daemon's diagnostic log with severity
  filter, copy, and refresh, backed by the new `daemon.logs` capability.
- Daemon diagnostics: a bounded in-memory log ring plus a size-rotated
  `daemon.log` under the state directory records startup, warnings,
  failures, and the exit reason; the GUI-spawned daemon's stderr is captured
  to `logs/daemon-stderr.log` instead of being discarded.
- The sidebar brand shows the packaged app version.

### Changed

- The daemon survives per-request failures: request-handler panics and
  errors, undeliverable responses, lone transport receive failures, and
  failed queued operations are logged and no longer stop the daemon.
- GUI timeouts are classified through the daemon ownership lock: a stopped
  daemon now reports itself as paused with a one-click start affordance
  instead of a generic "request timed out" retry loop, and an unresponsive
  daemon reports its pid with console guidance.
- On macOS the daemon warms the native credential store at startup, keeping
  the first connector request inside its deadline while the security server
  evaluates the fresh binary's code signature.

### Fixed

- A scheduler teardown panic (`JoinHandle` polled after completion) that
  could mask the daemon's real shutdown result.

## [0.1.2] - 2026-08-24

### Added

- Public help and documentation site served from GitHub Pages at
  <https://ro-ag.github.io/pam/>: install matrix, first-run guide, flows,
  connectors, local model, and troubleshooting (including the macOS login
  keychain `errSecAuthFailed (-25293)` repair).
- Fresh desktop screenshots of the control center, flows, skills, access,
  connections, and activity views under `docs/assets/screenshots/`, captured
  from the fixture frontend in both theme families.

### Changed

- README restructured as a showcase: project mark, release/CI/docs badges,
  screenshot gallery, theme grid, install matrix, quickstart, approvals,
  local model, security model, and development sections.
- Root `node_modules/` build residue is no longer tracked and is ignored
  going forward.

## [0.1.1] - 2026-08-24

### Fixed

- The GUI caller registration banner now reports the helper's actual
  sanitized failure reason (for example an unavailable native credential
  store) instead of a generic "retry from this screen" message; typed
  desktop errors pass through to the surface unchanged while untyped
  failures keep the fixed copy.

## [0.1.0] - 2026-08-24

First tagged release: the complete local loop on macOS, with portable desktop
packages for Linux and Windows.

### Added

- One `pam` binary with client (default), `pam daemon`, and `pam gui` modes
  over an authenticated local IPC protocol.
- Durable per-project queues in SQLite with lease recovery, ordered event
  replay, and content-addressed evidence retention.
- Default-deny project policy with explicit-deny precedence, exact-effect
  one-time approvals, revocable callers, and secrets in the operating
  system's native credential store.
- Deterministic log compaction with byte-exact source evidence handles.
- Bounded embedded llama.cpp runtime (macOS Metal) with verified user-owned
  model registration, license consent, and fail-closed memory admission.
- Native Tauri control center: control-center landing, activity, callers,
  flows, connectors, skills, model status, and visual flow editor surfaces.
- Connector platform with seven read-only, policy-gated, audited connectors:
  GitHub Actions, Jenkins, SonarQube, Jira Data Center, Confluence Cloud,
  SharePoint (Microsoft Graph), and an AWS CLI passthrough with a curated
  read-only command allowlist and user-owned credentials.
- Global-first skill inventory with per-project assignment.
- Flow authoring and execution with conditions, approvals, and durable
  feedback; project continuity through the supported `ptrack` JSON CLI.
- Desktop packages for Linux amd64/arm64 (AppImage, DEB), Windows
  amd64/arm64 (NSIS), and signed, notarized macOS arm64 (app bundle, DMG),
  all built and published from CI on tag push.
