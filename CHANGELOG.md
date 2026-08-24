# Changelog

All notable changes to PAM are documented in this file. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and PAM adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
