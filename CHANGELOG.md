# Changelog

All notable changes to PAM are documented in this file. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and PAM adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.10.0] - 2026-09-01

### Added

- Models can be loaded and unloaded in a running daemon. Changing models no
  longer means restarting: `pam model load` admits a registered model and
  `pam model unload` drops it while Pam keeps serving. Loading over a loaded
  model swaps it, draining and joining the old worker - which unmaps the
  weights - before the replacement is built, so an inference in flight ends
  cancelled rather than reading a half-swapped service. In the Models view
  the restart dance is gone: rows carry Load, the loaded model carries
  Unload.
- The model the daemon starts with is remembered. A default model is
  persisted beside the models directory and pinned per row in the Models
  view; an explicit `--model` still wins, and a default naming a model that
  is no longer registered says so plainly instead of failing the start.
- Registered models can be removed. `model.unregister` deletes the registry
  row and writes a changed-truth audit line, with `pam model unregister` on
  the CLI and an Unregister row action in the Models view. The GGUF file on
  disk is never touched: unregistering and deleting bytes are different
  effects, and both surfaces say so.
- Registered weights can be checked without booting the daemon on them.
  `pam model verify` re-reads what the registry claims and names the
  specific failure - missing, resized, or digest drift - and the Models view
  reports it per row as Check weights.
- `pam model sweep` reports drift in both directions: registry rows whose
  file is gone, and GGUF files with no row pointing at them, with sizes and
  an honest models-directory total. In-flight downloads are not mistaken for
  orphans.
- Weights deletion is gated on provenance. `pam model delete-weights` serves
  only files Pam downloaded and still owns; a model imported in place is a
  user file Pam never owned, so it is refused with an explanation and
  offered Reveal in Finder instead. Deleting weights unregisters in the same
  operation.
- A model can be downloaded from a pasted HTTPS URL, not just the curated
  list. The pasted path is gated before anything is fetched: HTTPS only, no
  credentials in the URL, hosts resolving to loopback, link-local or private
  addresses refused, redirects confined to the pasted host, and the digest
  as the real gate - bytes that do not match are never registered.
- Reset is tiered rather than one button. `reset.access` clears grants and
  approvals, `reset.identity` revokes callers and purges their keychain
  entries, `reset.history` clears audit, evidence and flow runs, and
  `reset.registry` unregisters every model without touching weights. Each
  has a dry run that reports counts and bytes and changes nothing, and the
  grant for a preview cannot be spent on the wipe.
- `pam reset all` performs a factory reset: daemon stopped, `--yes`
  required, the audit event emitted before the wipe, and a receipt written
  outside the directory being emptied naming everything removed. Settings
  gains a danger zone that renders each tier's dry-run counts before its
  confirm arms, with a typed confirmation for the factory tier.
- The CLI can finally see the model registry: `pam model list` and
  `pam model status`, closing the gap where the GUI had ten model commands
  and the CLI had two.

### Changed

- The sidebar daemon control names the action instead of the state. It reads
  Start Pam or Stop Pam, with the state on its own line beside it, and
  stopping asks first - saying plainly that it unloads the model and drops
  queued work. Restart no longer appears and disappears as the daemon comes
  and goes.
- The application, its macOS bundle and its release artifacts are lowercase
  `pam`, and the interface calls the product Pam. **The published disk image
  is now `pam_<version>_darwin_arm64.dmg`**, so any script pointing at the
  old asset name needs updating.
- The macOS per-user data directory is `dev.pam.pam`. Pam is pre-1.0 and
  ships no migration for it: state under the previous directory is not
  carried over.
- The disk image opens as a designed window - artwork behind the icons, the
  app and Applications placed deliberately, and the Pam mark on the mounted
  volume.
- The protocol is version 9. Version 8 clients are answered with an explicit
  unsupported-version failure.

### Fixed

- Refusals that told you to restart Pam now name the operation that actually
  fixes them: unregister and delete-weights point at `pam model unload`, and
  inference against no loaded model points at `pam model load`.

## [0.9.0] - 2026-08-31

### Added

- Flows can be run from PAM itself. Pick a definition, run it against the
  project PAM is open on, and watch its transitions arrive live; cancel one
  in flight, read the outcome with its evidence handles, and browse a
  bounded history of past runs, each labelled with the project it ran
  against. Running a flow previously existed only in the CLI.
- The app is organized around six views — Overview, Models, Flows, Skills,
  Access and Activity — with Settings in the footer. Models is the single
  home for the local model: what is registered, what is loaded, the load
  meter, Verify and Chat, the curated download picker, and manual GGUF
  import.
- Loading a model shows real progress. The meter reads the daemon's own
  resident set size as the weights map in, instead of spinning without a
  denominator.
- The Overview activity heatmap covers a full year, with a month axis
  across the top and weekday labels beside it, so a quiet stretch can be
  placed in time instead of just seen.
- Access presents the daemon-scope capability grants and the observed
  boundary as global surfaces, and the model budget is derived from this
  Mac's real memory rather than assumed.

### Changed

- PAM's durable writes from the app now travel through the daemon whenever
  one is running, so the app is no longer a second writer to the store the
  daemon owns. Registering a model still works with the daemon stopped,
  which is what a first run needs. Protocol version 8.
