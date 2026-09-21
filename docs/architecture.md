# Architecture

LocalStack Control Center is a Windows-first desktop tool that helps developers
understand everything currently running on **localhost**. This document
describes how the codebase is layered and — critically — where the boundaries
between layers sit.

## Layer overview

```
┌─────────────────────────────────────────────────────────┐
│                     UI (React/TS)                       │
│  src/features/* — pages, components, mock data          │
├─────────────────────────────────────────────────────────┤
│              State & hooks (React/TS)                   │
│  src/stores/* (Zustand), src/hooks/*                    │
├─────────────────────────────────────────────────────────┤
│              Domain types (TypeScript)                  │
│  src/types/domain.ts — Service, PortConflict, ...       │
├─────────────────────────────────────────────────────────┤
│         Tauri command boundary (IPC, explicit)          │
│  @tauri-apps/api invoke() ⇄ #[tauri::command] fns       │
├─────────────────────────────────────────────────────────┤
│          Rust feature modules (src-tauri/src/*)         │
│  discovery · process · intelligence · project · control │
│  · health · conflicts · workspace · ai                  │
├─────────────────────────────────────────────────────────┤
│      Native engines (OS APIs, Windows-first)            │
│  TCP table + process enumeration, filesystem, ...       │
└─────────────────────────────────────────────────────────┘
```

## 1. UI layer (React + TypeScript)

- Lives in `src/features/<feature>/`. Each feature owns its pages and
  components and may not reach into another feature's internals — shared
  presentation pieces move to a `components/` module of the owning feature and
  get imported explicitly.
- The shell (`src/app/`) owns navigation and layout only. It renders one
  placeholder view per navigation entry; it contains no domain logic.
- React is responsible for **presentation and state**, nothing else. It never
  queries the operating system, never spawns processes, and never inspects the
  filesystem.

## 2. State & hooks

- Zustand stores (`src/stores/`) hold cross-view state: the app store tracks
  the active view, and `portsStore` holds **real** discovery data from the
  native engine — listeners, the per-PID `processByPid` map, service
  identities (`serviceByPid`), resolved projects (`projects` +
  `projectByPid`), cycle duration, loading/error/lastUpdated. There is no
  mock store; every rendered row is native data.
- Hooks (`src/hooks/`) are the only place components start side effects.
  `usePortListeners` performs the initial load and the ~3-second automatic
  refresh.

### 2.1 Native client boundary (Phase 1+)

Components never call `invoke()` directly. The only module allowed to touch
the Tauri IPC surface is `src/services/native/` (Phase 1: `ports.ts`). The
call chain is strictly:

```
React component → hook → Zustand store → native client → Tauri invoke → Rust
```

This keeps the transport swappable and makes every data source auditable:
searching for `invoke(` finds exactly one directory.

## 3. Domain types (TypeScript)

- `src/types/domain.ts` defines the concepts the UI renders: `Service`,
  `ProcessInfo`, `PortListener`, `PortConflict`, `Workspace`, `SystemUsage`,
  `ViewId`.
- These types are the **contract** with the backend: Tauri commands return
  serde DTOs that serialize into exactly these shapes. Fields are added when
  an engine actually produces the data, not before. Listener information
  (`PortListener`) stays distinct from process information (`ProcessInfo`);
  they are merged only at render time via the PID key.

## 4. Tauri command boundary

- All OS interaction is behind Tauri **commands** (`#[tauri::command]` in
  `src-tauri/src/`, invoked via `@tauri-apps/api`).
- Commands are explicitly registered in `invoke_handler` and capabilities are
  explicitly declared in `src-tauri/capabilities/`.
- The boundary is also the **safety boundary**: no command may kill processes,
  mutate environment variables, touch firewall rules, reach remote machines, or
  require elevation — with exactly one, narrow, user-confirmed exception
  introduced in Phase 5: ending a *revalidated, eligibility-checked,
  user-owned development process* (`end_process`, authorized by an opaque
  backend-issued target id). System processes, databases, infrastructure,
  and unverifiable identities are refused by construction, the denylist is
  recomputed on fresh data at action time, and identity is revalidated at
  click time. Everything else in this boundary remains read-only.

### 4.1 Registered commands

| Command              | Returns                                        | Phase |
|----------------------|------------------------------------------------|-------|
| `greet`              | sample string (boundary smoke test)            | 0     |
| `get_port_listeners` | `PortListenersResponse` — listeners + process intelligence + service identities + project resolution + control capabilities | 1–5  |
| `end_process` | user-confirmed End Process of an opaque revalidated control target (denylist recomputed at action time) | 5 |
| `open_service_url` | open a snapshot-derived localhost URL in the default browser | 5 |
| `get_workspace_candidates` | backend-derived launch candidates for a project root (read-only) | 6 |
| `create_workspace` / `remove_workspace` | explicit workspace creation from derived candidates / removal | 6 |
| `list_workspaces` | workspace views incl. derived status + managed process state | 6 |
| `start_managed_service` / `stop_managed_service` / `restart_managed_service` | managed lifecycle actions on opaque ids (`force` only after a graceful timeout) | 6 |
| `start_workspace_services` / `stop_workspace_services` | workspace-level start (managed only, role order) / stop (managed registry only) | 6 |
| `get_service_logs` | incremental bounded log lines for one managed process | 6 |

## 5. Rust feature modules

`src-tauri/src/` mirrors the product's future engines. Each module owns one
responsibility:

| Module        | Responsibility                                        | Phase |
|---------------|-------------------------------------------------------|-------|
| `discovery`   | TCP listener enumeration (`GetExtendedTcpTable`)        | 1    |
| `process`     | Process intelligence: names, paths, CPU/RAM, start time, command line | 2–3 |
| `intelligence`| Evidence-based service & framework classification       | 3    |
| `project`     | Project resolution from process evidence (markers, Git, package manager) | 4    |
| `health`      | Localhost health probes and per-service health state   | 5+    |
| `workspace`  | Managed workspaces: launch specs, lifecycle, logs      | 6     |
| `control`    | Explicit, user-confirmed service control actions       | 5     |
| `conflicts`  | Detect and explain port conflicts                      | 7     |
| `ai`         | Detect local AI runtimes (Ollama, llama.cpp, ...)      | 8     |

### 5.1 Port discovery engine (implemented, Phase 1)

