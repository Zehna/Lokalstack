# Phase 11C — Diagnostics & Supportability — Design Specification

**Status:** Implemented (Phase 11C gates passed).
**Date:** 2026-09-18
**Baseline:** main `c714f9c48d694bb9a33014b5420da1b7542c9ce5` (v1.0.1 candidate)
**Predecessor:** Phase 10B local diagnostics (`docs/architecture.md` §"Local diagnostics (Phase 10B)"), Phase 10D bounds (`docs/phase-10d.md`)

---

## 1. Goal

Give LocalStack Control Center a first-class, safety-preserving diagnostics and
supportability capability: persistent typed incident history, deep health
checks, automatic severe-failure capture into DPAPI-encrypted local support
bundles, crash recovery, bounded retention, and three privacy-graded export
profiles with a GitHub issue preparation workflow — without weakening any
Phase 1–11B safety boundary.

The north-star user story: *something is wrong with LocalStack or the
environment it observes; the user can understand what, and can share exactly
as much evidence with a maintainer as they choose to — never more.*

## 2. Non-Goals

- No telemetry; no automatic/background network egress for diagnostics; no
  automatic support-bundle upload; no cloud crash service. The only
  network-adjacent action is an explicit user-driven open of LocalStack's
  fixed GitHub new-issue URL, transmitting only the already
  Safe-Share-transformed issue title/body (§17.0, §29) — the support bundle
  itself is never uploaded automatically.
- No automatic crash upload, cloud diagnostics, or remote support service.
- No automatic GitHub file upload; no PAT storage; no OAuth for support.
- No automatic repair / self-healing; no automatic process killing.
- No Docker mutation (Docker integration remains strictly read-only).
- No AI model inference or AI mutation (AI integration remains read-only,
  loopback-only observation).
- No remote-machine discovery; no LAN scanning.
- No Linux/macOS support (Windows-first, as always).
- No auto-updater work.
- No version bump, no v1.0.2 tag, no GitHub Release (release engineering is
  outside this phase).
- Diagnostics must **not** become a second control plane: it observes and
  records; it never issues lifecycle commands that existing features own.
- No elevation requirement anywhere in the diagnostics flow.
- No firewall, ACL, Windows service, or unrelated registry mutation.
- Official release packaging rules are unchanged and out of scope.

## 3. Preserved Safety Architecture (invariants)

Every design decision below defers to these, restated as binding:

1. **Windows-first.** All new native behavior is Windows; the existing
   `#[cfg(windows)]` / non-Windows no-op patterns are followed.
2. **React has no direct OS access.** The frontend talks only to narrow
   typed Tauri commands. Every OS touch lives behind Rust.
3. **External and managed processes remain distinct.** Diagnostics may
   *record* evidence about both; it may never control either beyond what
   existing features already do.
4. **No arbitrary raw-PID lifecycle APIs.** No `kill(pid)`, no generic
   process-handle surfaces.
5. **No arbitrary filesystem APIs from the frontend.** No
   `read_file(path)`/`delete_file(path)`-shaped commands; opaque IDs only.
6. **No arbitrary URL opener.** The only URL navigation diagnostics ever
   performs is LocalStack's own fixed GitHub new-issue URL.
7. **Docker integration remains read-only.** Diagnostics may copy
   already-collected read-only Docker state into bundles; it never issues
   new Docker requests beyond existing read-only polling shapes.
8. **AI runtime integration remains read-only and loopback-only.**
9. **No telemetry.** No automatic/background diagnostics network egress; no
   automatic support-bundle upload; no telemetry; no cloud crash service.
   Existing read-only loopback observability remains allowed. Explicit user
   action may open LocalStack's fixed GitHub new-issue URL, transmitting
   only the already Safe-Share-transformed issue title/body; the support
   bundle itself is never uploaded automatically.
10. **No elevation.** All checks run as the current unprivileged user.
11. **Diagnostics is not a control plane.** It reads state owned by other
    subsystems; it never mutates app behavior, settings, or services.

## 4. Existing Baseline (compatibility contract)

Phase 10B shipped `src-tauri/src/diagnostics.rs` (396 lines, 382-test suite
green on top of it). Phase 11C treats the following as **fixed contract**:

- Log file: `%LOCALAPPDATA%\localstack-control-center\localstack.log`
  (via the existing `local_app_data_dir()` helper shared with settings).
- Bounded local-only logging: `MAX_SESSION_LINES = 2_000` per session;
  cross-session file cap `MAX_FILE_BYTES = 256 KB` enforced by truncating an
  oversized file **on open** (Phase 10D §S).
- Deterministic secret redaction: the single-pass `redact()` with
  `SECRET_KEY_HINTS` (token/secret/password/authorization/cookie/api_key/
  private_key/credential shapes), covering `key=value`, `key: value`,
  `Authorization: Bearer …`, bare `Bearer …`, and `--flag value` forms.
- Best-effort logging: any logging failure is silently absorbed; logging
  must never crash the app or the calling subsystem.
- Existing panic hook: records redacted panic message + location, delegates
  to the previous hook, no OS queries, no network.
- Call-site compatibility: 27 call sites across `app_commands.rs`, `lib.rs`,
  `settings.rs`, `workspace/mod.rs`, `workspace/registry.rs`
  (12 × `info`, 10 × `warn`, 3 × `error`). If the module is refactored into
  a directory module, a facade/re-export (`pub use` in
  `diagnostics/mod.rs`) MUST keep every existing call site compiling
  unchanged. The migration is mechanical and must not alter any call-site
  behavior.

`localstack.log` stays at its documented path in Phase 11C (see §34
Migration).

## 5. Architecture Overview

A modular **Diagnostics Engine** on the Rust side and a dedicated frontend
feature, separated by the existing Tauri trust boundary.

### 5.1 Rust module boundaries

`src-tauri/src/diagnostics.rs` becomes `src-tauri/src/diagnostics/`:

```
src-tauri/src/diagnostics/
  mod.rs        # facade: re-exports info/warn/error (+ local_app_data_dir),
                # module wiring; keeps all 27 existing call sites compiling
  logger.rs     # existing bounded logger, moved verbatim
  redact.rs     # existing deterministic redactor, moved verbatim; extended
                # with profile-aware path/identity transforms (§17)
  incidents.rs  # typed incident API, fingerprints, coalescing, thresholds
  health.rs     # deep-health engine (§22), probe registry, timeout isolation
  crash.rs      # panic-hook emergency records + startup recovery flow
  snapshot.rs   # cached runtime snapshots (listeners/services/projects/
                # workspaces/subsystem status) prepared during normal op
  bundle.rs     # versioned support-bundle model, collectors, manifest
  crypto.rs     # DPAPI encrypt/decrypt (current-user scope), integrity
  retention.rs  # storage limits, oldest-first eviction, safe deletion
  export.rs     # export profiles (Safe Share / Developer Detail / Full
                # Forensics), ZIP assembly, summary generation
  commands.rs   # the narrow Tauri command surface (§25)
```

Filenames are illustrative; the binding requirement is **separation of
responsibilities** — fingerprinting, capture policy, packaging, encryption,
retention, and export are independently testable units with narrow interfaces.

Non-Windows builds: `crypto.rs` and Windows-identity collectors compile to
explicit unsupported stubs behind `#[cfg(windows)]` (matching the existing
`local_app_data_dir()` precedent), so `cargo check` stays green on any host.

