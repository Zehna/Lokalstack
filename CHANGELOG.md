# Changelog

All notable changes to LocalStack Control Center are documented here.
The project follows semantic versioning from 1.0.0 onward.

## 1.0.1 — Patch Release Candidate (2026-09-17)

### Fixed

- **Settings accessibility** — the Close behavior selector is now
  associated with its visible label, so accessibility APIs and screen
  readers expose the combobox as "Close behavior".
- **Windows tray accessibility** — the notification-area icon now uses
  the canonical "LocalStack Control Center" tooltip, giving the tray
  icon a useful accessible identity.

## 1.0.0 — Release Candidate (2026-09-12)

The first complete release. LocalStack Control Center is a Windows desktop
utility for understanding and safely managing local development environments.

### Capabilities

- **TCP listener discovery** — live Windows TCP table polling with process
  attribution, bounded caching, and honest "unknown" states.
- **Process intelligence** — names, paths, memory/CPU, start times, and
  command lines (bounded retrieval, PID-reuse safe).
- **Service & framework intelligence** — evidence-based identity for common
  stacks (Node, Python, PostgreSQL, Redis, AI runtimes, and more).
- **Project intelligence** — links running services back to local projects
  via manifests and git metadata (read-only).
- **Safe control** — hardened, opaque-target process control with two-step
  confirmation; PID-reuse protected; no raw PID API; denylist enforced.
- **Managed workspaces** — derive launch specs from project manifests
  (never frontend-typed), start/stop/restart with per-service readiness.
- **Conflict & dependency intelligence** — explains blocked launches:
  port ownership, dependencies, structured root-cause issues.
- **AI runtime observability** — read-only health, versions, model inventory
  and loaded models for Ollama, llama.cpp, ComfyUI, Gradio, Open WebUI.
  Loopback-only; never runs inference or mutates models.
- **Docker intelligence** — read-only container/port/compose observability
  over the Docker Engine named pipe. No lifecycle control, no exec.
- **Settings, tray, startup, single instance** — operational preferences,
  Windows tray icon, close-to-tray, launch-minimized, opt-in autostart,
  single-instance enforcement with focus restoration.
- **Hardening** — panic-audited, fail-closed control state, bounded logs and
  caches, handle/thread stability verified by soak testing.
- **Performance & accessibility** — sub-50 ms discovery median, flat memory,
  keyboard operability, accessible dialogs and status labels.

### Known limitations

- The release is **unsigned** — Windows SmartScreen will warn on first run.
- Theme preference is stored but visual theming is deferred.
- Auto-update is disabled; updates are manual until signed infrastructure
  exists.

### Release-candidate hardening (Phase 10F audit)

- Fixed: the Ports and Services views could fail to render with live
  discovery data — the backend flattened `capability` fields into each
  control entry while the frontend contract nests them under
  `capability`, so `ControlActions` crashed on real snapshots. A
  serialization-shape regression test now pins the wire contract.

## 0.1.0 — Development phases 0–10D

Internal development releases (not published).