```
discovery/
├── mod.rs        facade: async command body, response DTO, platform dispatch
├── ports.rs      pure logic: DTOs, byte-order conversion, address decoding,
│                 normalization — fully unit-tested, no OS access
└── windows.rs    the ONLY unsafe code in the crate: GetExtendedTcpTable FFI
                  (sizing probe → query → bounded row copy), with compile-time
                  struct-layout assertions
```

Design points:

- **Windows-native, not screen-scraping.** Rows come from
  `GetExtendedTcpTable(TCP_TABLE_OWNER_MODULE_LISTENER)` for `AF_INET` and
  `AF_INET6` — the same tables `netstat` itself reads. There is no port-range
  probing anywhere.
- **No native types cross the boundary.** The FFI surface ends inside
  `windows.rs`; the command returns a serde DTO whose JSON shape is mirrored
  by `src/types/domain.ts`.
- **Correctness pinned by tests.** Network-byte-order port conversion (the
  OS stores ports big-endian in the low 16 bits of a dword), IPv4/IPv6 address
  decoding, and deterministic normalization are pure functions with unit
  tests; row offsets are enforced at compile time; a live-system test
  (`cargo test -- --ignored --nocapture`) exercises the real tables.
- **Dedup is address-aware.** Rows are deduplicated only on the full
  (address, port, pid) tuple — the same port on `0.0.0.0` and `::` is two
  sockets and both are shown.

### 5.2 Process intelligence engine (implemented, Phase 2)

```
process/
├── mod.rs       facade: one-cycle pipeline, cross-cycle SampleCache state,
│                the get_port_listeners command
├── sampler.rs   pure logic: ProcessInfo DTO, FILETIME conversion, basename
│                extraction, delta CPU math, cache merge — fully unit-tested
└── windows.rs   unsafe FFI: OpenProcess / QueryFullProcessImageNameW /
                 GetProcessTimes / GetProcessMemoryInfo (+ toolhelp snapshot
                 name lookup); every handle closed via an RAII guard
```

Sampling architecture (one refresh cycle):

```
listeners (GetExtendedTcpTable)
  → unique PID set (BTreeSet)
  → OpenProcess once per PID (minimum rights: PROCESS_QUERY_LIMITED_INFORMATION)
  → path + times + memory per PID
  → delta CPU vs the previous cycle's cache (per-core normalized)
  → merge into PortListenersResponse { listeners, processes, capturedAt, durationMs }
```

Design points:

- **Minimum access rights.** Only `PROCESS_QUERY_LIMITED_INFORMATION` is
  requested — it covers all three queries. No `PROCESS_ALL_ACCESS`, no
  elevation.
- **Access-denied is data, not an error.** A protected process (services.exe,
  lsass.exe, most svchost.exe instances) keeps its listener row and PID and
  renders as `accessible: false` with `null` metadata; a best-effort display
  name comes from the toolhelp process snapshot, which needs no handle. A
  process that dies between listener enumeration and inspection is the same
  honest case.
- **CPU is a delta, never a cumulative counter.**
  `cpu% = 100 × (Δ cpu_ticks / Δ wall_ticks) / logical_cores`, computed per
  refresh cycle against the previous cycle's sample, clamped to 0–100. The
  first observation is `null` ("—" in the UI), never a fabricated `0%`.
  Ticks are 100 ns units from `GetProcessTimes`; wall time comes from the
  snapshot timestamp. The cache key includes the process creation time, so a
  reused PID cannot inherit a stale baseline; dead PIDs are dropped every
  cycle, so the cache cannot grow unboundedly.
- **Memory metric: WorkingSetSize** from `GetProcessMemoryInfo`, exposed as
  raw bytes in the domain; formatting to KB/MB/GB happens only in the UI
  layer.
- **Start time** is the `GetProcessTimes` creation FILETIME converted to Unix
  epoch milliseconds (tested conversion, isolated in `sampler.rs`); the UI
  renders it as a local time or relative age.
- **No duplicated scans.** Five listeners sharing one PID cost one
  `OpenProcess` per cycle; the Services page groups rows by PID purely in
  frontend logic (`groupProcesses.ts`) — no second native pass.

### 5.4 Service & framework intelligence engine (implemented, Phase 3)

```
intelligence/
├── mod.rs     facade: per-PID identity DTO, pure classify step over the
│              cycle's processes + listeners (no extra Windows calls)
└── rules.rs   the pure, deterministic, fully unit-tested detector:
               ServiceIdentity / ServiceKind / ServiceCategory /
               Confidence / Evidence, rule tables, token matching
```

Pipeline: `PortListener → ProcessInfo → ProcessEvidence → detect_service →
ServiceIdentity`. Identity is **PID-based** — all listener rows of one
process share one identity (`services` array in the response, keyed as
`serviceByPid` in the frontend).

**Product principle:** never claim a framework or service identity without
sufficient evidence. "Node.js" beats an incorrect "Next.js". Port numbers
are never strong evidence — node.exe on :3000 stays Node.js, a Python web
server on :7860 stays Python/Uvicorn.

**Evidence model.** Every identity carries its evidence:
`{ source: process_name | executable_path | command_line, value }`.
Retained in the domain for the Details view; the UI may summarize it.

**Confidence model (enum, not a number):**

- `exact` — the executable itself identifies the service (postgres.exe,
  mysqld.exe, redis-server.exe, ollama.exe) or the runtime name is certain
  (node.exe → Node.js even without framework evidence).
- `high` — strong command-line/path evidence (next dev/start, vite,
  flask run, manage.py runserver, uvicorn, llama-server.exe, ComfyUI path).
- `medium` — plausible but not conclusive (fastapi CLI, gradio CLI,
  open-webui, Docker helper processes).
- `low` — weak indication (generic unknown executables, inaccessible
  processes).

**Rule priority:** exact executables → command-line rules scoped to the
detected runtime family (Node, Python) → path-based AI rules → runtime
fallbacks (Node.js / Python / Java Process) → generic fallback keeping the
real executable name (`OneDrive.Sync.Service.exe`, confidence `low`) or
`Unavailable` for inaccessible PIDs.

**Token matching:** command-line needles match at word-ish boundaries
(start/end, whitespace, quotes, path separators, `=`/`:`/`,`) — so
`next` does not fire inside unrelated flags. Detection is case-insensitive
end to end (Windows paths are); fallback display names preserve the
original case.

