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

## Phase 5 — Service Control ✅

**Goal:** act, carefully — the first write capability.

**Implemented:**

- New `src-tauri/src/control/` engine, hardened after a pre-commit safety
  audit: `mod.rs` (DTOs, per-cycle capability derivation, the full
  authorization chain, a source-level no-broadcast guard, the live
  end-process test) · `registry.rs` (**opaque control-target registry** —
  the native trust boundary) · `rules.rs` (pure denylist + development
  evidence + URL mapping) · `windows.rs` (narrow unsafe FFI: revalidation
  probe, liveness, bounded wait, TerminateProcess, ShellExecuteW —
  RAII-guarded; **no console control events, ever**).
- **The frontend cannot name a process.** A pre-commit audit rejected the
  original stop command that accepted PID + creation time + metadata from
  the frontend (forgeable) — and rejected the graceful-stop approach of
  `GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, 0)` after
  `AttachConsole(pid)`: group id 0 is a **console-wide broadcast** (every
  process sharing the attached console receives it; PID ≠ process-group
  id). Both are gone. Commands now accept only an **opaque target id**
  (BLAKE3-256 over PID ‖ creation time ‖ per-boot key — unpredictable,
  64-hex) issued server-side during discovery. The registry is bounded
  (1024), TTL-expiring (15 min), and replaced wholesale each refresh so
  stale ids stop resolving immediately.
- **Authorization chain at action time** (server-side, before any write
  primitive): resolve opaque id → re-inspect the live process → validate
  creation time + executable path (PID-reuse proof; mismatch →
  `STALE_TARGET`, unverifiable → `IDENTITY_UNVERIFIABLE`) → **recompute the
  denylist on fresh data** (reserved PIDs 0/4, system names, Windows
  directory, databases, infrastructure) → require the backend-stored
  development-evidence hint. Frontend-supplied service/project/name/canStop
  fields do not exist in the protocol; backend hints can tighten a refusal,
  never loosen one. Verified live: forged/unknown ids refused with the
  process untouched.
- **Conservative eligibility:** hard-refused list (System, smss, csrss,
  wininit, winlogon, services, lsass, svchost, dwm, explorer, conhost, …),
  anything under the Windows directory, database engines (postgres, mysqld,
  mariadbd, redis-server, mongod, sqlservr), infrastructure category,
  reserved PIDs, and every unverifiable identity. Controllable only with
  development evidence: a classified service identity or a confirmed
  project association. The honest refusal reason is always shown.