### 5.2 Frontend layering (existing architecture preserved)

The diagnostics feature follows the established frontend layers — features
never call Tauri `invoke` directly; native clients live in
`src/services/native/`; Zustand application state lives in `src/stores/`;
wire/domain contract types are centralized in `src/types/domain.ts`:

```
src/features/diagnostics/
  DiagnosticsPage.tsx      # page shell: Overview / Health / Incidents /
                           # Bundles sections (tabbed or stacked per plan)
  components/              # HealthGroup, IncidentRow, BundleRow,
                           # ExportDialog, ForensicsConfirm, Banner, …
  (optional UI-only helpers/types)   # never the native wire contract

src/stores/
  diagnosticsStore.ts      # zustand store, existing store pattern
                           # (imports the native client; never invoke)

src/services/native/
  diagnostics.ts           # the only module importing diagnostics Tauri
                           # commands; typed wrappers, no other OS access

src/types/domain.ts        # diagnostics wire/domain DTO types added to
                           # the existing centralized domain-type module
```

Binding rules (consistent with the existing `invoke(` boundary, which
today resolves only to `src/services/native/` and is enforced by
`safetyGuards.test.ts`):

- components/features **must not** call Tauri `invoke` directly; a search
  for `invoke(` must continue to find only the trusted native-client
  directory;
- the diagnostics API wrapper lives exclusively under
  `src/services/native/`;
- Zustand application state belongs under `src/stores/`;
- feature-local types are allowed only when they are UI-only view models —
  never the native wire contract.

**"Opaque IDs, no internal paths" clarified:** internal diagnostics storage
paths (index, bundle files, emergency markers) never cross to the frontend —
the backend resolves opaque IDs to verified LocalStack-owned paths (§20).
Diagnostic *data* such as a full executable or project path MAY appear in
an explicitly requested Developer Detail / Full Forensics detail DTO
(§17); receiving a display-only diagnostic path string is not filesystem
authority, and passing it back confers no access.

Navigation: one new `NAV_ITEMS` entry in `src/app/navigation.ts`
(`{ id: 'diagnostics', label: 'Diagnostics', feature: 'diagnostics' }`) with
the `FeatureGroup` union extended. Everything else reuses existing layout,
a11y, and test conventions (colocated `.test.tsx`).

### 5.3 Data flow

```
subsystem error ──▶ report_incident(…)            [incidents.rs]
                       │  (short lock: counters + decision only)
                       ├─▶ log line via existing logger (redacted)
                       └─▶ CaptureDecision::Capture ──▶ bounded queue
                                                            │
                                        capture worker ─────┘ (async, §10)
                                            │ collectors (timeout-isolated)
                                            ▼
                              versioned bundle → DPAPI encrypt →
                              atomic persist → retention commit (§33)
                                            │
                     index.json update ─────┘
                                            ▼
                     context-aware notification (§28) → Diagnostics UI
```

Crash path is deliberately *not* in this flow: the panic hook writes only a
bounded emergency record (§11); full capture happens on next startup (§12).

## 6. Core Data Model

Three levels: **DiagnosticEvent → Incident → SupportBundle**.

### 6.1 DiagnosticEvent (in-memory, ephemeral)

A single observation: subsystem, typed code, severity, operation, timestamp,
fingerprint. Events never persist individually; they fold into incidents.

### 6.2 Incident (persistent)

```
IncidentRecord {
  id: opaque incident ID            # collision-resistant and opaque; never
                                    # derived from sensitive content (§7);
                                    # exact generation mechanism (existing
                                    # deps / native APIs) chosen in the plan
  fingerprint: stable fingerprint   # §7
  subsystem: SubsystemId            # enum: discovery, docker, ai, control,
                                    # settings, workspace, project, tray,
                                    # capture, health, …
  code: typed code                  # e.g. "docker.pipe_unavailable"
  operation: OperationTag           # e.g. "snapshot", "settings-load"
  severity: Severity                # info | warning | severe | critical
  first_seen / last_seen: timestamp
  occurrence_count: u32
  summary: redacted short summary   # ≤ ~200 chars, redaction applied
  review_state: New | Reviewed
  trigger_reason: RecentSevere | Critical | ManualCapture | StartupRecovery
                                  # reason for the most recent capture
  related_bundle_ids: [opaque ID]   # bundles coalesced under this incident
}
```

Persistent storage: `diagnostics/index.json` (incidents + bundle registry),
bounded:

- **max 500 incident records globally**;
- **max 100 records per subsystem**.

Overflow eviction is **deterministic**:

1. enforce the per-subsystem cap (100) first;
2. then the global cap (500);
3. eviction candidate ordering, at each enforcement step, is deterministic:
   **severity lowest first** (info → warning → severe → critical), then
   **last_seen oldest first**, then **opaque ID as stable tie-breaker**.

Lower-severity history is thus discarded before severe/critical history
when a lower-severity candidate exists, while the structure always remains
bounded even if all remaining incidents are critical. `occurrence_count`
uses saturating arithmetic (overflow-safe, pinned at `u32::MAX`).

### 6.3 Persistent metadata prohibition

Incident/index metadata must **never** contain:

- stdout/stderr blobs;
- stack traces;
- environment variables;
- raw command lines;
- machine identity (SID, MachineGuid, MAC, serials) — those live only inside
  encrypted bundles (§14);
- arbitrary raw exception payloads (only redacted summaries).

Detailed payload belongs exclusively in **encrypted support bundles**.

### 6.4 SupportBundle (persistent, encrypted)

```
SupportBundleMeta {
  id: opaque bundle ID              # collision-resistant, opaque, never
                                    # derived from sensitive content; also
                                    # the filename stem (§20)
  created_at: trusted timestamp     # index-recorded at creation; see §33
  trigger: RecentSevere | Critical | Manual | StartupRecovery
  subsystem / severity / fingerprint
  encrypted_size: bytes
  encrypted_storage_id              # bundle-<opaque-id>.lsdiag
  review_state: New | Reviewed
  integrity_state: Valid | Corrupt | Unsupported | RecoveryRequired
                                  # §21: four-state model; "Corrupt" means
                                  # the encrypted payload itself failed
                                  # DPAPI/integrity — index loss alone
                                  # never marks a valid bundle Corrupt
  app_version
  bundle_schema_version
}
```

## 7. Incident Fingerprinting

**Purpose:** the same logical problem coalesces into one incident;
PIDs/paths/timestamps changing must not spawn new incidents; no
secret-bearing raw text can ever become part of a fingerprint.

Fingerprint = hash over **normalized stable fields only**:

- subsystem (enum value)
- typed error code/class
- operation tag
- stable component identity (e.g. managed-service *ID*, not its path or argv)
- normalized severity

Explicitly excluded from fingerprint input: PID, timestamp, username,
hostname, full path, ephemeral port, raw message, stack trace, credentials,
any secret-shaped token. The normalized-field assembly runs the existing
`redact()` over each free-text field *before* hashing as defense-in-depth.

Hash function: **blake3** (`blake3 = "1"` is already a direct dependency —
used today for safety-guard integrity hashing). No new crypto dependency.
Output: full 256-bit, hex-encoded in metadata.

## 8. Typed Incident Reporting & Severity Model

