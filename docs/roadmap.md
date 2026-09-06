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

## Phase 2 — Process Intelligence ✅

**Goal:** know *who* is listening.

**Implemented:**

- New `src-tauri/src/process/` engine: `sampler.rs` (pure logic + unit
  tests) · `windows.rs` (narrow unsafe FFI, RAII handle guard) · `mod.rs`
  (one-cycle pipeline + cross-cycle state).
- Windows APIs with **minimum access rights**
  (`PROCESS_QUERY_LIMITED_INFORMATION` only): `OpenProcess`,
  `QueryFullProcessImageNameW` (executable path → basename),
  `GetProcessTimes` (start time, cumulative CPU), `GetProcessMemoryInfo`
  (working set), plus a `CreateToolhelp32Snapshot` name lookup for
  access-denied PIDs. No PowerShell/WMIC/tasklist in the production path.
- `get_port_listeners` now returns `{ listeners, processes, capturedAt,
  durationMs }` — one payload per cycle, each unique PID sampled exactly
  once per cycle regardless of how many ports it owns.
- **CPU via delta sampling:**
  `cpu% = 100 × Δcpu_ticks / Δwall_ticks / logical_cores`, clamped 0–100,
  computed against the previous cycle's cache. First observation is `null`
  ("—"), never a fabricated 0%. The cache key includes the process creation
  time, so PID reuse invalidates the baseline; dead PIDs are dropped each
  cycle (no unbounded growth).
- **Memory metric: WorkingSetSize** (raw bytes in the domain; KB/MB/GB
  formatting only in the UI). **Start time:** creation FILETIME → Unix ms.
- **Access-denied semantics:** protected processes (lsass, services,
  svchost, …) keep their listener row and PID with `accessible: false` and
  null metadata plus a snapshot-derived display name — discovery never fails
  because one process refuses inspection.
- Frontend: `ProcessInfo` domain type, `processByPid` map in `portsStore`,
  process-aware Ports table (Port / Process / PID / CPU / Memory / Bind
  Address / IP / State), Dashboard rows with real process name + CPU + RAM,
  and the Services page is now the real grouped "Active Local Processes"
  view (`groupProcesses.ts` adapter — pure frontend logic, no extra
  scanning). All mock data and mock stores removed.

**Verification (all passed):**

- 50 Rust unit tests, including FILETIME conversion (boundaries, rounding,
  extreme values), basename extraction, CPU math (per-core normalization,
  clamping, first-sample, backwards-clock, counter-reset), PID-reuse cache
  identity, merge behavior, and live FFI checks.
- Live two-cycle system test (`cargo test -- --ignored --nocapture`):
  45–46 listeners, 23–24 unique PIDs, 11–12 accessible; cycle time 16–39 ms;
  CPU averages dynamic across runs (0.03% → 2.88% with a dev server
  polling).
- Independent cross-check via `Get-Process`: node.exe, explorer.exe and
  OneDrive.Sync.Service.exe matched on PID, name, full path and working set
  (±1–4%, expected for a fluctuating metric); PID 1068 (svchost) showed an
  empty `Path` in PowerShell too — access-denied confirmed independently.
- Port 1420 checked explicitly while the dev server ran: engine
  `1420 IPv6 ::1 PID 23232 node.exe C:\Program Files\nodejs\node.exe` —
  exact match with `Get-NetTCPConnection` + `Get-Process`.
- Desktop sanity: `tauri dev` launched the real app window against the new
  command.
- `npm run typecheck`, `npm run build`, `cargo check`, `cargo test` all green.

**Known limitations:** protected system processes stay metadata-less (by
design — no elevation); CPU values are per-window rates, not instantaneous
snapshots; working set is the only memory metric so far.

## Phase 3 — Service Detection ✅

**Goal:** name what is running.

**Implemented:**

- New `src-tauri/src/intelligence/` layer: `rules.rs` (pure, deterministic,
  fully unit-tested detector) + `mod.rs` (per-PID identity attached to the
  cycle; no extra Windows calls, so cycle cost stays at Phase 2 scale).
- **Command-line retrieval** added to `process/windows.rs` via the documented
  PEB walk — `NtQueryInformationProcess(ProcessBasicInformation)` → remote
  `PEB` → `RTL_USER_PROCESS_PARAMETERS.CommandLine` → `ReadProcessMemory` —
  requiring `PROCESS_VM_READ` on top of `QUERY_LIMITED_INFORMATION`, with
  graceful degradation (command line `null`) when refused. `ProcessInfo`
  gained `commandLine`.
- **ServiceIdentity model:** `kind` (30+ `ServiceKind` variants) ·
  `displayName` · `category` (frontend/backend/database/ai/infrastructure/
  unknown) · `confidence` (exact/high/medium/low enum — no fake percentages)
  · `evidence` (`{source, value}` entries retained for the details view).
- **Rule priority:** exact executables (postgres/mysqld/mariadbd/
  redis-server/ollama → Exact; llama-server/ComfyUI → High) → command-line
  rules scoped to the runtime family (next dev/start, vite → High;
  flask run, manage.py runserver, uvicorn → High; fastapi CLI, gradio →
  Medium) → path-based AI rules → honest fallbacks (Node.js/Python exact
  runtime, `OneDrive.Sync.Service.exe`-style low, `Unavailable`).
- **Anti-false-positive guarantees (tested):** port 3000 alone never means
  Next.js; port 7860 alone never means Gradio; uvicorn never auto-equals
  FastAPI; `node server.js` stays Node.js (no Express without its CLI);
  token-boundary matching prevents substring accidents; all matching is
  case-insensitive with original-case fallback display names.
