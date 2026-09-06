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
│  discovery · health · control · conflicts · workspace · ai
├─────────────────────────────────────────────────────────┤
│      Native engines (OS APIs, Windows-first)            │
│  process/port enumeration, filesystem inspection, ...   │
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
  the active view, `portsStore` holds **real** listener data from the native
  engine, and the dashboard store holds mock data for still-mock surfaces.
- Hooks (`src/hooks/`) are the only place components start side effects.
  `usePortListeners` performs the initial load and the ~3-second automatic
  refresh; `useMockLiveUsage` simulates usage until Phase 2.

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
  `ServiceStatus`, `PortConflict`, `Workspace`, `SystemUsage`, `ViewId`.
- These types are the **contract** with the backend: from Phase 1 on, Tauri
  commands return serde DTOs that serialize into exactly these shapes. Phase 0
  deliberately keeps them minimal — fields are added when an engine actually
  produces the data, not before.

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

| Command              | Returns                              | Phase |
|----------------------|--------------------------------------|-------|
| `greet`              | sample string (boundary smoke test)  | 0     |
| `get_port_listeners` | `PortListenersResponse` (read-only)  | 1     |

## 5. Rust feature modules

`src-tauri/src/` mirrors the product's future engines. Each module owns one
responsibility:

| Module       | Responsibility                                        | Phase |
|--------------|-------------------------------------------------------|-------|
| `discovery`  | Enumerate listeners, map to processes, classify services | 1–3  |
| `health`     | Localhost health probes and per-service health state   | 2+    |
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

Rules for the modules:

- OS-specific code stays inside `discovery`/`control` behind platform-agnostic
  facades, so the rest of the app depends on interfaces, not on `netstat`
  output.
- Modules expose `pub` items only when an engine exists.

## Data flow (implemented for port discovery, Phase 1)

```
Rust: windows.rs (FFI) ──► ports.rs (normalize) ──► mod.rs (DTO)
                                                          │
React ◄─ stores/portsStore ◄─ hooks/usePortListeners ◄─ services/native/ports.ts
                                                          │
                                                   invoke('get_port_listeners')
```

### Refresh behavior (Phase 1)

- **Automatic:** while the Dashboard or Ports view is mounted, listeners are
  refreshed every ~3 seconds (`usePortListeners`). Timers are cleaned up on
  unmount; the store drops overlapping requests, so a slow poll never stacks.
- **Manual:** a Refresh button is present on both views and goes through the
  same store action.
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

## Known limitations (Phase 1)

- Windows-only. Other platforms get an explicit error, not silent emptiness.
- TCP only — UDP discovery would be a separate, explicit design.
- PIDs are reported but not yet resolved to process names/executables
  (Phase 2). System-owned sockets (PID 0/4) are shown as-is.
- The bind address is shown verbatim (including link-local scope ids when the
  OS reports them); zone/scope qualifiers are Phase 2 polish.
- IPv6 v4-mapped addresses render in their canonical `::ffff:a.b.c.d` form.
- The System Usage card and Services preview on the dashboard are still mock
  data, labeled as such.

## See also

- `docs/roadmap.md` for the phase plan.