Existing `diagnostics::error(…)` call sites **remain plain log lines** — they
do not automatically become incidents. Reporting an incident is an explicit,
typed act:

```rust
diagnostics::report_incident(
    subsystem,   // enum
    code,        // typed, closed set per subsystem
    severity,    // Info | Warning | Severe | Critical
    operation,   // tag
    summary,     // free text; redacted before storage
)
```

Severity rules:

| Severity  | Behavior |
|-----------|----------|
| `info`    | Recorded; never triggers capture. |
| `warning` | Recorded; **never** automatically creates a bundle. |
| `severe`  | Recorded; participates in the 2-in-2-minute threshold (§9). |
| `critical`| **Bypasses the threshold** and requests capture immediately; still passes through cooldown/anti-spam (§9). |

Panic and unrecoverable startup failures are **critical paths** by
definition (§11/§12).

## 9. Automatic Severe Capture Policy

Per `(subsystem, fingerprint)` key:

- **2 severe events within 2 minutes → create one support bundle.**
- After capture: **10-minute cooldown** for that key. During cooldown the
  incident continues to update (occurrence count, last seen, history) but no
  new bundle is created for it.
- Repeated events coalesce: 100 matching severe events in a few seconds
  produce **one incident with occurrence_count 100** and **at most one
  capture** inside the cooldown window.
- **Critical** incidents bypass the 2/2min threshold but still pass
  cooldown/anti-spam: a burst of criticals produces one capture, then
  cooldown-bounded further captures, never a capture storm.

All threshold/cooldown logic is unit-testable pure decision logic: given
event history + now → `CaptureDecision { Capture, Coalesce, Suppress }`.
The decision function never touches the filesystem.

## 10. Capture Worker / Non-Blocking Behavior

Bundle generation must **never block the reporting subsystem**.

- One bounded background capture queue: **one worker, capacity exactly 8**.
- Producers (`report_incident` callers) enqueue a request and return
  immediately; the producer never blocks indefinitely — a full queue drops
  the capture request and emits **one bounded diagnostic breadcrumb**
  ("capture queue full; capture suppressed for fingerprint X") into the
  incident record, never an error to the caller, never an app failure.
- Duplicate fingerprint requests coalesce in-queue (a queued request for key
  K makes a new request for K a no-op).
- **Lock discipline:** the incident tracker lock is held only to update
  counters/state and compute the `CaptureDecision`, then released. Bundle
  building, ZIP assembly, DPAPI calls, and collector execution all happen
  **outside** any incident-tracker lock. The registry/index lock (§33) is a
  separate lock, never held while a collector runs.

Implementation note: tokio is already a dependency; the worker may be a
tokio task or a dedicated thread — the plan picks one after inspecting how
collectors mix blocking FS/DPAPI work with async (recorded decision, §43).

## 11. Panic / Crash Architecture

**Approved mode: "maximum safe" hybrid.** The panic hook may include
pre-prepared cached snapshots — and nothing else.

The panic hook must **not**:

- run new OS queries (no Docker queries, no AI runtime queries, no fresh
  listener scans, no `SHGetKnownFolderPath`, no directory discovery, no
  registry lookup, no large directory-tree creation);
- perform frontend IPC of any kind;
- enumerate large filesystem areas;
- ZIP files; call DPAPI;
- acquire risky subsystem locks (settings, registries, docker client, AI
  registry) — and, per §11.1, **never wait for any Mutex/RwLock**,
  diagnostics-owned or otherwise;
- serialize unbounded structures.

The hook writes only a **bounded emergency crash record** containing, when
safely available:

- schema version, timestamp, app version;
- redacted panic summary + source location;
- error fingerprint (same normalization as §7);
- active frontend view — only if already cached via the normal-runtime
  context mechanism (§11.2); if no cached value exists, omit;
- **last 50 diagnostic log lines** (read from the in-memory session ring —
  see below — not from disk);
- compact **cached** listeners (≤100), services (≤100), projects (≤50),
  workspaces (≤50), and a cached subsystem-status summary.

**Snapshot provenance — non-blocking reads only:** `snapshot.rs` maintains
these caches *continuously during normal operation* from data the owning
subsystems already produce (read-only observation, unchanged). The panic
hook reads cached crash context **only through non-blocking mechanisms** —
an `Arc`-swapped immutable snapshot (atomic pointer swap), `try_lock()` on
a lock-protected cache, or another demonstrably non-blocking read path.
It never asks a live registry for fresh state. **If a cache is busy,
poisoned, missing, or unavailable, that section is simply omitted** —
omission is always preferred over waiting. The recent diagnostic-log ring
is subject to the same rule (§11.1): a contended log-tail cache is omitted,
never awaited.

**Session log ring:** the logger additionally retains its last 50 lines in
memory. Reads of this ring on the panic path use the same non-blocking
rule — a lock-free ring or `try_lock` snapshot; the emergency writer must
**not** depend on acquiring the normal log-file mutex (the existing
`LOG_FILE` `Mutex<File>` behind `OnceLock`) or any logger write-path lock
in order to produce the crash marker. This is a logger-internal extension;
file format and caps are unchanged.

**Hard bounds (exact, enforced in code, tested — see §19 constants):**

| Field | Bound |
|---|---|
| recent log lines | 50 (max) |
| cached listeners | 100 (max) |
| cached services | 100 (max) |
| cached projects | 50 (max) |
| cached workspaces | 50 (max) |
| emergency crash record | 512 KiB = 524,288 bytes (max) |

The record is written **atomically** (temp + rename) to
`diagnostics/emergency/emergency-<timestamp>.json`. Writing is best-effort:
a smaller valid crash marker is strictly better than risking deadlock. If
emergency storage was not successfully prepared during normal runtime
(§11.1), panic capture degrades safely to the existing redacted panic
breadcrumb (or no emergency marker) rather than attempting risky recovery
work inside the hook. After writing, the previous hook runs (unchanged
delegation).

### 11.1 Emergency storage is prepared during NORMAL runtime

The panic path itself must not need to: resolve known folders
(`SHGetKnownFolderPath`), discover directories, consult the registry,
create a directory tree, query Docker/AI, or touch frontend IPC. Therefore
the **minimum emergency writer/path is resolved and prepared during normal
runtime** (at normal startup): the diagnostics root and the
`diagnostics/emergency/` directory are created and the prepared path + a
pre-opened writer (or a cheaply recreatable path constant) are published
through a non-blocking mechanism (e.g. an `OnceLock`/atomic swap of a
prepared-state struct) that the panic hook can read without waiting.

This is the **only** exception to lazy directory creation (§34): all other
diagnostics directories (`bundles/`, `failed/`, `temp/`) remain lazy. If
emergency preparation failed at startup (e.g. disk error), the panic hook
detects the unprepared state without OS work and degrades to the existing
redacted panic breadcrumb — no emergency marker, no in-hook recovery.

### 11.2 Active-view context (normal-runtime mechanism)

The existing active view is frontend state; today no view reporting exists.
Because the approved maximum-safe crash snapshot includes the active view
when available, a narrow normal-runtime mechanism is added, conceptually:

```rust
tauri::command: update_diagnostics_context(view_id: ViewId)
```

Requirements:

- invoked only on normal navigation/view changes (not on panic — the panic
  hook performs no IPC and reads only the already cached value);
