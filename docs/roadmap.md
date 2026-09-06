# Roadmap

LocalStack is developed in small, verified phases. Each phase must stand on its
own: the app builds, runs and stays honest about what it does and does not
know. Nothing in this roadmap includes destructive or privileged operations —
the product observes and explains localhost, it does not reconfigure machines.

## Phase 0 — Foundation ✅

**Goal:** clean architecture and the desktop shell.

- Tauri v2 + React + TypeScript + Vite + Tailwind CSS + Zustand + Lucide.
- Feature-oriented frontend structure (`src/features/*`).
- Placeholder Rust modules (`discovery`, `health`, `control`, `conflicts`,
  `workspace`, `ai`).
- Left navigation, dashboard with mock summary cards and a clearly-labeled
  mock "Active Services" list.
- Domain types (`Service`, `PortConflict`, `Workspace`, `SystemUsage`).
- No discovery, no OS interaction beyond the Tauri window itself.

**Done when:** typecheck, frontend build and `cargo check` all pass; the shell
renders all eight views with mock data properly labeled.

## Phase 1 — Port Discovery ✅

**Goal:** show the real listening ports on localhost.

**Implemented:**

- Windows-native TCP listener enumeration in `src-tauri/src/discovery/` via
  `GetExtendedTcpTable` (`TCP_TABLE_OWNER_MODULE_LISTENER` for `AF_INET` and
  `AF_INET6`) — no port-range probing, no `netstat` parsing.
- Structure: `ports.rs` (pure logic + unit tests) · `windows.rs` (narrow
  unsafe FFI, compile-time layout assertions) · `mod.rs` (facade + DTO).
- `get_port_listeners` Tauri command returning serde DTOs; Windows types
  never cross the boundary.
- Frontend flow: `services/native/ports.ts` (the only `invoke` site) →
  `portsStore` (listeners / loading / error / lastUpdated, `loadListeners` /
  `refreshListeners`) → `usePortListeners` hook → pages.
- Automatic refresh every ~3 s while Dashboard/Ports are mounted (clean timer
  cleanup, no overlapping requests) plus a manual Refresh button.
- Dashboard shows real listeners honestly (port, TCP · IPv4/6, address, PID —
  no invented identities); mock labels remain only on still-mock surfaces.
- Ports page: real table (Port / Protocol / Bind Address / IP Version / PID /
  State) with search by port/PID/address, ascending/descending port sort,
  loading/empty/error states, last-updated indicator, and no blind dedup by
  port (dual-stack and multi-address sockets each get a row).

**Verification (all passed):**

- 20 Rust unit tests (byte-order conversion, address decoding, table parsing,
  normalization) plus a live-system test of the full async command path.
- Cross-checked against independent Windows sources: every
  `Get-NetTCPConnection -State Listen` row (39) matched exactly on
  address + port + PID; every normalized `netstat -ano -p tcp` row matched.
- Port 1420 checked explicitly: engine output matched PowerShell
  (`::1`, same PID) while a dev server was live, and it appeared/disappeared
  with the server.
- `npm run typecheck`, `npm run build`, `cargo check`, `cargo test` all green.

**Known limitations:** Windows-only (explicit error elsewhere); TCP only;
PIDs not yet resolved to process names (Phase 2).

## Phase 2 — Process Intelligence

**Goal:** know *who* is listening.

- Map ports to PIDs, executable paths, command lines, CPU and RAM usage.
- Platform abstraction inside `discovery` so Windows specifics stay contained.
- First health signals from `health/` (process liveness, basic responsiveness).

## Phase 3 — Service Detection

**Goal:** name what is running.

- Classify listeners into known services (Next.js, Vite, Express, Flask,
  FastAPI, Django, PostgreSQL, MySQL, Redis, ...).
- Evidence-based classification (command line, ports, handshake responses) —
  never guessing on port numbers alone.

## Phase 4 — Project Detection

**Goal:** tie services to the projects they come from.

- Read-only filesystem inspection: package manifests, venvs, repo markers.
- Surface project directory and framework per service; extend the domain model
  (`Project`, service-to-project links).

## Phase 5 — Service Control

**Goal:** act, carefully.

- Explicit, user-confirmed start/stop/restart of user-owned dev processes.
- Every action requires confirmation in the UI and is recorded in history.
- The safety boundary in `docs/architecture.md` still applies: no system
  services, no elevation, no environment mutation.

## Phase 6 — Workspaces

**Goal:** group related services into development workspaces.

- Cluster services by project relationships and shared roots.
- Workspace views: "what belongs together is shown together."

## Phase 7 — Port Conflict Engine

**Goal:** explain and resolve conflicts.

- Detect competing listeners per port, present involved processes and
  resolution options (suggestions only — the user acts, the app never kills
  processes silently).

## Phase 8 — AI Service Detection

**Goal:** recognize local AI runtimes.

- Ollama, llama.cpp, ComfyUI, Gradio, Open WebUI and similar localhost AI
  services, with model/runtime metadata where available.
- Detection only — no model downloads or management in scope.

## Phase 9 — Docker

**Goal:** include containerized services.

- Read-only Docker integration: containers publishing localhost ports, mapped
  to their images. No Docker configuration changes.

## Phase 10 — Polish

**Goal:** make it a product.

- History/event log of observations, preferences, onboarding, performance
  work, packaging/installer hardening, accessibility pass.

---

## Standing constraints (every phase)

- localhost only — never scan remote machines or external networks.
- No killing processes, environment changes, firewall changes, database
  installs, Docker modifications, or privileged operations without an explicit,
  user-confirmed feature design.
- Each phase ends verified: TypeScript check, frontend build and Rust checks
  pass before moving on.