- **Honest stop semantics:** externally discovered processes were not
  launched into a LocalStack-managed process group, so
  `gracefulStopSupported` is `false` ("Process was not launched in a
  LocalStack-managed process group") and the UI action is **End Process** —
  never labeled "graceful", never auto-escalated, always explicitly
  confirmed. Unavailable graceful control is represented honestly rather
  than faked with a broadcast.
- **Phase 6 contract:** LocalStack-launched workspace processes
  (`CREATE_NEW_PROCESS_GROUP`, group identity retained) may regain a
  *targeted* `CTRL_BREAK` against their known group id — a genuinely scoped
  graceful stop. Externally discovered processes never will.
- **Open:** snapshot-derived localhost URLs only (127.0.0.1, [::1],
  wildcards → localhost), opened via ShellExecuteW (default browser), with
  URL-shape checks and optional PID liveness check in the command.
- **No process trees:** exactly the selected process is stopped.
- Commands: `end_process` (opaque target id), `open_service_url`;
  `get_port_listeners` carries per-PID `controls[]` (capabilities + URLs +
  opaque target ids).
- Frontend: `ControlActions` (Open + End Process with two-click
  `ConfirmButton`; copy states the process will be terminated — no fake
  "graceful" labeling) on Services and Ports rows; refusal reasons as
  tooltips; `controlStore` (single in-flight action, honest session history
  incl. stale/unknown-target refusals); the History page is now the real
  action audit trail. Restart is deliberately **not** implemented:
  `canRestart` is `false` everywhere — reliable restart needs working
  directory, environment
  and stream ownership, which belongs to Phase 6 workspaces.

**Verification (all passed):**

- 173 Rust tests, including the hardened control suite: opaque registry
  (unpredictable ids, bounded capacity, TTL expiry, refresh replacement,
  structured unknown/expired refusals), authorization chain (unknown id,
  expired id, refreshed-over id, forged-id, forged-metadata,
  reserved-PID-0/4, system-process, stale identity, changed-executable),
  URL mapping, capability/URL combination, and a **source-level guard**
  that fails the build if `GenerateConsoleCtrlEvent`, `AttachConsole`, or
  `CTRL_BREAK_EVENT` ever reappear in crate code.
- Live end-to-end (`cargo test -- --ignored --nocapture live_end_process`)
  against a **disposable child process** started by the test itself (with
  a leak-guard so even a failing assertion cannot orphan it): child
  discovered → opaque id issued → forged/unknown id refused, child
  untouched → valid id passed the full chain → explicit End Process
  terminated it → exit observed. **No console control events used.**
- `npm run typecheck`, `npm run build`, `cargo check`, `cargo test` all
  green; desktop app launched with control buttons live.

**Known limitations:** console-event grace only reaches shared-console
processes (detached-console servers need the confirmed force path); child
processes of a stopped dev server are intentionally not managed; stop is
the only lifecycle action — start/restart wait for Phase 6 orchestration;
history is in-memory per session (persistence belongs to Phase 10).

## Phase 6 — Managed Workspaces & Service Lifecycle ✅

**Goal:** LocalStack launches and manages development services itself — the
first phase with lifecycle write capability, built on the Phase 5 trust
model.

**Implemented:**

- **Managed vs external, enforced everywhere.** A managed service is one
  LocalStack launched (it owns the trusted launch spec, root PID + creation
  identity, and the known Windows process-group id). External services keep
  every Phase 1–5 rule: no graceful stop, End Process only via the opaque
  control-target registry, never restarted, never orchestrated. Workspace
  actions never touch external databases or infrastructure.
- **Workspace domain.** `Workspace { id, projectRoot, name, services,
  status }` with `WorkspaceService { id, name, role, expectedPort,
  launchSpecId, source, managedProcess }`. Creation is **explicit** in the
  UI: the backend derives launch candidates from trusted project metadata
  only (package.json `dev`/`start`/`serve` scripts via the detected package
  manager, `cargo run`, `go run .`), the user confirms which become managed
  services. Roles are evidence-derived (known dev-server tools →
  `frontend`, otherwise honest `other`).
- **Opaque launch specs.** The frontend sends only a `launchSpecId`; Rust
  stores the trusted spec (bounded BLAKE3-id registry, ids never survive an
  app restart) and revalidates root/program/cwd at launch time
  (`STALE_LAUNCH_SPEC` on drift). No cwd/program/args/env ever cross the
  boundary from the frontend.
- **Windows launching.** `CreateProcessW` with
  `CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW`, piped stdout/stderr into
  bounded per-service rings (1,000 lines, incremental index-based polling).
  Batch launchers (`npm.cmd`) run through one scoped `cmd.exe /d /s /c` with
  every argument individually quoted. Program resolution searches PATH with
  proper PATHEXT semantics (a real bug the live test caught).
- **Graceful stop, finally real — for managed processes only.** Targeted
  `CTRL_BREAK_EVENT` to the **known group id** (== root PID by construction;
  never group 0): attach to the child's own console when needed, send, wait
  bounded 5 s → `Stopped` or honest `StopTimeout`. Force Stop is a separate
  confirmed action, gated on the timeout, identity-revalidated, root-process
  only. Externally discovered processes never receive console events.
- **Restart** (managed-only): graceful stop → relaunch from the same trusted
  spec → genuinely new PID/creation identity/group; the old managed id
  cannot act on the new process. Graceful timeout aborts a restart — no
  silent force-and-restart.
- **Preflight.** Duplicate-launch detection (managed registry + listener
  table → `ALREADY_RUNNING` / honest external-instance message) and
  port-conflict preflight (`PORT_CONFLICT` with the owning PID — no
  automatic port changes).
- **Honest state machine.** `Starting → Running` only when the expected
  port (explicit `--port` evidence only) actually listens; alive-but-no-port
  past 25 s → `Degraded`; immediate death → `StartFailed` with the real exit
  code. No-port services go `Running` after a 3 s alive-grace. A monitor
  thread (500 ms tick, identity-checked liveness probes — no retained child
  handles) captures exits and cleans terminal entries (bounded registry, no
  zombies).
- **Workspace actions.** Start Workspace (managed services only, in
  deterministic role order), Stop Managed (managed registry entries only),
  Restart Managed (sequential, partial-failure honest). Workspace status is
  derived from managed states only (`stopped/starting/running/partial/
  stopping/error/conflict`) — external dependencies are never "stopped".
- **Frontend.** Real Workspaces page (create flow with detected candidates,
  service cards, Logs panel with auto-scroll and stream tags, per-service
  Start/Stop/Restart/Logs, workspace-level buttons), Dashboard workspace
  summary card, Services page MANAGED/EXTERNAL badges, History records
  lifecycle events.

**Verification (all passed):**

- 212 Rust tests (43 new): candidate derivation, spec validation, program
  resolution, duplicate/port-conflict preflight, status derivation,
  ordering, log-ring bounds, state transitions, registry bounds/opaque-id
  semantics, argument quoting.
- Live managed-lifecycle test (`live_managed_lifecycle_disposable_child`,
  `#[ignore]`d, disposable node child): launch → known group id == root PID
  → stdout captured → port listens (STARTING→RUNNING) → targeted
  CTRL_BREAK stop → relaunch → new identity → old identity refused → force
  with correct identity works. No group-0 broadcasts.
- `npm run typecheck`, `npm run build`, `cargo check` (0 warnings),
  `cargo test` all green; desktop app with the workspace UI verified.

**Known limitations:** managed state is in-memory — after an app restart,
previously managed services appear external and are not re-adopted (app
exit never kills them); children/grandchildren of a stopped managed root
are not managed (npm.cmd wrappers may leave grandchildren); roles beyond
`frontend`/`other` await evidence sources; logs are not persisted; env is
inherited (no .env integration yet).

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
