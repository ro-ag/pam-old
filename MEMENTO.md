# Memento ledger (project)

Managed by memento.py — log with `memento hit`, do not hand-edit entry fields.

## pam-baywatch-not-rescue-console
- kind: habit
- scope: project
- rule: PAM's brand is Baywatch for developers, not a generic rescue or emergency-response tool.
- fix: Keep the calm timeline and evidence UX, but use confident sunlit coastal personality; avoid emergency-console language, sirens, shields, lifebuoys, and disaster styling.
- hits: 2026-08-17
- cost: 0
- status: watching

## license-exact-compare
- kind: habit
- scope: project
- rule: When claiming an exact file match, compare the complete unnormalized files byte-for-byte.
- fix: Use cmp on full files; report normalized or body-only comparisons explicitly instead of calling them exact.
- hits: 2026-08-18
- cost: 0
- status: watching

## complete-selected-ptrack-plan
- kind: habit
- scope: project
- rule: When asked to pick an available ptrack plan, complete every open task in that plan rather than stopping after the first task.
- fix: Inspect the full plan, continue through all open tasks, and mark the plan done only when each task has genuine completion evidence.
- hits: 2026-08-18
- cost: 0
- status: watching

## defer-windows-implementation
- kind: project-way
- scope: project
- rule: When Windows work is explicitly deferred, preserve clean platform seams without implementing or claiming Windows support.
- fix: Keep OS-specific operations behind narrow helpers or cfg boundaries, validate the current macOS/Linux targets only, and record Windows execution and durability proof as deferred.
- hits: 2026-08-18
- cost: 2
- status: watching

## pam-benchmark-target-too-rigid
- kind: habit
- scope: project
- rule: When the user authorizes a practical benchmark substitute, do not preserve an inferred hardware blocker; run on the authorized host with the requested resource cap and label the evidence accurately.
- fix: Reopen task #24, benchmark on the current M4 Max while enforcing a 20 GB maximum model-memory budget, and report the host distinction explicitly.
- hits: 2026-08-19
- cost: 0
- status: watching

## reuse-existing-pam-design-system
- kind: habit
- scope: project
- rule: When the PAM theme palettes and identity tokens already exist, reuse them exactly; do not propose or recreate palette work.
- fix: Inspect and apply the existing Ventisquero and Viña token maps while changing only shell composition and spatial hierarchy.
- hits: 2026-08-21
- cost: 0
- status: watching

## pam-features-coded-but-not-surfaced
- kind: project-way
- scope: project
- rule: PAM capabilities repeatedly land daemon/CLI-side without GUI exposure and the owner discovers them 'missing' (skills, model status, callers) — every feature plan must include its GUI surface or an explicit owner-approved deferral note
- fix: Control Center is GLOBAL — never render any project identity (name, breadcrumb, selector) on the main page; project_name from control_center.rs bootstrap must not reach Shell breadcrumb. Grep frontend for projectName renders when touching shell chrome.
- hits: 2026-08-22, 2026-08-24, 2026-08-24, 2026-08-25, 2026-08-27
- cost: 60
- status: enforced -> /Users/rodox/dev/rs/pam/AGENTS.md

## junk-removal-grep-references
- kind: habit
- scope: project
- rule: Before untracking or deleting a file as 'junk/residual', grep the repo for references — build scripts may consume dotfiles like .openai/hosting.json
- fix: rg -l '<filename>' before git rm; prototype/scripts/prepare-sites-build.mjs required prototype/.openai/hosting.json and CI prototype job failed at npm run test:sites
- hits: 2026-08-24
- cost: 0
- status: watching

