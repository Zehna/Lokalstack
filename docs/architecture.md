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
│  discovery · process · health · control · conflicts ·   │
│  workspace · ai                                         │
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
  native engine — listeners, the per-PID `processByPid` map, cycle duration,
  loading/error/lastUpdated. There is no mock store; every rendered row is
  native data.
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
| `get_port_listeners` | `PortListenersResponse` — listeners + process intelligence, read-only | 1–2  |

## 5. Rust feature modules

`src-tauri/src/` mirrors the product's future engines. Each module owns one
responsibility:

| Module       | Responsibility                                        | Phase |
|--------------|-------------------------------------------------------|-------|
| `discovery`  | TCP listener enumeration (`GetExtendedTcpTable`)        | 1    |
| `process`    | Process intelligence: names, paths, CPU/RAM, start time | 2    |
| `health`     | Localhost health probes and per-service health state   | 3+    |
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

#### 5.3 Refresh behavior

## Data flow (implemented for port + process discovery, Phase 1–2)

```
Rust: discovery/windows.rs (TCP FFI) ──► ports.rs (normalize) ─┐
                                                               ├─► process/mod.rs (DTO)
Rust: process/windows.rs (OpenProcess FFI) ──► sampler.rs ─────┘        │
                                                                        │
React ◄─ stores/portsStore ◄─ hooks/usePortListeners ◄─ services/native/ports.ts
                                                                        │
                                                       invoke('get_port_listeners')
```

### Refresh behavior (Phase 1–2)

- **Automatic:** while the Dashboard, Ports or Services view is mounted, a
  full cycle (listeners + process sampling) runs every ~3 seconds
  (`usePortListeners`). Timers are cleaned up on unmount; the store drops
  overlapping requests, and the Rust-side cache mutex serializes cycles as
  defense in depth. A cycle on a typical dev machine takes ~20 ms, so the
  3 s cadence is comfortably non-overlapping.
- **Manual:** a Refresh button is present on Dashboard, Ports and Services
  and goes through the same store action.
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

## Known limitations (Phase 2)

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
- Framework/service identities are deliberately not implemented — Phase 3.

## See also

- `docs/roadmap.md` for the phase plan.