- the payload is a **validated `ViewId` enum** — no arbitrary strings;
- no OS authority: the cached view ID is display/evidence context only;
- stored via a non-blocking swap (§11.1 pattern) readable from the panic
  hook; if never set (headless/early crash), active view is omitted;
- this command is part of the narrow allowed command surface (§25) with
  this limited, documented purpose.

## 12. Crash Recovery Flow

On next startup, before/while the UI comes up (never blocking startup):

```
detect emergency marker
  → validate size + schema version
  → defensively redact again (never trust prior content)
  → collect normal enriched diagnostics (deep collectors, full snapshot)
  → build full support bundle (trigger: StartupRecovery)
  → DPAPI encrypt → durable atomic write
  → persist metadata/index atomically
  → only THEN delete the emergency marker
  → notify the user after UI is ready (§28: one in-app banner)
```

Failure handling:

- If finalization fails: **keep the marker**, retry on the next startup,
  **max 3 attempts** (attempt count recorded alongside the marker).
- After 3 failed attempts: move the marker into LocalStack-owned
  `diagnostics/failed/`, show a diagnostics-recovery-failure entry in the UI,
  stop retrying, never block app startup.
- The startup recovery path runs on the capture worker, not the startup
  critical path; the app is fully usable while recovery is pending.

Fatal startup failures use the same emergency/finalization model when full
capture is not safe at the point of failure: write the bounded marker, let
the process die; the next successful startup recovers it.

## 13. Support Bundle Content Model

Versioned logical bundle (sections are files inside the encrypted
container):

```
manifest.json          # §13.1
summary.txt            # human-readable Safe-Share-grade summary
health.json            # deep-health results (when run)
incidents.json         # incident records relevant to the trigger
system.json            # OS/CPU/RAM/GPU/WebView2/runtime availability
inventory.json         # LocalStack-owned runtime/tool availability detail
projects.json          # evidence-based project registry state
workspaces.json        # workspace registry state
docker.json            # read-only Docker observation (if detected)
ai-runtimes.json       # read-only AI observability (if detected)
diagnostics.log        # localstack.log tail (redacted)
managed-output/        # bounded managed-service stdout/stderr (§16)
crash/emergency.json   # the recovered emergency record (§12) when present
```

Not every bundle needs every file; **partial bundles are valid**.

### 13.1 Manifest (required in every bundle)

Records: schema version; bundle ID; app version; created timestamp; trigger;
subsystem; severity; fingerprint; **collection errors** (typed, per failed
collector); **truncation state + exact truncated sections**; per-component
sizes; redaction applied; privacy/export-profile metadata as applicable.

### 13.2 Collector failure policy

If one collector fails: keep the rest of the bundle, record a typed
collection error in the manifest. **Only packaging, encryption, or
durable-write failures fail final persistence.**

## 14. Internal System Detail Policy

Internal (encrypted, DPAPI, current-user) diagnostics **may** collect
detailed local machine context where technically useful:

- Windows username, hostname, Windows version/build, architecture;
- CPU, RAM, GPU (where obtainable without elevation or driver hacks);
- WebView2 version; runtime/tool availability;
- local/private IP addresses, MAC addresses;
- Windows SID, MachineGuid;
- disk/device serial or related local device identifiers (where supported);
- full executable paths; full project/workspace names and paths.

**Constraints:**

- Allowed **only** for local encrypted support data (§18 DPAPI, §20
  filesystem safety). These fields never appear in incident metadata,
  notifications, summaries, or Safe Share exports.
- **No public IP lookup; no network service is contacted** to collect
  diagnostics (loopback-only reads of already-integrated AI endpoints remain
  the AI observability shape; nothing new is contacted). No
  automatic/background network egress exists for diagnostics; the only
  user-driven network-adjacent action is the fixed GitHub new-issue URL
  (§17.0, §29).

## 15. Redaction / Secret Invariant

Layered protection — every layer applies independently:

```
collection boundary  → storage boundary  → export boundary
```

- **Collection:** collectors are allow-listed per section; a collector
  physically cannot emit env dumps, argv, auth headers, or cookies (§37
  security gates make each a release blocker).
- **Storage:** the existing deterministic `redact()` runs over all free
  text entering the bundle; extended (not replaced) with the profile
  transforms of §17.
- **Export:** profile transforms run again at export time (a bundle stored
  under Full Forensics-grade internal detail can still export as Safe
  Share).

**Hard invariant across ALL profiles, including Full Forensics — never
intentionally collect:**

- raw environment variables;
- raw process command lines (argv);
- authorization headers; cookies;
- passwords; API keys/tokens; private keys;
- known credentials;
- arbitrary secret-bearing request bodies;
- raw AI prompt/request payloads.

Managed stdout/stderr is arbitrary user-produced text, so automated
redaction is **best-effort**. The UI must warn users to review exports
before sharing. Never claim redaction makes arbitrary logs 100% safe — the
export flow says so explicitly (§26).

## 16. Managed Service stdout/stderr Policy

- Include **bounded** stdout/stderr from **managed services only** —
  never scrape output from arbitrary external processes.
- **Reuse the existing managed-process bounded LogRing snapshots.** The
  workspace/managed-process registry already captures managed output into
  bounded rings (existing `LogRing`); diagnostics takes a bounded
  snapshot/tail of that already-captured output and passes it through
  redaction. No second stdout/stderr capture pipeline is added;
  `CreatePipe`/process-launch behavior is **unchanged**. Arbitrary external
  processes still have no output scraping.
- Roughly last **100 lines per managed service**; pass through the
  redaction pipeline; **hard byte cap 256 KiB per service**; **hard global
  cap 5 MiB combined** (exact constants, §19).
- Deterministic tail truncation with an explicit marker:
  `[TRUNCATED: original output exceeded diagnostic limit]`.
- Still **never** collect raw process command lines or environment variable
  dumps — the managed-service registry's existing records are used as-is;
  if it doesn't record argv/env (it doesn't), diagnostics doesn't ask.

## 17. Three Export Profiles

**Structural, not free-text-only.** Export privacy transformation operates
over the **typed bundle model before serialization** — not merely on
serialized text. It covers: all structured JSON fields; arrays/maps;
generated text files; summaries; filenames/entry names; `README-PRIVACY`;
the GitHub issue title/body; and every emitted string field where
identity/path data may appear. Safe-Share issue title/body is produced
**only** from an already Safe-Share-transformed representation, never raw
incident/bundle state. Defense-in-depth: final export validation tests use
synthetic known-sensitive fixtures (§35) asserting no fixture value appears
in any exported artifact. Free-text `redact()` remains layered on top (§15)
— it is the second line, not the only one.

### 17.0 Egress wording (normative across this document)

All sections of this spec share one precise network-egress invariant:

- no automatic/background diagnostics network egress;
- no automatic support-bundle upload;
- no telemetry; no cloud crash service;
- existing read-only loopback observability remains allowed;
- explicit user action may open LocalStack's fixed GitHub new-issue URL,
  transmitting only the already Safe-Share-transformed issue title/body;
- the support bundle itself is never uploaded automatically.

### 17.1 Safe Share (default)

Remove/generalize: Windows username, hostname, SID, MachineGuid, MAC,
disk/device serials, local/private IPs, unique machine identifiers, full
local user/project paths, and potentially identifying project names where
appropriate.

