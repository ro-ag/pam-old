# Changelog

All notable changes to PAM are documented in this file. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and PAM adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