## memento-md-trailing-blank-fails-foundation
- kind: project-way
- scope: project
- rule: In the pam repo, any file the memento CLI touches (MEMENTO.md via hit, AGENTS.md/CLAUDE.md via promote) must have its trailing blank line at EOF stripped before landing — the Foundation gate's git diff --check fails main on it
- fix: Applies to ANY file in the repo, not just MEMENTO.md/AGENTS.md/CLAUDE.md — a subagent-authored source file hit it too (frontend/src/views/ModelsView.test.tsx). The Foundation job runs: git diff --check $(git hash-object -t tree /dev/null) HEAD. Check before pushing with that exact command; strip with: printf '%s\n' "$(cat FILE)" > FILE. Brief subagents to run the diff --check themselves, since Foundation fails fast and skips every other job.
- hits: 2026-08-24, 2026-08-24, 2026-08-27, 2026-08-28
- cost: 20
- status: enforced -> /Users/rodox/dev/rs/pam/AGENTS.md

## pam-minimum-model-qwen3-coder-30b
- kind: project-way
- scope: project
- rule: PAM's minimum viable local model is Qwen3-Coder-30B-A3B-Instruct Q4_K_S — all owner validation ran on it; presets, floors, and docs must never offer smaller models as adequate
- fix: Preset catalog baseline = Qwen3-Coder-30B-A3B-Instruct Q4_K_S; manual-import floor sits just under its file size; larger quants (Q4_K_M+) are the upgrades
- hits: 2026-08-25
- cost: 15
- status: watching

## pam-min-system-32gb-ram
- kind: project-way
- scope: project
- rule: PAM targets machines with at least 32 GB RAM — local AI is the product premise; GUI hints, fixtures, docs, and hardware gates must treat 32 GB as the supported minimum, never curate for smaller hosts
- fix: host_memory DTO carries supportedMinimumBytes = 32 GiB; model panel warns below it; fixtures model a 32 GB Mac (34_359_738_368 bytes)
- hits: 2026-08-25
- cost: 5
- status: watching

## pam-flows-are-global
- kind: project-way
- scope: project
- rule: Flows are GLOBAL named definitions — define once, invoke against any project when asking PAM to do work; never store or present flow definitions per-project (runs stay project-scoped, definitions do not)
- fix: Flow library = daemon-global (like skills); Flows view shows the global catalog with no project switcher; the project is chosen at invocation time, not at definition time
- hits: 2026-08-26, 2026-08-27
- cost: 40
- status: watching

## pam-global-views-no-project-identity
- kind: project-way
- scope: project
- rule: PAM's Control Center, Flows, Access and Skills are global surfaces: never render a project selector, project name, path, breadcrumb or project-shaped empty state in them — only the callers log may show project identity.
- fix: Do not give a global view a contextBar prop or mount ProjectContextBar/ProjectPicker in it; project-scoped rows are labelled per row instead of gated behind a picker. Before shipping any view change: grep frontend/src for contextBar|projectContextBar|ProjectPicker and check each hit is the callers log.
- hits: 2026-08-27
- cost: 0
- status: watching

## pam-verify-in-the-running-app
- kind: gate
- scope: project
- rule: A PAM change is not verified until it has been exercised in the running app — unit and DTO-shape tests pass while the real daemon path (scope admission, authorization, replay guards) still refuses the request
- fix: Before calling GUI/daemon work done: build the RC, click the actual surface, and read daemon-stderr.log. Two defects this session (operation-UUID replay in Flows, network.diagnostics missing from capability_is_daemon_scoped) shipped green test suites and failed on first click. Authorization tests must send the real envelope through handle_incoming, not assert a DTO.
- hits: 2026-08-27
- cost: 0
- status: watching

## gh-pr-checks-watch-exits-zero-on-failure
- kind: habit
- scope: project
- rule: Never gate a merge on 'gh pr checks --watch &&' alone: it can exit 0 while a check is FAILURE, silently merging a red PR
- fix: After --watch returns, assert explicitly before merging: FAILED=$(gh pr checks <n> --json state --jq '[.[] | select(.state != "SUCCESS" and .state != "SKIPPED")] | length'); [ "$FAILED" = 0 ] && gh pr merge <n> --squash --delete-branch. Observed on PR 137: Desktop frontend FAILURE while --watch exited 0, merging a red PR to main.
- hits: 2026-08-31
- cost: 0
- status: watching