Path generalization (deterministic transform, tested):
`C:\Users\Example\Projects\SecretApp` → `<USER_HOME>\Projects\<PROJECT>`.

Retain useful support facts: app version; OS/build/architecture; generic
hardware/runtime info; health status; typed incident codes; fingerprint;
relevant PID/port if useful and non-identifying; redacted logs.

Safe Share is the default profile for the GitHub support workflow (§29) and
for Copy Diagnostic Summary (§27).

### 17.2 Developer Detail

Retain: full project names; full project/workspace paths; full executable
paths; PID; port; runtime/framework; service/project relationships; bounded
redacted managed stdout/stderr.

Remove: SID, MachineGuid, MAC, serial/device IDs, unique machine IDs,
credentials, raw argv, environment variables, cookies/auth-header/private-key
material.

UI shows a warning that full paths/project names may reveal local context.

### 17.3 Full Forensics

May retain explicitly approved local identity fields: username, hostname,
local IP, MAC, SID, MachineGuid, serial/device identifiers, full paths,
project/process context.

**The §15 secret invariant still applies in full.**

**Confirmation semantics (user-intent gate, not a security proof):** the
per-export explicit confirmation is a **user-intent / UX gate in the
official LocalStack flow** — the backend cannot cryptographically prove
that a human clicked a modal, and this spec does not claim so. Required
semantics:

- the official UI requires a **fresh, per-export** Full Forensics
  confirmation (a focus-managed confirmation step — §30 a11y);
- the warning clearly states the categories of sensitive data included;
- the backend still validates the trusted export flow (bundle ID + profile
  + command-surface integrity, §25) independently of any frontend-supplied
  flag; a boolean confirmation flag supplied by the frontend is **not**
  described as security authorization proof;

## 18. DPAPI Encryption

Internal support bundles are encrypted at rest with **Windows DPAPI,
current-user scope** — native API `CryptProtectData` / `CryptUnprotectData`
via `windows-sys` (the existing dependency; requires adding its
`Win32_Security_Cryptography` feature — no new crate).

Semantics:

- An encrypted bundle (`.lsdiag`) can be opened automatically only by the
  same Windows user context; it is **not portable**.
- User **export** produces a normal portable ZIP (§26); once exported, the
  ZIP is no longer protected by LocalStack.
- No custom password system; no application-owned encryption keys; no cloud
  keys; no key material stored anywhere.

Integrity: DPAPI is the security/integrity primitive. The manifest may
additionally carry a blake3 content digest as **corruption/bookkeeping
defense-in-depth — never a replacement for DPAPI integrity.** The digest is
computed over the fully serialized, pre-encryption bundle byte stream
(manifest excluded, so it is not self-referential; the manifest's own
integrity rides on the DPAPI-protected container). Decrypt-verify is part
of bundle detail/export. DPAPI roundtrip and corrupted-ciphertext handling
are covered by Windows tests (§36).

## 19. Bundle Size / Storage Limits

Hard limits, **all three enforced**, as **exact constants suitable for
TDD** (each pinned in code and asserted in tests):

| Constant | Exact maximum |
|---|---|
| capture queue capacity | 8 |
| emergency crash record | 512 KiB = 524,288 bytes |
| recent diagnostic lines (panic ring) | 50 |
| cached listeners | 100 |
| cached services | 100 |
| cached projects | 50 |
| cached workspaces | 50 |
| managed-service output, lines | 100 per service |
| managed-service output, bytes | 256 KiB per service |
| all managed output, combined | 5 MiB |
| support bundle logical budget | 50 MiB |
| total encrypted diagnostics-bundle storage target | 1 GiB |
| bundle count | 20 |
| incident count | 500 globally |
| incident count per subsystem | 100 |

**Transactional overshoot:** the 1 GiB target governs **retained**
encrypted bundle bytes; transient filesystem bytes in `temp/` (a bundle
temporarily present in both `temp/` and `bundles/` during a commit,
§33) may briefly exceed 1 GiB and are excluded from the accounting. This
small temporary overshoot is accepted deliberately: it is safer than
deleting user diagnostics before the replacement state is durable (§33).

Whichever retained-state limit is hit first causes oldest bundles to be
removed. Retention:

- **Oldest first by trusted `createdAt`** (recorded in the index at creation
  time — **not** filesystem mtime);
- a `New` (unreviewed) bundle is **not** exempt;
- deletion affects only trusted indexed LocalStack bundle files (§20).

**Never silently truncate.** A bundle that would exceed its cap is built
with explicit truncation: manifest records `truncated: true` and the exact
truncated sections. Section-shrink priority (trim from the bottom first):

1. manifest + incident core
2. crash emergency record
3. health checks
4. LocalStack diagnostic log
5. core system info
6. project/workspace/service inventory
7. Docker/AI detail
8. managed stdout/stderr

Managed output is trimmed before core diagnostic evidence is lost.

## 20. Safe Filesystem Design

All diagnostics storage stays under:

```
%LOCALAPPDATA%\localstack-control-center\diagnostics\
  index.json
  bundles/       # bundle-<opaque-id>.lsdiag
  emergency/     # emergency-<timestamp>.json markers
  failed/        # exhausted recovery markers (§12)
  temp/          # atomic-write staging
```

- Internal bundle filenames use **opaque IDs only** — never project/path/
  error text.
- Writes are atomic: `temp/` → durable write → validation → rename.
- The frontend **never** sends an internal file path for delete / decrypt /
  export / open-folder. It sends **opaque IDs**. The backend resolves
  `bundle ID → trusted registry/index entry → verified LocalStack-owned
  path`.
- Delete/cleanup must: affect only trusted indexed bundle files; verify
  containment beneath the LocalStack diagnostics root; **reject
  symlink/junction/reparse-point escape**; never recursively delete
  arbitrary user paths; **fail closed on containment ambiguity**.
- Retention must **not** glob `diagnostics/*` and delete oldest filesystem
  entries — eviction is index-driven and target-verified (§33 commit flow).

## 21. Index / Corruption / Schema Versioning

Explicit schema versions from day one: incident index schema, bundle schema,
emergency crash schema — each with a major version constant and forward-
compatible (ignore-unknown-fields) parsing for minor growth.

**Integrity state model (four states).** `SupportBundleMeta.integrity_state`
uses (exact enum naming deferred to the plan):

- **Valid** — decryptable and integrity-checked; normal operation;
- **Corrupt** — the **encrypted payload itself** failed a DPAPI
  decrypt/integrity attempt (one bounded attempt, below);
- **Unsupported** — trusted discovery recovered a manifest whose bundle
  schema major version exceeds the supported major;
- **RecoveryRequired** — index missing/malformed left a strict
  owned-file-discovery bundle unindexed and automatic recovery was not (yet)
  attempted.

Important: **missing or malformed index does NOT mean an encrypted payload
is corrupt.** A valid bundle is never labeled `Corrupt` merely because its
metadata was lost. Recovery from index loss:

- strict owned-file discovery (§20 filename shape) may perform **one
  bounded DPAPI decrypt/integrity attempt** per bundle to recover a
  manifest and rebuild metadata;
- successful decrypt/validation → rebuild trusted metadata (state Valid);
- unsupported major schema → `Unsupported`, not `Corrupt`;
- DPAPI/integrity failure → `Corrupt`;
- if automatic recovery is intentionally not attempted (e.g. deferred by
  the plan to a user-visible action) → `RecoveryRequired`;