**Command-line retrieval (new in Phase 3).** Command lines are read natively
and read-only: `NtQueryInformationProcess(ProcessBasicInformation)` →
remote `PEB` → remote `RTL_USER_PROCESS_PARAMETERS` → UTF-16
`CommandLine` buffer, copied with `ReadProcessMemory` — the same documented
mechanism Sysinternals tools use, requiring `PROCESS_VM_READ` on top of
`PROCESS_QUERY_LIMITED_INFORMATION`. The opener tries the richer rights
first and **degrades gracefully** to limited-only (command line `null`, all
other metadata intact) when the target refuses — expected for protected
system processes. Cross-bitness targets are skipped rather than guessed.

**Fallback behavior.** Every process gets a usable identity: known runtimes
stay honest (Node.js, Python — confidence `exact` for the runtime, not the
framework), unknown executables keep their real name with `low`, and
inaccessible processes render `Unavailable` while keeping port + PID.

### 5.6 Safe service control engine (implemented, Phase 5)

```
control/
├── mod.rs      facade: ControlTarget / ControlCapability / PidControl /
│               StopResult DTOs, per-cycle capability derivation, the
│               authorization chain (authorize_action), tests incl. the
│               source-level no-broadcast guard and the live end-process test
├── registry.rs opaque control-target registry — the native trust boundary:
│               BLAKE3-derived unpredictable ids, bounded capacity,
│               TTL expiry, refresh-time wholesale replacement
├── rules.rs    pure eligibility (denylist + development evidence) and
│               browsable-URL mapping, fully unit-tested
└── windows.rs  narrow unsafe FFI: revalidation probe, liveness, bounded
                wait, TerminateProcess, ShellExecuteW — all RAII-guarded.
                **No console control events, ever.**
```

**The safety model — the product's first write capability.**

- **The frontend cannot name a process.** Control commands accept only an
  **opaque target id** issued by the backend during discovery. IDs are
  BLAKE3-256 over (PID, creation time, per-boot random key) — unpredictable
  and unusable for any other PID. The `ControlTargetRegistry` is the trust
  boundary: bounded (1024 entries), TTL-expiring (15 min), and replaced
  wholesale on every refresh cycle so stale ids stop resolving the moment
  fresher data exists. Policy-bearing fields (PID, creation time, service
  category, project, canStop) are **never accepted from the frontend**.
- **Every action runs the full authorization chain server-side**
  (`authorize_action`): ① resolve the opaque id (unknown/expired →
  `UNKNOWN_TARGET` refusal) → ② re-inspect the live process right now
  (`revalidate`) → ③ validate identity: creation time (PID-reuse proof) +
  executable path, case-insensitive; mismatch → `STALE_TARGET`, unverifiable
  → `IDENTITY_UNVERIFIABLE` → ④ **recompute the denylist on the fresh data**
  (`denylist_refusal`: reserved PIDs 0/4, system process names, Windows
  directory, databases, infrastructure) → ⑤ require the backend-stored
  development-evidence hint → only then `TerminateProcess`.
- **Backend-stored evidence hints, never frontend hints.** The trusted
  target carries only what the backend itself derived at issuance (service
  category, dev-evidence flag). These hints can *tighten* a refusal
  (infrastructure category) or satisfy the evidence gate — they can never
  loosen a hard deny rule, which always re-runs on fresh process data.
- **Conservative eligibility (`rules.rs`, pure + unit-tested).** Refusal
  priority at issuance: reserved PIDs (0, 4) → inaccessible process → no
  creation time → Windows system process list (System, smss, csrss,
  wininit, winlogon, services, lsass, svchost, dwm, explorer, conhost, …)
  → anything under the Windows directory (`SystemRoot`) → database engines
  (postgres, mysqld, mariadbd, redis-server, mongod, sqlservr) →
  infrastructure category (Docker) → finally *require development
  evidence*: a classified service identity or a confirmed project
  association. Unclassified unknown executables are refused with a reason.
  The reason is always shown in the UI tooltip.
- **No graceful console-stop for externally discovered processes.** An
  earlier design used `AttachConsole(pid)` +
  `GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, 0)` — that is a **broadcast**:
  group id 0 reaches every process sharing the attached console, and a PID
  is not a process-group id. It has been removed entirely (a source-level
  regression test greps the crate's code — comments and strings stripped —
  and fails if `GenerateConsoleCtrlEvent`, `AttachConsole`, or
  `CTRL_BREAK_EVENT` ever return). Honest semantics instead:
  `gracefulStopSupported: false` with reason "Process was not launched in
  a LocalStack-managed process group", and the user-facing action is
  **End Process** — explicit, confirmed, terminating.
- **End Process (the only stop path in Phase 5).** One explicit,
  two-click-confirmed action that runs the authorization chain and then
  terminates exactly the selected process (`TerminateProcess`, PID-scoped,
  RAII-guarded handle), followed by a bounded 3 s wait and an honest
  `stopped / stillRunning` report. There is no automatic escalation and no
  staged "graceful-then-force" dance — nothing fake is attempted first.
- **Phase 6 contract (documented, not implemented).** When LocalStack
  itself launches a workspace process — `CREATE_NEW_PROCESS_GROUP`, with
  LocalStack retaining the process-group identity — a *targeted*
  `CTRL_BREAK` to that **known group id** can restore genuine graceful
  stops for managed processes. Externally discovered processes will never
  regain one.
- **Open.** `ShellExecuteW` with the `open` verb (respects the default
  browser). URLs are derived only from localhost bind forms — `127.0.0.1`,
  `[::1]`, and wildcards `0.0.0.0`/`::` mapped to `localhost`; specific
  non-loopback addresses get no URL. The command re-checks URL shape
  (http-only, no query/credentials) and, when a PID is supplied, that the
  process still exists.
- **No process trees.** Exactly the selected process is stopped; parents
  and unrelated children are never touched (documented Phase 5 limitation;
  framework-proven parent/child relations would need their own design).

### 5.5 Project intelligence engine (implemented, Phase 4)

```
project/
├── mod.rs      facade: ProjectIdentity model, candidate extraction,
│               resolve_process / resolve_projects (content-addressed cache),
│               cycle wiring
├── markers.rs  pure + bounded filesystem logic: marker scan, parent walk,
│               package.json parsing, package-manager detection,
│               start-command inference, manifest name extraction
└── git.rs      read-only Git facts: .git directory AND worktree file,
               branch from HEAD parsing — no git CLI is ever spawned
```