- The layout targets a 2K desktop: the flow catalog and step inspector hold
  fixed widths instead of growing with the window, and full-width lists
  pair up two-across on wide screens.

### Fixed

- A daemon started with a large model could announce itself ready and then
  never answer anything, holding a core at 100% until it was killed, and
  ignoring Ctrl-C while it did. Its endpoint was accepting connections for
  the minutes the model took to load without ever reading them, and the
  abandoned health probes that piled up left it spinning instead of
  serving. The endpoint is now opened only once the daemon can serve.
- PAM can stop a daemon that has stopped answering, instead of leaving you
  to find and kill it yourself.
- The Overview model tile and the Activity caller identifiers are readable
  rather than truncated.
- Revoking a daemon-scope grant re-reads the observed boundary, so the
  Access view stops showing authority that was just withdrawn.
- Installing a skill from git no longer hangs for the full lifetime of a
  stray git process after its deadline has already passed.
- Two PAM windows racing the same database migration wait for each other
  instead of failing with a busy database.
- Loose content in Settings sits on the panel's own gutter.

### Removed

- The interactive design prototype. The shipped six-view app and its layout
  contract are the reference now; the tree stays in git history.

## [0.8.3] - 2026-08-26

### Fixed

- GGUF metadata that spells its license in lowercase ("apache-2.0") now
  prefills the license URL and notice too, not just the identifier.
- Re-importing a model you already registered no longer fails with
  "already registered with different metadata": an identical re-import
  is accepted as-is, and re-importing the same verified file with a new
  license notice updates your recorded consent. A different file
  claiming an existing model's identity is still refused.

## [0.8.2] - 2026-08-26

### Added

- When an imported GGUF declares no license in its own metadata, PAM asks
  the public Hugging Face index for the matching model and prefills the
  license identifier, URL, and notice from what the repository declares —
  narrated in the form, and falling back quietly to manual entry when
  offline or unmatched. Accepting the license stays yours.

## [0.8.1] - 2026-08-26

### Added

- Manual GGUF imports show live progress: a hashing bar with real bytes
  and percent, then a registering stage. The file is verified and
  registered in place — never copied.
- A running model download can be cancelled. The partial file stays on
  disk and the same model resumes right where it left off.

### Fixed

- Importing a model no longer freezes the rest of the app: the multi-GB
  verification used to hold PAM's command gate for its full duration,
  which left every other screen (Settings included) stuck loading until
  the import finished. Verification now runs in the background.

## [0.8.0] - 2026-08-26

### Added

- Settings shows the global flow library's on-disk location as a fourth
  storage entry — revealable in Finder like the model, data, and log
  directories.

### Fixed

- The Flows view's Visual mode now works without an active project:
  parsing a flow document into the graph and composing it back to TOML
  run under the daemon authority, matching the rest of the global flow
  library.

## [0.7.0] - 2026-08-26

### Added

- Flows are now a global library: definitions are named once and usable
  against any project you point PAM at, with the project chosen when a
  flow runs. Existing project-local flows migrate into the library
  automatically the first time it loads, and `pam flow run` accepts
  `--project` (defaulting to the current directory). Only runs remain
  project-scoped.
- A Settings view (gear icon, ⌘8) shows where PAM keeps things: the model
  download directory (now configurable and persisted), the data directory,
  and the daemon's on-disk logs with their size and a delete action —
  each revealable in Finder.
- Caller histories label callers by kind (CLI, GUI, coding agent, local
  application) with a shortened id instead of a raw UUID.
- Importing a GGUF that declares its own license prefills the license
  details — for well-known licenses including the canonical URL and
  notice — so the Advanced section usually never needs typing.

## [0.6.1] - 2026-08-26

### Fixed

- The manual import's Import button no longer sits silently disabled when
  the license details under Advanced are empty: clicking it opens the
  disclosure and names exactly what PAM needs.
- Two rare test-harness races were fixed at the root: a daemon scheduler
  handle could be polled after completion during teardown (masking the
  original failure), and the flow editor's save lock now retries briefly
  instead of failing a same-instant contender.

## [0.6.0] - 2026-08-26

### Added

- Caller histories across the Control Center and Activity views now show
  repo names instead of project ids: every CLI request carries its
  validated project root, the daemon remembers it, and the GUI labels
  projects by catalog name first, then the repo folder name with the full
  path on hover.
- The manual import's Browse button actually opens the file picker (the
  dialog module was missing from the shipped bundle in 0.5.2).

### Changed

- The runtime now accepts all three curated quantizations — balanced and
  high fidelity presets load after downloading instead of being refused.
- Resumed downloads report true progress, counting what is already on
  disk.
- A deep multi-agent review hardened the model surfaces: hostile paths
  (pipes, devices, symlinks) are rejected before they can stall the app,
  imports and inspections carry bounded timeouts, oversized model-name
  metadata no longer fails an import, the fleet usage list covers up to
  512 projects, downloads survive an app reload and block preset
  switching while running, stale badges and prefills clear correctly,
  phone-width fleet rows collapse properly, license checkboxes read
  distinctly, memory figures use one unit base, and light-theme warnings
  meet WCAG AA contrast.

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
