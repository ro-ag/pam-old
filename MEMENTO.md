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
- fix: Control Center: no project selector on main page (projects = per-caller daemon request counts only); model setup fully UI-driven incl. daemon restart — never render CLI commands as setup instructions in the GUI
- hits: 2026-08-22, 2026-08-24, 2026-08-24
- cost: 50
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
- fix: for f in MEMENTO.md AGENTS.md CLAUDE.md; do printf '%s\n' "$(cat $f)" > $f; done; verify: git diff --check $(git hash-object -t tree /dev/null) HEAD
- hits: 2026-08-24, 2026-08-24
- cost: 15
- status: watching