Pipeline: `ProcessInfo (command line, executable path) → candidate paths →
bounded parent walk → confirmed project root → manifests → ProjectIdentity`.

**Candidate extraction.** Only absolute paths (drive-letter or UNC) from
quote-aware command-line tokenization are candidates. Relative paths are
skipped — the working directory is unknown, and resolving against the app's
own CWD would fabricate evidence. The executable path is a last-resort
candidate (covers `target\debug\app.exe`-style launches); a runtime install
directory simply finds no marker and yields no project.

**Bounded parent walk.** From each candidate the walk ascends at most
`MAX_PARENT_DEPTH` (10) levels, stopping at the first strong marker
(package.json, pyproject.toml, setup.py/cfg, Cargo.toml, go.mod). Supporting
markers (lockfiles, requirements.txt, Pipfile, poetry.lock, uv.lock,
docker-compose.yml, compose.yml) confirm a root but never claim one beneath
a stronger candidate. Paths inside dependency directories (`node_modules`,
`.venv`, `venv`, `site-packages`, `target`, `dist`, `build`) are lifted to
their nearest non-dependency ancestor before the walk. **Home directories
and drive roots are never claimed as project roots** — a stray package.json
in `C:\Users\me` cannot own a process running deep inside `AppData`.

**ProjectIdentity model.** `{ id (root path), name, rootPath, kind
(node_js/python/rust/go/unknown), git { isRepository, rootPath, branch },
packageManager, startCommand { command, confidence, evidence }, confidence,
evidence }`. Names come from the manifest when readable (package.json
`name`, pyproject `project.name`, Cargo `[package] name`, go.mod `module`
last segment) and fall back to the directory basename.

**Package-manager detection.** `package.json` `"packageManager"` field wins
always; otherwise exactly one lockfile decides (package-lock → npm,
pnpm-lock → pnpm, yarn.lock → Yarn, bun.lock/.lockb → Bun); conflicting
lockfiles produce the honest `Ambiguous` — never a silent choice.

**Start-command inference.** If the observed command line maps to a
package.json script (its underlying command, e.g. script `"dev": "vite"`
and a vite process), the honest result is `npm run dev` / `pnpm dev` — with
the manager prefix only when manager evidence exists, otherwise the
underlying command. A directly run script reports verbatim. Unmapped
command lines are reported as-is with `medium` confidence; nothing is ever
fabricated, and no command line means no command.

**Association confidence.** `exact` = direct in-root path evidence (script
inside the project itself); `high` = dependency-directory lift
(`node_modules/vite/bin/vite.js` → enclosing project); `medium` =
supporting-marker-only root. No evidence → no project at all: the process
stays unlinked ("Unknown Project" in the UI), never forced.

**Evidence model.** Same shape as Phase 3 (`{ source, value }`), with
sources `command_path`, `marker`, `package_json`, `lockfile`, `start_command`,
`git_root`, `pyproject`, `cargo_manifest`, `go_mod`.