- **no repeated decrypt loop on every poll/render** — at most one bounded
  attempt per bundle per discovery, state sticky thereafter (§6.4).

- **Unsupported future major schema (index-level):** do not parse
  speculatively; do not delete automatically; mark unsupported; app must not
  crash.
- **Corrupt bundle** (payload hash mismatch / DPAPI failure): state is
  sticky after the single bounded attempt; allow safe **Delete**;
  **disallow Export** when integrity/decryption fails; record one bounded
  breadcrumb; never crash.

## 22. Deep Health Checks

A dedicated deep-health engine (`health.rs`) producing:

```
HealthCheckResult {
  id, subsystem, label,
  status: healthy | degraded | unavailable | skipped,
  code, summary, detail?, checkedAt, durationMs
}
```

`skipped` is **not** an error state (a prerequisite genuinely doesn't exist
— e.g., Docker not installed).

Three categories:

**A. LocalStack internal:** settings validity/readability; diagnostic log
state; diagnostics directory/storage; startup registration state;
single-instance state; tray state; bundle-index integrity; pending crash
recovery; polling ownership; workspace/project registry availability.

**B. Integrations:** discovery engine; service intelligence; project
intelligence; workspace registry; AI observability; Docker integration;
managed-process registry.

**C. Windows/system:** Windows version/build/architecture; WebView2
availability/version; known-folder resolution; **LocalStack-owned filesystem
write test** (create/write/delete a tiny temp file ONLY inside LocalStack
diagnostics storage); TCP listener table accessibility; relevant Win32 API
availability; LocalStack startup-registry read access; Docker named-pipe
reachability (only if Docker is detected/configured); runtime/tool
availability; system identity/hardware collection where supported.

All checks: non-elevated; bounded; **timeout-aware**; isolated so one probe
cannot block all results (per-probe timeout → `unavailable` with a typed
timeout code, remaining probes still run). No firewall/ACL/Windows-service/
system-settings/unrelated-registry modification; no Docker-state mutation.

## 23. Quick vs Deep Health

- **Quick health:** cheap/cached signals surfaced on the Overview (existing
  cached state — no new polling).
- **Deep health:** explicit user action ("Run Deep Diagnostics") or bundle
  capture enrichment.
- **Crash enrichment:** a bounded subset runs automatically during startup
  recovery (§12).

Diagnostics **does not take ownership of polling** already owned by other
features; it reuses their existing runtime/cache state via read-only
accessors. Opening the Diagnostics page never triggers a full OS rescan.

## 24. Diagnostics UI Model

A dedicated Diagnostics page (frontend feature §5.2) with four primary
areas:

**Overview**
- overall health; app version; last deep check;
- active incident count; pending crash recovery;
- diagnostics storage usage; support bundle count.

**Health**
- results grouped by category (§22); text status per check;
- details, duration, last checked;
- "Run Deep Diagnostics" action with textual progress (§30 a11y).

**Incidents**
- subsystem, severity, first/last seen, occurrence count, fingerprint,
  short summary, bundle relationship, New/Reviewed state.

**Support Bundles**
- timestamp, trigger, subsystem, severity, size, fingerprint/dedup group,
  short summary, New/Reviewed, integrity status.

**Actions** (per bundle / globally): Export · Delete · Open Folder ·
Mark Reviewed · Create GitHub Issue · Copy Diagnostic Summary.

Behavioral rules:

- Search/filter operates on **loaded metadata only** — never triggers
  decryption or rescans (§23).
- Bundle detail view shows **metadata first**; the detailed payload is
  decrypted only when the user explicitly requests it.
- Destructive actions (Delete, Full Forensics export) require explicit
  confirmation with focus management (§30).

## 25. Tauri Trust Boundary (Command Surface)

The frontend↔backend boundary stays narrow and capability-specific. Opaque
IDs cross the boundary; trusted path resolution happens exclusively in Rust.

Conceptually acceptable commands (final names locked in the plan):

- `get_diagnostics_overview`
- `run_deep_health_checks`
- `list_incidents`
- `mark_incident_reviewed`
- `list_support_bundles`
- `get_support_bundle_detail` (metadata-first; decrypts payload only on request)
- `export_support_bundle` (bundle ID + profile + user-intent confirmation
  flag; §17.3 confirmation semantics — the flag is UX intent, not a
  security proof; the backend independently validates the trusted flow)
- `delete_support_bundle` (bundle ID)
- `open_diagnostics_folder` (no argument — fixed LocalStack-owned folder)
- `get_support_summary` / `copy_support_summary` (generated Safe Share text)
- `prepare_localstack_github_issue` (fixed URL, user-driven; §29)
- `update_diagnostics_context(view_id)` — narrow context feed: the frontend
  pushes a validated `ViewId` enum on normal navigation so the panic hook's
  cached snapshot can include the active view (§11.2). No OS authority, no
  arbitrary strings, panic-time IPC prohibited.

Explicitly **forbidden** — must never exist as Tauri commands:

- `read_file(path)` / `delete_file(path)` / `decrypt_file(path)`
- `open_any_folder(path)` / `open_url(user_input)`
- `run_probe(command)` / arbitrary command execution
- arbitrary source→destination filesystem copy

Every diagnostics command uses typed DTOs and opaque IDs; the backend
resolves `bundle ID → trusted index entry → verified LocalStack-owned path`
and fails closed on ambiguity (§20). The existing safety-guard test pattern
extends to this surface: a PR adding any forbidden command shape must fail
CI (§35 test strategy).

## 26. Export Model (profiles, destination, ZIP)

Export = decrypt internal bundle → apply profile transform (§17) → assemble
ZIP → hand to a **trusted native save flow**.

- The frontend never receives raw decrypted payload wholesale; the export
  command takes `(bundle ID, profile, user-intent confirmation flag)` and
  returns a user-facing result (§17.3 semantics). A display-only string of
  the user-selected export destination MAY be returned for UX, but such a
  path is display-only: passing it back never becomes filesystem authority.
- Destination: at implementation time, inspect existing Tauri dependencies
  first and prefer the smallest existing API (recorded decision, §43). No
  generic arbitrary-filesystem command is added for any reason.
- Exported ZIP carries the warning, shown in the UI at export time and
  embedded in the ZIP's `README-PRIVACY.txt`:
  **"Exported support bundles are not encrypted by LocalStack. Review them
  before sharing."**

## 27. Copy Diagnostic Summary

Always follows the **Safe Share** posture. Must **not** contain: SID,
MachineGuid, MAC, unique machine IDs, full local paths, unredacted stdout,
secrets. Should contain: LocalStack version; Windows version/architecture;
subsystem; health state; typed incident code/fingerprint; occurrence count;
first/last seen; bundle ID if applicable. Clipboard write is a narrow
command operating on generated summary text only.

## 28. Notifications (context-aware)

- App **foreground** → in-app banner only.
- App **minimized/hidden/background** → Windows native notification.
- **Previous crash finalized on startup** → one in-app banner after UI is
  ready. (Never both channels for the same event unnecessarily.)
