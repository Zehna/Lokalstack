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
│  discovery · process · intelligence · project ·         │
│  health · control · conflicts · workspace · ai          │
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
  require elevation. Anything beyond read-only localhost inspection needs an
  explicit, user-confirmed design decision in a later phase.

### 4.1 Registered commands

| Command              | Returns                                        | Phase |
|----------------------|------------------------------------------------|-------|
| `greet`              | sample string (boundary smoke test)            | 0     |
| `get_port_listeners` | `PortListenersResponse` — listeners + process intelligence + service identities + project resolution, read-only | 1–4  |

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
| `workspace`  | Group services into development workspaces             | 6     |
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
identities) + `projectLinks` (PID → project id); the frontend maps them to
`projectByPid`. One project shared by many PIDs (vite + the desktop binary)
references one identity; unrelated sibling projects stay separate because
the association requires root-marker evidence, not a shared parent folder.

**Caching.** Resolution is content-addressed on *(executable path, command
line)* — exactly the inputs that determine the outcome. Warm cycles do zero
filesystem work; the cache is bounded (cleared at 256 entries). Invalidation:
command-line change, cache eviction, or the manual Refresh button, which
passes `bypassProjectCache: true` to re-read markers, manifests, and the Git
branch from disk. Between manual refreshes a branch change is picked up on
the next invalidation — a documented trade-off that keeps 3-second polling
filesystem-free.

## Data flow (implemented for port + process + service + project discovery, Phase 1–4)

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

## What stays out of scope by design

- No generic network/port scanning of remote or external targets — localhost
  only, always.
- No privileged operations. If a future feature seems to need elevation, the
  feature gets redesigned, not the permission model.
- No process modification without an explicit, user-confirmed action in the UI
  (Phase 5 will define that UX; nothing exists today).

## Known limitations (Phase 2–4)

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

## See also

- `docs/roadmap.md` for the phase plan.