- **Frontend:** `ServiceIdentity`/`Evidence`/`Confidence` domain types,
  `serviceByPid` map, Service column on Ports (hover shows evidence),
  identity-first Dashboard rows, Services page with category filters +
  confidence/category badges + expandable per-PID details (executable,
  command line, confidence, evidence), service-name search.

**Verification (all passed):**

- 89 Rust tests total (39 new detection tests incl. every acceptance-criteria
  rule); all Phase 1–2 tests untouched and green.
- Live: LocalStack's own Vite dev server on :1420 classified **Vite
  (High, evidence `command_line: "vite"`)** — command line independently
  confirmed via `Get-CimInstance Win32_Process`. A real **PostgreSQL
  (Exact)** discovered on a non-default port (55432) from executable
  evidence alone; CIM confirmed name + path. Framework-less node processes
  stayed "Node.js"; protected processes kept honest generic identities.
- Perf: 48 listeners / 25 unique PIDs / full cycle 11–43 ms.
- Desktop app relaunched against the Phase 3 binary; identities reached the
  UI. `npm run typecheck`, `npm run build`, `cargo check`, `cargo test` all
  green.

**Known limitations:** framework detection only as good as Windows allows
(protected processes → no command line → generic identity); Express/WebUI
need their own CLI evidence; classification is intentionally not
manifest-based (no package.json reading — Phase 4).

## Phase 4 — Project Detection ✅

**Goal:** tie services to the projects they come from.

**Implemented:**

- New `src-tauri/src/project/` layer, separate from discovery/process/
  intelligence: `mod.rs` (ProjectIdentity model, candidate extraction,
  resolution + content-addressed cache, cycle wiring) · `markers.rs`
  (marker scan, bounded parent walk, package.json/pyproject/Cargo/go.mod
  parsing, package-manager detection, start-command inference) · `git.rs`
  (read-only `.git` directory **and** worktree-file detection, branch from
  HEAD parsing — no git CLI ever spawned).
- **ProjectIdentity:** `{ id, name, rootPath, kind (node_js/python/rust/go/
  unknown), git { isRepository, rootPath, branch }, packageManager,
  startCommand { command, confidence, evidence }, confidence, evidence }`.
  Identity separation is preserved: `projects` (unique) + `projectLinks`
  (PID → project id) in the response; `projectByPid` in the store. Many PIDs
  share one project; nothing is duplicated per listener.
- **Evidence-based association:** absolute command-line paths (quote-aware;
  relative paths skipped) → bounded parent walk (max 10 levels, dependency
  directories lifted out) → confirmed root markers (package.json,
  pyproject.toml, setup.py/cfg, Cargo.toml, go.mod; lockfiles and
  compose files as supporting-only markers). **Home directories and drive
  roots are never claimed as project roots.** Confidence: exact (in-root
  path) / high (dependency-dir lift) / medium (supporting-marker root);
  no evidence → no project — the honest "Unknown Project".
- **Package manager:** `packageManager` manifest field wins; one lockfile
  decides (npm/pnpm/Yarn/Bun); conflicting lockfiles → honest `Ambiguous`.
- **Start-command inference:** command line ↔ package.json script mapping
  yields `npm run dev` / `pnpm dev` (manager prefix only with manager
  evidence, else the underlying command); unmapped command lines report
  verbatim as medium; nothing is fabricated.
- **Cache:** content-addressed on (executable path, command line) — warm
  cycles do zero filesystem work; bounded; the manual Refresh button passes
  `bypassProjectCache: true` to re-read markers, manifests and the Git
  branch from disk.
- **Frontend:** Projects page is real (project cards with root, branch,
  package manager, start command, confidence, per-process ports, CPU/RAM,
  expandable evidence details; unknown processes grouped honestly);
  Dashboard rows gained a project badge and the Projects summary card went
  live; Ports gained a Project column and project/branch-aware search;
  Services rows show the owning project with its confidence.

**Verification (all passed):**

- 141 Rust tests (52 new project-engine tests incl. temp-dir synthetic
  trees: nested node_modules resolution, walk bounds, package-manager
  matrix, script mapping, worktree files, multi-PID grouping, sibling
  separation, cache cold/warm/bypass/invalidation); all Phase 1–3 tests
  untouched and green.
- Live LocalStack verification: vite :1420 (PID from CIM: command line
  `D:\Projects\localstack\node_modules\.bin\..\vite\bin\vite.js`) resolved
  to **localstack-control-center**, root `D:\Projects\localstack`, Git
  branch **master** (matches `git rev-parse` + `git branch --show-current`),
  package manager **npm** (matches the sole package-lock.json), package
  name `localstack-control-center` (matches package.json), start command
  **npm run dev** (from the real `dev: vite` script) — association
  confidence High with the full evidence chain.
- PostgreSQL (PID 16752) stayed **project-unknown** — no fabricated project
  ownership for services launched outside a source project.
- A sibling project (crypto-intelligence-platform, branch phase3-recovery)
  resolved independently with `exact` in-root confidence — siblings are not
  merged.
- Perf: full cycle 20–29 ms cold at 45 listeners / 23 PIDs; warm cycles do
  no filesystem work.
- Desktop app ran the Phase 4 binary; project identities reached the UI.
  `npm run typecheck`, `npm run build`, `cargo check`, `cargo test` all
  green.

**Known limitations:** association requires command-line path evidence
(protected processes with unreadable command lines stay project-unknown);
Git branch changes surface on the next cache invalidation (command-line
change or manual refresh), not on the 3 s poll; Python/Rust/Go start-command
inference reports the observed command line (no ecosystem equivalent of
`npm run`); `.git` file worktrees are supported for branch reading, but
`gitdir`-relative edge cases beyond the documented layout are untested.

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