- One support-bundle capture causes **at most one** visible notification
  event; cooldown/coalescing prevents notification spam (the capture
  decision's cooldown *is* the notification gate).
- Notifications **never** include sensitive paths, tokens, IDs, usernames,
  or secret-bearing raw errors — fixed, generic copy only.
- If native-notification click-to-open-Diagnostics requires substantial new
  lifecycle/dependency complexity, it degrades gracefully to an
  informational notification (recorded decision, §43). No flaky toast-
  rendering CI automation (§36).

## 29. GitHub Issue Support Workflow

Approved full workflow — every step user-driven:

```
user selects "Create GitHub Issue"
  → generate Safe Share bundle export (§17.1)
  → generate Safe Share issue summary text (§27 shape) — produced only
    from the already Safe-Share-transformed representation (§17)
  → warn user (contents + manual attachment)
  → open LocalStack's FIXED GitHub new-issue URL with prefilled
    title/body (URL-encoded, length-bounded)
  → reveal the folder containing the exported ZIP (capability-shaped:
    `reveal_export_result(export_id)` — §43.8; never `open_any_folder`)
  → user manually attaches the ZIP if desired
```

- Never store a GitHub PAT; never implement OAuth for support; never upload
  the bundle automatically; never call the GitHub API to attach files; never
  publish anything without explicit user action.
- The target repository/issue endpoint is **fixed by trusted application
  code/configuration** — no arbitrary `open_url` exists.
- Failure states (browser/open-folder failure) degrade to showing the
  summary text and the file location, in-app (§32).

## 30. Accessibility

The Diagnostics UI maintains the standard established by the Phase 11B
accessibility hardening (native label association, exposed roles/names).
Acceptance requirements — all regression-tested:

- status **never encoded by color alone** (text always present);
- health statuses have text;
- buttons have accessible names;
- filters are labelled;
- bundle rows/actions are keyboard accessible;
- New/Reviewed is semantically exposed (not just a color dot);
- deep-diagnostics progress is exposed textually (no spinner-only state);
- the Full Forensics warning is announced and focus-managed;
- banners/notifications have meaningful accessible content.

## 31. Performance Constraints

Diagnostics must not materially slow core LocalStack operation
(Phase 10D baseline: ~6% of one core steady-state across all pollers):

- opening Diagnostics triggers no full OS rescan;
- cached snapshots reused whenever possible;
- deep checks run only explicitly or for bundle enrichment;
- incident locks are short (decision-only, §10);
- capture queue is background/non-blocking; bundle building never blocks
  discovery/control paths;
- panic path remains bounded (§11);
- all collectors have timeouts; diagnostics errors are isolated (§32).

## 32. Failure Isolation

Injectable failure cases and required behavior:

| Failure | Required behavior |
|---|---|
| collector failure | partial bundle + typed manifest error; no total loss |
| bundle packaging failure | bundle marked failed in index; incident retained; breadcrumb; core app unaffected |
| DPAPI failure | bundle not persisted; typed error surfaced in UI; retry only on new capture decision (not a tight loop) |
| index write failure | in-memory state continues; index rebuilt/repaired on next successful write; app never crashes |
| retention failure | overflow surfaced in UI; no unbounded growth silently accepted; breadcrumb |
| notification failure | silently absorbed (banner skip / toast skip); never affects capture |
| browser/open-folder failure | degrade to in-app display of summary + location (§29) |

Cross-cutting: core application remains usable; original subsystem behavior
intact; Diagnostics may become degraded; no crash cascade. All failure
injection is test-facing (the plan defines the injection seams).

## 33. Retention Safety (crash-safe commit flow)

Retention serializes with the bundle registry/index — never racy — and is
**crash-safe**: no valid old indexed bundle is ever deleted merely to make
room *before* the new state has a durable commit point. The previous
ordering (evict-then-commit) could lose old diagnostics if the rename or
index persistence failed afterward; the corrected ordering commits the new
state first and deletes evicted files only after the durable index commit:

```
build + encrypt + validate new object in LocalStack temp storage (temp/)
  → acquire registry/index serialization lock
  → compute eviction candidates (all three retained-state limits, §19)
  → finalize the new owned bundle object (rename temp →
     bundles/bundle-<id>.lsdiag)
  → atomically commit an index state that INCLUDES the new bundle and
    EXCLUDES the eviction candidates (durable index write)
  → only AFTER the durable index commit: delete evicted trusted files
    (best-effort)
  → release lock appropriately
```

Properties:

- Crash between finalize and index-commit → the new bundle is an **owned
  orphan**; startup reconciliation handles it safely (strict filename shape,
  §21 recovery states — attempt manifest recovery or mark
  `RecoveryRequired`; never delete on ambiguity).
- Crash after index-commit but before eviction deletes → evicted files
  become owned orphans; startup reconciliation deletes them safely
  (strictly: LocalStack bundle filename shape, regular file, inside the
  verified diagnostics root, no reparse/junction escape, bounded count,
  never arbitrary recursive cleanup).
- A small temporary transactional storage overshoot (temp + finalized copy
  coexisting; §19) is acceptable and is safer than deleting old diagnostics
  before the replacement state is durable.
- Trusted `createdAt` (index-recorded at creation) — never filesystem mtime
  — orders eviction.
- No reparse-point traversal outside the diagnostics root (§20).

### 33.1 Startup reconciliation (owned orphans)

At startup, once, bounded: scan `bundles/` for files matching the strict
LocalStack bundle filename shape (`bundle-<opaque-id>.lsdiag`, regular
file, no reparse points, inside the verified root). Files present on disk
but absent from the index are owned orphans: attempt the §21 bounded
manifest-recovery path; successful recovery re-indexes (Valid/
Unsupported); failed/declined recovery marks `RecoveryRequired` (or
`Corrupt` per §21) and the file stays until explicit user Delete. Never
invent sensitive metadata; never recursively delete arbitrary files;

## 34. Migration (existing installs)

Existing users have only `settings.json` and `localstack.log` in
`%LOCALAPPDATA%\localstack-control-center\`.

- New diagnostics directories (`diagnostics/` tree) are created **lazily**,
  on first need — never eagerly at startup.
- **No destructive migration.** Nothing existing is moved, rewritten, or
  reformatted.
- `localstack.log` stays at its existing documented path during Phase 11C
  (the logger's in-memory tail extension, §11, changes no file semantics).
  Moving it would require an overwhelming technical reason; none exists.

## 35. Test / TDD Strategy

The later implementation **must** use strict TDD: every meaningful new
behavior goes **RED → observe expected failure → GREEN minimal → REFACTOR**.
No behavior lands without a failing-test-first evidence trail in the
implementation plan's checkpoints.

Required Rust coverage:

fingerprint normalization · severe 2/2min trigger · 10-min cooldown ·
critical bypass · duplicate coalescing · bounded queue (capacity, non-
blocking producer, full-queue breadcrumb) · panic emergency-record bounds
(§11 table) · startup crash recovery · 3-attempt recovery limit ·
`failed/` escalation · redaction regressions (existing suite + layered
boundaries) · export profile transforms (per profile, §39) · DPAPI
current-user roundtrip · corrupt ciphertext · 20-bundle retention · 1 GiB
total policy · 50 MiB bundle cap · deterministic truncation markers ·
index-loss recovery states (Valid/Corrupt/Unsupported/RecoveryRequired —
index loss never marks a valid payload Corrupt) ·
partial bundle handling · opaque-ID lookup · path containment ·
junction/reparse escape rejection · malformed/corrupt index handling ·
unsupported-schema handling · probe timeout isolation.

Required frontend coverage:

Diagnostics page/navigation · health grouping · status text (not
color-only) · filters/search (metadata-only) · New/Reviewed semantics ·
export profile selection · Full Forensics confirmation flow · accessible
names · keyboard flow · deep-check progress (textual) ·
degraded/skipped states · in-app banner behavior · GitHub workflow
success/failure states.

Test data: **fake synthetic sensitive fixtures only** — never real user
credentials, API keys, tokens, or real machine identity values.

## 36. Windows Integration Verification

Later implementation verification must include real Windows checks for:
DPAPI encrypt/decrypt; internal bundle encrypted at rest (ciphertext
inspection — plaintext must not appear on disk); exported ZIP readable;
Safe Share sanitization; Developer Detail sanitization; Full Forensics
inclusion policy; crash marker recovery (real emergency marker → restart →
recovered bundle); retention containment; corrupted-bundle handling;
non-elevated deep-health operation; Windows notification anti-spam.

Where interactive desktop notification testing is unreliable in CI: unit/
integration-test the logic, do a controlled Windows live smoke test, and
**do not create flaky CI automation solely to prove toast rendering**.

## 37. Regression Gates

Implementation completion requires **fresh** evidence of:

- frontend typecheck · frontend full test suite · frontend production build
- `cargo check --locked` · `cargo test --locked`
- required GitHub check: **Quality gates (windows-latest)** — green on the
  PR and post-merge on main.

No completion claim without fresh verification output.

## 38. Security Acceptance Gates (release-blocking for 11C)

Each of these is a gate; violating any one blocks the phase:

- no raw process argv in bundles;
- no environment dump;
- no authorization-header dump;
- no cookie dump;
- no private-key material;
- no automatic network upload;
- no arbitrary filesystem delete;
- no arbitrary open-folder API;
- no arbitrary URL opener;
- no GitHub PAT storage;
- no elevation;
- no firewall/ACL/service mutation;
- no unrelated registry mutation.

## 39. Privacy Acceptance Gates

- **Safe Share:** unique machine IDs removed; usernames/hostnames removed;
  full paths generalized/removed as appropriate.
- **Developer Detail:** full path/project/process diagnostic detail
  retained; unique machine identifiers removed.
- **Full Forensics:** approved identity fields retained; the §15 secret
  invariant still enforced.

## 40. Scope

**In scope:** dedicated Diagnostics page · deep health checks · persistent
incident history · typed severe/critical incident API · incident
fingerprinting · panic emergency snapshot · startup crash recovery ·
automatic severe capture · bounded capture queue · DPAPI-encrypted local
bundles · retention · three export profiles · bounded managed-service
stdout/stderr · detailed system diagnostics · context-aware in-app/Windows
notifications · GitHub issue preparation workflow · accessibility · tests ·
documentation.

**Out of scope:** telemetry · automatic crash upload · cloud diagnostics ·
remote support · automatic GitHub file upload · PAT/OAuth support ·
automatic repair/self-healing · process killing · Docker mutation · AI
model inference · remote-machine discovery · LAN scanning · Linux/macOS
support · auto-updater work · version bump · v1.0.2 tag/release · any
GitHub Release publication.

## 41. Definition of Done (for future implementation)

Phase 11C is complete only when:

- approved spec **and** approved implementation plan exist;
- implementation followed strict TDD;
- full frontend/Rust test suites pass fresh;
- relevant live Windows checks pass (§36);
- DPAPI verified; privacy profiles verified; crash recovery verified;
  retention verified;
- filesystem/path boundary audited; Tauri command boundary audited;
- accessibility verified;
- docs updated;
- PR required CI green; main clean after merge;
- **no tag/release/version bump performed.**

## 42. Acceptance Criteria (verifiable statements)

1. A severe incident, occurring twice within 2 minutes for the same
   fingerprint, produces exactly one encrypted bundle; a further burst
   inside the cooldown produces none; occurrence counts coalesce.
2. A panic produces a bounded emergency record (≤ 512 KiB = 524,288 bytes,
   §11 bounds) without querying Docker/AI/filesystem-at-large and without
   waiting on any lock, and the next startup recovers it into a bundle and
   deletes the marker.
3. Three recovery failures escalate to `failed/` and surface in the UI
   without blocking startup.
4. All three storage limits are enforced with index-ordered eviction, and
   deletion provably cannot escape the diagnostics root (reparse-point
   rejection tested).
5. Safe Share export of a Full-Forensics-grade internal bundle contains no
   username/hostname/SID/MachineGuid/MAC/serial/full-path data (tested
   transform + live check).
6. The official LocalStack UI cannot initiate a Full Forensics export
   without a fresh per-export explicit confirmation; the §15 secret
   invariant holds in every profile.
7. The Diagnostics page renders quick health from cache with no rescan;
   deep checks run only on demand and are timeout-isolated.
8. Every §38 security gate and §39 privacy gate has a corresponding test
   or live verification.
9. Existing 27 diagnostics call sites compile and behave identically via
   the facade; `localstack.log` path/format unchanged.
10. Frontend and Rust suites, build, and the required GitHub check are
    green fresh at merge.

## 43. Bounded Design Decisions Deferred to the Implementation Plan

These are intentionally **not** invented here; each is a bounded decision
requiring repository/API inspection at plan time:

1. **Export destination API.** No dialog plugin is currently a dependency.
   At plan time, inspect existing Tauri capabilities/plugins and pick the
   smallest existing-API approach for the native save flow; document the
   reason before adding any plugin (§26 constraint).
2. **Windows notification mechanism.** `tauri-plugin-notification` vs a
   minimal direct Win32 path — decide after inspecting how tray/window
   notifications could reuse existing surface; graceful degradation to
   informational-only is pre-approved if complexity is substantial (§28).
3. **Capture worker substrate.** Tokio task vs dedicated thread — decide
   after mapping which collectors are blocking (DPAPI/FS) vs async
   (§10).
4. **windows-sys feature addition.** DPAPI requires enabling
   `Win32_Security_Cryptography` on the existing `windows-sys` dependency;
   no new crate. Plan verifies exact feature path for the 0.60 series.
5. **Frontend snapshot-read surface.** Exact read-only accessors each
   owning subsystem exposes to `snapshot.rs` (listeners/services/projects/
   workspaces/subsystem status) — enumerated per module in the plan,
   adding no new polling ownership.
6. **GPU/WebView2 version collection specifics.** Bounded, non-elevated,
   best-effort — exact APIs chosen at plan time with "omit if unavailable"
   as the default.
7. **Diagnostics UI information architecture.** Tabbed vs stacked sections
   for Overview/Health/Incidents/Bundles (§24), per existing layout
   patterns — a UX detail for the plan, with §30 a11y requirements binding
   either way.
8. **Reveal-exported-folder mechanism.** The GitHub workflow must reveal
   the exported ZIP's folder without any `open_any_folder(path)`. Preferred
   capability shape: the backend retains a trusted export result/capability
   ID from the native save operation, and a narrow
   `reveal_export_result(export_id)` reveals that exact user-selected
   destination; the frontend never supplies an arbitrary folder path for
   shell-open. A display-only export-path string may be surfaced for UX but
   is never filesystem authority when passed back (§26, §29). If the chosen
   save API natively returns the chosen path to the backend only, the plan
   locks the capability-ID plumbing accordingly.