**Identity separation.** The response carries `projects` (unique

```
Rust: discovery/windows.rs (TCP FFI) ──► ports.rs (normalize) ─┐
                                                               ├─► process/mod.rs ──► intelligence (classify)
Rust: process/windows.rs (OpenProcess + PEB FFI) ──► sampler.rs ┘                 │
                                                                                  ├─► project (resolve, cached) ──► DTO
                                                                                  │
React ◄─ stores/portsStore (listeners · processByPid · serviceByPid · projects · projectByPid) ◄─ hooks/usePortListeners ◄─ services/native/ports.ts
                                                                                  │
                                                                 invoke('get_port_listeners', { bypassProjectCache? })
```

### Refresh behavior (Phase 1–3)

- **Automatic:** while the Dashboard, Ports or Services view is mounted, a
  full cycle (listeners + process sampling + classification) runs every
  ~3 seconds (`usePortListeners`). Timers are cleaned up on unmount; the
  store drops overlapping requests, and the Rust-side cache mutex
  serializes cycles as defense in depth. A cycle on a typical dev machine
  takes ~11–43 ms even with command-line retrieval, so the 3 s cadence is
  comfortably non-overlapping.
- **Manual:** a Refresh button is present on Dashboard, Ports, Services and
  Projects and goes through the same store action. A manual refresh
  additionally re-reads project metadata from the filesystem
  (`bypassProjectCache`), so Git branch changes and new markers appear
  without restarting.
- **Failure handling:** errors surface in an inline error state; the last
  successful snapshot stays visible. A failing poll never clears data and
  never crashes the app.
- A server that starts or stops is reflected automatically within one poll —
  no user action required.

### Managed-workspace data flow (Phase 6)

```
React (WorkspacesPage / workspaceStore) ── invoke('create_workspace', { projectRoot })
      │                                   ◄─ launch candidates (backend-derived)
      │── invoke('start_managed_service', { launchSpecId })   ← opaque id only
      │        Rust: lookup trusted spec → revalidate root/program/cwd
      │              → duplicate + port-conflict preflight → CreateProcessW
      │              (CREATE_NEW_PROCESS_GROUP) → pipes → log ring
      │── invoke('get_service_logs', { managedId, afterIndex })  ← incremental
      │── invoke('stop_managed_service', { managedId, force? })
      │        Rust: identity revalidation → targeted CTRL_BREAK (known group,
      │              never 0) → bounded wait → honest state
      └── monitor thread: readiness (port evidence), exits (identity-checked
          probes), zombie cleanup — event-ish, 500 ms tick
```

### Conflict & dependency intelligence (Phase 7)

Two separate engines feed one readiness view:

```
Workspace state ─┬─ conflicts/    (Phase 7A)  Port Conflict Engine
                 │     rules.rs     — pure bind semantics + classification
                 │     free_port.rs — pure bounded free-port finder
                 │     mod.rs       — owner resolution from a live snapshot
                 │                    + Tauri commands (evaluate_port,
                 │                    find_free_ports)
                 └─ dependencies/ (Phase 7B) Dependency / Readiness Engine
                       graph.rs     — cycle detection + topological order
                       readiness.rs — deterministic readiness + root causes
                       mod.rs       — dependency storage + Tauri commands
```

**Port conflicts (7A).** Every evaluation starts from a *fresh* listener
snapshot — ownership is never cached (spec §33). The owner of each listener
is resolved through the same one-shot sampling used by discovery (process
name → service identity → project identity → managed-registry lookup), and
fields that could not be resolved stay absent rather than being invented.
A `PortOwner` carries pid, process/service/project names, and the
`managed`/`external` lifecycle. Classification (`no_conflict`,
`already_running`, `same_project_external`, `same_project_managed`,
`other_project`, `unknown_owner`, `dual_stack_equivalent`,
`reserved_or_unverifiable`) never uses port numbers as evidence — the same
port held by the *exact same managed service* is `already_running` (info),
not a conflict. Bind semantics are modeled as scopes (wildcard/loopback/
specific per family): same-family overlap is blocking, cross-family pairs
are disjoint, and IPv6-wildcard dual-stack behavior is **never claimed safe**
— it is `potential` with an explicit "bind semantics cannot be verified"
message. The Free Port Finder is advisory-only and bounded: at most 100
consecutive ports are examined, at most 5 suggestions are returned, the
occupied preferred port leads the Used list, and no configuration is ever
edited and no bind test is performed.

**Dependencies (7B).** Edges are created only from user-confirmed mappings
(backend-validated: source and target service must belong to the workspace,
self-edges are refused, HTTP endpoints are localhost-only). Targets are
tagged (`service`, `external_service`, `tcp_port`, `http_endpoint`); each
carries `required` — required-unavailable blocks readiness, optional-unavailable
warns. `DependencyState.available` claims only that the endpoint is
*listening*; the issue text says explicitly that a listening port is not a
health claim. The graph engine detects cycles (reporting the path, e.g.
`a → b → a`, as an ERROR issue) and derives a topological start order for
managed services; external dependencies are sinks and never auto-started.

**Readiness.** `evaluate_all` joins managed states, dependency states, and
conflict verdicts into one deterministic status per workspace with a
documented precedence: `error` (cycle / start-failed / stop-timeout) →
`conflict` (blocking port occupied) → `blocked` (required dependency down) →
`starting` → `ready` (every managed service fully Running) → `partial`
(some running, or any Degraded) → `stopped`. Every non-ready status carries
structured `ReadinessIssue`s (`PORT_OCCUPIED`, `DEPENDENCY_UNAVAILABLE`,
`START_TIMEOUT`, `DEPENDENCY_CYCLE`, …) with stable identities (code +
service/dependency ids + port) so the history layer can record
**transitions** — one event per change, never per poll.

**Preflight integration.** Workspace/service start runs the conflict engine
before `CreateProcessW`: a blocking conflict refuses the launch with
`PORT_CONFLICT` (the owner is never stopped automatically), an
external-instance condition reports honestly, and a required dependency
that is down gates the start (conservative: the workspace preflight does
not partially auto-start around a blocked dependency).

```
React (WorkspacesPage / conflictsStore) ── invoke('get_workspaces_readiness')
      │                                   ◄─ readiness + issues + deps + conflicts
      │── invoke('evaluate_port', { port })  ◄─ owner report + resolutions
      │── invoke('find_free_ports', { preferred })  ◄─ bounded advisory list
      │── invoke('add_workspace_dependency', …)  ← user-confirmed, validated
      └── conflict dialog: owner details + advisory alternatives; no
          destructive action is ever automatic
```

## What stays out of scope by design

- No generic network/port scanning of remote or external targets — localhost
  only, always.
- No privileged operations. If a future feature seems to need elevation, the
  feature gets redesigned, not the permission model.
- Process control exists only through the Phase 5 safety model: opaque
  revalidated targets, conservative eligibility, graceful-first stop, and a
  second confirmation before force. No PID-typed input from the frontend is
  ever accepted, no process tree is ever killed, and no system/database/
  infrastructure process is ever controllable.
- Restart and workspace orchestration exist **only** for LocalStack-managed
  processes (Phase 6): trusted backend-derived launch specs, known process
  groups, identity revalidation at every step. External services can never
  be restarted or orchestrated, and workspace actions never touch external
  databases/infrastructure.
- AI runtime probing (Phase 8) exists **only** for runtimes the Phase 3
  classifier already identified, only over loopback, only read-only GETs of
  approved inspection paths. There is no command equivalent to
  `http_get(url)` — the frontend passes opaque `runtimeId`s; endpoint
  selection is entirely backend-controlled.

### AI runtime intelligence (Phase 8)

```
ServiceIdentity (Phase 3, reused — never duplicated)
      ↓  resolve_runtimes: AI-classified PIDs only
TrustedRuntime { pid, creation_ms, kind, endpoint }
      ↓  adapter per kind (Ollama / llama.cpp / ComfyUI / generic)
read-only GET over loopback (redirects disabled, 1 s connect / 2 s total)
      ↓
AiRuntimeSnapshot { health, version, capabilities, models, loadedModels,
                    resources, props, latency, structured error }
```

**Trust & endpoint policy.** A runtime exists only when the Phase 3
classifier marked a PID as an AI service **and** the listener table yields a
loopback bind. `listener_base_url` maps `0.0.0.0`/`::` to `localhost` (the
client connects over loopback, never to the wildcard) and only
`127.0.0.1`/`localhost`/`[::1]` pass `validate_endpoint` — private LAN,
public IPs, other schemes, and credentials in URLs are rejected before any
request. The HTTP client follows **no redirects** (a 302 to a non-loopback
host would otherwise be chased before any policy check).

**HTTP bounds (spec §8–9).** Connect timeout 1 s, request timeout 2 s,
body cap 2 MB enforced while streaming (Content-Length is not trusted),
≤ 500 models and ≤ 64 loaded models per snapshot, ≤ 4 concurrent probes.
AI inspection runs in its own Tauri blocking task on its own cadence — a
hung runtime costs its own 2 s, never the 20–40 ms discovery cycle.

**Cache cadence (spec §26).** Health + loaded models: 8 s. Model inventory:
45 s. Version: process lifetime. The runtime id is BLAKE3 over
(pid ‖ creation time ‖ kind ‖ endpoint ‖ per-boot key): a process restart
changes the id and orphans the old snapshot — no stale metadata. Manual
refresh bypasses all TTLs.

**Health semantics (spec §5, §14, §16).** `ready` requires adapter payload
evidence (a parsed `/api/tags`, `/api/ps`, `/api/version`, a llama.cpp
`{"status":"ok"}` or `loading model` body, ComfyUI `/system_stats`) — a TCP
listener alone is never READY. `loading`/`busy` come from payload text;
`degraded` means the runtime answered unexpectedly; `unavailable` means
connect/timeout; missing optional endpoints (version, metrics, props) are
**not** failures. Errors stay structured (`connect_failed`, `timeout`,
`http_status`, `malformed_json`, `too_large`, `policy_rejected`) with a
concise honest label for the UI.

**Adapters.** Ollama: `/api/version`, `/api/tags`, `/api/ps` (installed
models with family/params/quant/size; loaded models with reported VRAM and
best-effort `expiresAt` — never estimated). llama.cpp: `/health` (payload
examined, not assumed), `/props` (normalized context/slots subset),
`/v1/models` then `/models` (plain GETs only — metadata queries cannot
trigger model autoload), `/metrics` optional (404/501 ⇒ `metrics: false`, no
error banner). ComfyUI: `/system_stats` + `/queue` counts only — no workflow
contents, no submit/cancel/clear. Gradio/Open WebUI/unknown: HTTP
reachability only; no page scraping, no auth, no account/token data.

**Privacy (spec §42) & no-action guarantees.** Only model names and runtime
metadata are collected; prompts, conversations, cookies, tokens, and
accounts are never touched — and no inference request is ever sent, so no
such data can be generated. No pull/delete/load/unload/copy model, no
workflow submission, no configuration mutation. Nothing is transmitted off
the machine.

### Docker & container intelligence (Phase 9)

```
Docker Engine (Windows named pipe \\.\pipe\docker_engine)
      ↓  read-only GET allowlist (no CLI, no tcp://2375, no POST)
DockerEngineSnapshot { engine info, containers, projectLinks, portOwnerships }
      ↓                      ↓
Container → Compose → LocalStack Project   Published host port → container overlay
```

**Transport (spec §2–3).** A small bounded HTTP/1.1 client speaks directly
to Docker Desktop's named pipe `\\.\pipe\docker_engine` — no `docker.exe`
CLI parsing, no `tcp://localhost:2375` fallback, no `daemon.json` changes.
The pipe is opened with `CreateFileW`; the handle is owned by a single
`std::fs::File` (RAII close, no guard/double-close). Responses are read
under an 8 MB cap with a 3 s timeout; `Content-Length` and chunked bodies
are both handled.

**Read-only allowlist (spec §9, §51).** The only requestable paths are
`/_ping`, `/version`, `/info`, `/containers/json`,
`/containers/{id}/json`, `/containers/{id}/stats?stream=false` — checked
*before any I/O*. There is no frontend command carrying a method, path, or
pipe name; the surface is exactly `get_docker_snapshot`, `refresh_docker`,
and `get_container_details(trusted container id)`. Mutation endpoints
(stop/kill/create/exec/images) are refused by construction, and a test
proves it end-to-end against the live-shaped transport.

**Engine API versioning (spec §4).** `/version` is fetched once per engine
session (10 min cache) and its `Version`/`ApiVersion`/`Os`/`Arch` are
shown; the adapter uses only long-stable list/inspect/stats shapes, so a
newer daemon never breaks discovery.

**Container model (spec §5–7).** `DockerContainer` carries id/shortId,
name, image + imageId, normalized `state` (created/running/paused/
restarting/removing/exited/dead/unknown) and `health`
(healthy/unhealthy/starting/none/unknown). Health comes **only** from
Healthcheck evidence — inspect `State.Health.Status`, or a `(healthy)`-style
list `Status` suffix; a running container with no healthcheck is `none`,
never `healthy`.

**Ports (spec §11–14).** Published mappings keep host and container ports
distinct (`hostIp`/`hostPort` → `containerPort`/`protocol`), including
specific-IP binds and multiple mappings. UDP mappings are displayed as
Docker metadata only — the Phase 1 scanner is TCP and we do not pretend
otherwise. The `portOwnerships` index (one entry per published TCP host
port, with container/compose/project facts) overlays the Ports and Services
views: the host PID of the Docker proxy listener stays authoritative for
process facts, while the container supplies the ownership dimension a PID
cannot.

**Compose & project association (spec §15–20, §55–56).** Only canonical
labels are read (`com.docker.compose.project`, `.service`,
`.container-number`, `.project.working_dir`, `.project.config_files`); a
similar container *name* is never evidence. Association confidence:
`exact` (compose working dir == project root), `high` (bind mount ==
project root), `medium` (project root nested under a mount), `low`
(unique compose-project-name == project basename). Ambiguous name matches
(sibling projects) associate with **nothing**. Bind-mount paths are used
internally as evidence; contents are never opened.

**Cadence & bounds (spec §26–28).** Engine+list 5 s, stats 8 s,
version 10 min; manual refresh bypasses caches. Inspect/stats run only for
running containers; the per-container cache is keyed by container ID and
pruned the moment a container disappears. When the engine is away, an
exponential backoff (5 s doubling to 60 s) prevents pipe hammering.
Everything runs in `spawn_blocking` off the discovery loop.

**Typed errors (spec §29–30, §66).** `docker_unavailable` (honest state,
not an app error), `access_denied` (informational only — no elevation, no
ACL changes), `timeout`, `api_unsupported`, `malformed_response`,
`response_too_large`, `engine_error`. UI labels never contain pipe
internals.

**Privacy (spec §46–48).** `Config.Env` is never even parsed into an
intermediate structure; arbitrary labels, auth data, registry credentials,
and raw inspect payloads never cross to React (the DTO is the boundary, and
a fixture test proves secrets in the payload do not survive serialization).
Container logs are not ingested.

**No control (spec §44–45).** No start/stop/restart/kill/pause/remove/
create/exec/compose lifecycle exists anywhere in the adapter, the Tauri
commands, or the UI. Phase 9 observes, identifies, maps, and explains.

## Known limitations (Phase 2–9)

- Windows-only. Other platforms get an explicit error, not silent emptiness.
- TCP only — UDP discovery would be a separate, explicit design.
- Protected processes cannot be fully inspected by an unelevated process —
  expected, and rendered honestly (`accessible: false`). Full metadata for
  system processes is out of scope by design (no elevation).
- CPU percentages are per-refresh-window rates; instantaneous values will
  differ from other tools sampling at different moments (verified plausible
  and dynamic, not bit-identical).
- Working set is the chosen memory metric; private/commit memory
  (`PROCESS_MEMORY_COUNTERS_EX`) can be added later without breaking the
  contract.
- The bind address is shown verbatim (including link-local scope ids when the
  OS reports them); zone/scope qualifiers are future polish.
- IPv6 v4-mapped addresses render in their canonical `::ffff:a.b.c.d` form.
- Command lines are only as good as Windows allows: protected processes
  (and cross-bitness targets) yield `commandLine: null`, so their framework
  cannot be detected — they stay generic (svchost.exe → "svchost.exe",
  node-without-VM_READ → "Node.js").
- Classification is executable/command-line based; manifest content is used
  only for project naming/package-manager/start-command facts, never for
  service-identity classification.
- Express is only detected from its own CLI; `node server.js` stays Node.js
  by design. Open WebUI/Gradio need explicit command/path evidence — a mere
  port (7860) or process name is never enough.
- Project association requires command-line path evidence; a process whose
  command line is unreadable (protected) and whose executable lives in a
  non-project directory stays project-unknown — databases launched by
  services are the normal example (PostgreSQL → Unknown, by design).
- Git facts come from parsing `.git` (directory or worktree file) and `HEAD`;
  detached HEAD reports no branch. Between manual refreshes a branch switch
  is picked up on the next cache invalidation (command-line change or
  explicit refresh), not on the 3-second poll.
- A process mapping to no project is the honest outcome, not an error:
  `npm-cache/_npx` caches resolve (they have real package.json files) but
  render as opaque cache directories with their hash names.
- Control actions accept only backend-issued **opaque target ids** — no PID,
  creation time, or metadata crosses the Tauri boundary from the frontend;
  the registry is the native trust boundary (see §5.6).
- There is no generic targeted graceful console-stop for externally
  discovered processes (a console-wide `CTRL_BREAK` broadcast would affect
  unrelated processes and is never used); the honest action is a confirmed
  **End Process**. LocalStack-**managed** processes (Phase 6) do get a
  targeted `CTRL_BREAK` against their known process group.
- Stop targets exactly one process; children spawned by the dev server are
  intentionally not touched (no proven parent/child model yet — managed
  launches terminate only their root process, and npm.cmd wrappers may
  leave grandchildren alive after a stop; a proven tree model is future
  work).
- Managed-process state is in-memory only: after an app restart, previously
  managed services appear as external and are not re-adopted (documented
  Phase 6 boundary). App exit never kills managed services.
- Workspace service roles are evidence-derived (`frontend` for known dev
  servers, otherwise `other`); `database`/`ai`/`worker` roles are reserved
  for phases that can actually derive them.
- Logs are bounded in-memory rings (1,000 lines/service, not persisted).
  Environment variables are inherited, never displayed or logged.
- Conflict evaluation predicts bind compatibility from the TCP table only;
  dual-stack behavior cannot be verified without actually binding, so it is
  always reported as `potential`, never safe.
- Dependency edges exist only when a user (or backend metadata) declared
  them — no convention-based inference ("port 5432 ⇒ PostgreSQL dependency")
  is performed, so a workspace with no declared dependencies shows none.
- Dependency `available` means *listening*; real HTTP health checks are a
  later phase and are never faked.
- History is per-session and transition-deduped; repeated identical issues
  do not spam the log, but there is no persistence yet.
- AI probes are GET-only against a fixed set of approved paths per adapter;
  runtimes behind authentication, runtimes exposing metadata only on
  non-standard paths, and HTTPS-on-loopback runtimes degrade to reachability
  or `unavailable` rather than being guessed at.
- `expiresAt` for Ollama models is parsed best-effort (UTC RFC3339 only);
  other formats stay unknown instead of being guessed.
- GPU telemetry beyond what a runtime itself reports (driver-level stats) is
  out of scope by design in Phase 8.

## Unsafe FFI audit (Phase 10B)

### Local diagnostics (Phase 10B)

LocalStack writes a bounded, local-only diagnostic log to
`%LOCALAPPDATA%\localstack-control-center\localstack.log`
(`FOLDERID_LocalAppData`; no roaming, no admin). Properties:

- **Session-bounded** (2,000 lines) and best-effort — a logging failure can
  never fail the app.
- **Redacted**: every line passes a deterministic redactor covering
  `key=value` / `key:value`, `Authorization: Bearer …` (all splits), bare
  `Bearer <token>`, and `--api-key/--token/--password <value>` forms.
- **Panic hook**: panic payloads pass the same redaction path; only the
  message + source location are recorded, then the default hook runs. No
  telemetry, no network, no crash service.
- **No command lines**: production diagnostics carry fixed strings + typed
  codes only (subsystem, error class) — never process argv, environment
  variables, Docker `Config.Env`, prompts, or cookies. Managed-service
  stdout/stderr goes to the UI's bounded log rings, not the diagnostics
  file.

### Workspace registry poison policy (Phase 10B correction)

The launch-spec and managed-process registries participate in
**process-control mutations**, so a poisoned lock quarantines lifecycle
control (sticky flag): start/stop/restart/force refuse with the typed
`WORKSPACE_STATE_UNAVAILABLE` outcome before any spec resolution, PID read,
or FFI call — no fallback to raw PID, executable name, or frontend
metadata. Read paths recover the guard (views stay alive). Recovery
rebuilds a NEW clean map (specs re-derive on next create/refresh; managed
entries become External — the documented restart semantic) before control
returns. The external **control-target registry** (Phase 5) remains fully
fail-closed on poison: resolution returns `Unknown` and no new targets are
issued.

All `unsafe` code is confined to the Windows FFI boundary modules. Every
block carries a `SAFETY:` comment stating its ownership invariant. The
complete inventory:

| Module | Unsafe surface | Purpose / justification | Key invariants |
|---|---|---|---|
| `discovery/windows.rs` | `GetExtendedTcpTable` calls | Read the OS TCP listener tables (the foundation of all discovery) | Buffer sized via probe call; truncated tables rejected, not trusted |
| `process/windows.rs` | `OpenProcess`, `GetProcessTimes`, `QueryFullProcessImageNameW`, `ReadProcessMemory` + `NtQueryInformationProcess`, `GetProcessMemoryInfo`, ToolHelp snapshots | Per-PID process intelligence (identity, CPU, memory, command line) | `PROCESS_QUERY_LIMITED_INFORMATION` minimum rights; handle wrapped in an RAII guard (closes exactly once); PID + creation-time identity checked after every handle acquisition (PID-reuse guard) |
| `control/windows.rs` | `OpenProcess`, `TerminateProcess` | Confirmed End Process for eligible external targets | Registry resolution → re-inspection → identity revalidation → policy recomputation *before* the unsafe call; terminate right is requested only at action time |
| `workspace/windows.rs` | `CreateProcessW` + attribute list, pipe `CreatePipe`, `WaitForSingleObject`, `GetExitCodeProcess`, targeted `GenerateConsoleCtrlEvent`, `TerminateProcess` | Managed-service launch and lifecycle | `CREATE_NEW_PROCESS_GROUP` so the group id equals the root PID; group-0 broadcast impossible (never constructed); child pipe handles moved as integers and reconstructed in the reader thread (owned exactly once); wait/exit/terminate always revalidate the stored creation time |
| `docker/transport.rs` | `CreateFileW` on `\\.\pipe\docker_engine` | Named-pipe transport to the Docker Engine (read-only allowlist) | Handle owned by a single `std::fs::File` (RAII, no double close); 8 MB streaming cap; all failures mapped to typed errors, never unwrapped |
| `diagnostics.rs` | `SHGetKnownFolderPath` + `CoTaskMemFree` | Resolve the per-user log directory | Path buffer length computed before conversion; PWSTR freed exactly once; any failure silently disables logging (diagnostics never crash the app) |

Notes:

- No `unsafe` exists outside these modules (enforced by review; the static
  guard tests pin the *behavioral* boundaries: no raw-PID commands, no
  group-0 console events, no generic exec/HTTP/Docker primitives).
- No `unsafe impl Send/Sync` anywhere; raw `HANDLE` (a pointer type, not
  `Send`) is deliberately moved across threads as an `isize` and
  reconstructed inside the owning thread with an exclusive-ownership
  comment (workspace pipe readers).
- Integer conversions in the FFI layer (`FILETIME` ticks → Unix ms, CPU
  tick deltas, memory sizes) use checked/saturating arithmetic; malformed
  or nonsensical local values degrade to `None`/0 rather than panicking.

## Application shell (Phase 10C)

Settings, tray, single instance, and close behavior complete the desktop
shell without touching the service-control security model:

- **Settings** (`src-tauri/src/settings.rs`) — one normalized model with
  server-side defaults, validation, and clamping. Persisted atomically
  (temp file + rename) to `%LOCALAPPDATA%\localstack-control-center\settings.json`
  — outside the source tree. Missing/empty/corrupt/wrong-typed files and
  unknown future fields recover to validated defaults with a diagnostics
  entry; startup never crashes on bad settings.
- **Polling integration** — `autoRefresh`, the port interval, and the
  AI/Docker toggles flow from the settings store through the Phase 10A
  polling-owner `configure()`: disabling keeps consumer leases, an interval
  change recreates exactly one timer, and manual refresh is always available.
- **Tray** — Open (show/restore/focus the single window), Refresh
  (read-only discovery refresh; never starts/stops/kills anything), Exit
  (exits LocalStack only; managed/external services keep running).
- **Close behavior** — `exit` (default) or `minimize_to_tray`; the window
  lifecycle never implies a service lifecycle. Launch-minimized hides the
  main window at startup but falls back to showing it if tray
  initialization fails — the app is never left inaccessible.
- **Windows startup** — opt-in HKCU `Run` registration via the autostart
  plugin (no elevation, reversible, `--startup` argument honored). The
  settings view reports the OS registration state, and stale registrations
  are reconciled/removed at startup rather than lied about.
- **Single instance** — a second launch signals the first (window shown and
  focused) and exits; it never re-adopts managed processes, restarts
  polling stacks, or creates a second tray.

## Diagnostics & supportability (Phase 11C)

Module map (all under `src-tauri/src/diagnostics/` unless noted):

- `mod.rs` — Phase 10B logging facade (info/warn/error), bounds, panic hook
  delegation; unchanged call-site compatibility.
- `incidents.rs` / `policy.rs` — typed incidents, deterministic
  fingerprinting (subsystem+code+operation+severity), bounded history
  (500 global / 100 per subsystem), severe 2-in-2-min + critical capture
  policy with 10-minute cooldown.
- `worker.rs` — single capture worker on `std::sync::mpsc::sync_channel(8)`;
  producers never block (`try_send`), duplicates coalesce, per-job
  `catch_unwind` isolation, cooperative shutdown.
- `cache.rs` — non-blocking (try-lock) runtime snapshot cache that both the
  panic hook and bundle builders read; never locks live registries.
- `emergency.rs` — panic-path emergency record (≤512 KiB): pre-prepared path
  only, no locks, no OS queries, no DPAPI/ZIP; degrade-to-omit on any miss.
- `recovery.rs` — startup finalization of emergency records into full
  encrypted bundles; ≤3 attempts, then `failed/` + UI banner.
- `crypto.rs` — Windows DPAPI current-user (`CryptProtectData`/
  `CryptUnprotectData`, checked length conversion, `LocalFree` ownership).
- `store.rs` — bundle registry/index transaction: temp → finalize → atomic
  index commit → best-effort eviction of tombstoned old bundles; tombstones
  prevent crash-window resurrection; startup reconciliation of orphans and
  stale temp files; all deletes are per-trusted-file with reparse refusal.
- `bundle.rs` / `collectors.rs` — versioned bundle model and bounded
  collectors (health, identity, inventories, LogRing managed-output tails).
- `health.rs` — deep health probes (internal / integrations / Windows),
  timeout-isolated, non-elevated, read-only.
- `export.rs` / `redact_export.rs` — structural privacy transforms over the
  typed bundle model (Safe Share / Developer Detail / Full Forensics), ZIP
  export via trusted native save, one-shot reveal capability (8 entries,
  10-minute TTL, restart-invalidated).
- `commands.rs` — the narrow Tauri command surface (opaque IDs only) and the
  production wiring of state + capture worker.
- `notify.rs` — foreground/background notification decision (query failure
  → conservative in-app banner).

Frontend: `src/features/diagnostics/` (page + tabs + dialogs),
`src/stores/diagnosticsStore.ts`, `src/services/native/diagnostics.ts` —
components never call Tauri directly.

Data flow: subsystem reports typed incident → policy decides capture →
worker builds the bundle from cached snapshots → DPAPI encrypt → registry
commit with tombstoned retention → user-initiated export applies the chosen
privacy profile → Safe Share summary/URL for the fixed GitHub issue flow.
No telemetry, no automatic upload, no network egress from diagnostics.

## See also

- `docs/roadmap.md` for the phase plan.
