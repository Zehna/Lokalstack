# Phase 11C Diagnostics & Supportability Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the approved Phase 11C design: typed incident history with deterministic bounds/eviction, bounded non-blocking capture, DPAPI-encrypted support bundles under a crash-safe retention transaction, panic emergency records + startup recovery, deep-health engine, three structural privacy export profiles, dedicated Diagnostics UI, and the narrow Tauri command surface — strict TDD, zero weakening of Phase 1–11B safety boundaries, no tag/release/version bump.

**Architecture:** `src-tauri/src/diagnostics.rs` (396 lines, Phase 10B) becomes directory module `src-tauri/src/diagnostics/` with `mod.rs` re-exporting the exact facade (`info`, `warn`, `error`, `local_app_data_dir`, plus the existing types the 5 caller files use). Incidents persist as an atomic JSON index; support bundles are single-file encrypted `.lsdiag` artifacts (opaque-ID filenames) under `%LOCALAPPDATA%\localstack-control-center\diagnostics\`. One dedicated capture worker thread; lock-free snapshot caches (`ArcSwap` pattern via `std::sync::Mutex<Option<Arc<..>>>` read under `try_lock`, written under lock only from normal runtime) shared with the panic hook. Panic path: only prepared context + non-blocking reads, never a contended lock. Frontend: `src/features/diagnostics/` (UI only) → `src/stores/diagnosticsStore.ts` (Zustand) → `src/services/native/diagnostics.ts` (only `invoke` callsite) → wire DTOs in `src/types/domain.ts`.

**Tech Stack:** Existing: `tauri 2`, `serde`, `serde_json`, `blake3`, `tokio 1`, `windows-sys 0.60.2` (already in Cargo.lock; crate declares `rust-version = "1.82"` at `src-tauri/Cargo.toml:6`), React 19, Zustand, Vitest + Testing Library. Direct dependencies ADDED by this plan (Rust crates only — **no new npm packages and no new frontend capability grants**; the frontend never gains direct plugin authority, rationale in §43-A/B/H): `tauri-plugin-dialog = "2"` (A), `tauri-plugin-opener = "2"` (H), `tauri-plugin-notification = "2"` (B), `zip = { version = "2", default-features = false, features = ["deflate"] }` (A/export). **YAGNI removals versus the first plan draft:** NO `crossbeam-channel` direct dep — std `std::sync::mpsc::sync_channel::<CaptureRequest>(8)` + `SyncSender::try_send` + `Receiver::recv_timeout` provide identical capacity/non-blocking/timeout semantics (C); NO `tempfile` direct dep — a LocalStack-owned atomic-write helper over `diagnostics/temp/` with opaque validated filenames + same-volume `std::fs::rename` replaces it (Tasks 1/2/8/10). windows-sys feature additions (exact, verified against the local windows-sys-0.60.2 registry source): `Win32_Security_Cryptography` (Task 8 DPAPI + Task 8/§43-H `BCryptGenRandom` IDs), `Win32_Security_Authorization` (Task 12 `ConvertSidToStringSidW`), `Win32_System_SystemInformation` + `Win32_Graphics_Dxgi` (Task 12), plus conditional `Win32_System_VariantFlags` only if Task 8's `cargo check` demands it. **MSRV evidence (binding `rust-version = "1.82"`):** every added line resolves at 1.82 — tauri-plugin-* 2.x declare `rust-version ≥ 1.77.2` (docs.rs, verified during planning); `zip` MUST resolve to the 2.x line (2.1.3 MSRV 1.73; the 8.x line requires 1.88 and is forbidden) — Task 13 verifies the resolved Cargo.lock entry stays on 2.x.

**Spec:** `docs/superpowers/specs/2026-09-18-phase-11c-diagnostics-supportability-design.md` (binding). Repo reality anchors: `src-tauri/src/diagnostics.rs` (facade), `src-tauri/src/workspace/rules.rs:440-500` (`LogLine`, `LogRing`), `src-tauri/src/workspace/mod.rs:487` (managed-process `logs: LogRing`), `src/services/native/ports.ts` (sole production `invoke(`), `src/safetyGuards.test.ts` (static guard: `invoke(` forbidden outside native client), `src/app/navigation.ts:7,17-25` (`ViewId`, `NAV_ITEMS`), `src/stores/appStore.ts` (`setActiveView`), `.github/workflows/ci.yml:27` (required check `Quality gates (windows-latest)`; steps: typecheck / test:run / build / cargo check --locked / cargo test --locked; no clippy gate).

---

## Global Constraints (binding, copied from approved spec)

### Exact numeric bounds (each owned by a test in the referenced task)

| Constant | Exact value | Owning task |
|---|---|---|
| Capture queue capacity | 8 | Task 4 |
| Emergency crash record max | 524,288 bytes (512 KiB) | Task 6 |
| Recent diagnostic lines (panic record) | 50 | Task 6 |
| Cached listeners max | 100 | Task 5 |
| Cached services max | 100 | Task 5 |
| Cached projects max | 50 | Task 5 |
| Cached workspaces max | 50 | Task 5 |
| Managed output per service | max 100 lines | Task 11 |
| Managed output per service | max 256 KiB | Task 11 |
| All managed output combined | max 5 MiB | Task 11 |
| Support bundle logical budget | max 50 MiB | Task 7 (§7.3) |
| Retained encrypted bundle storage | 1 GiB target max (retained committed bytes; Task 10 enforces; bounded transaction overshoot exempt, Task 10 tests it) | Task 10 |
| Bundle count | max 20 | Task 10 |
| Persistent incidents | max 500 globally | Task 2 |
| Persistent incidents | max 100 per subsystem | Task 2 |
| Severe trigger | 2 matching severe failures within 2 minutes | Task 3 |
| Cooldown | 10 minutes | Task 3 |
| Crash-recovery finalization attempts | max 3 | Task 9 |
| Existing log contract (unchanged) | 2,000 session lines; 256 KiB cross-session file bound | Task 1 regression |

### Binding behavioral contracts

- **Facade survives:** `diagnostics::info/warn/error(...)` and `diagnostics::local_app_data_dir(...)` keep signature and behavior; the 5 caller files (`lib.rs`, `app_commands.rs`, `settings.rs`, `workspace/mod.rs`, `workspace/registry.rs`) need no edits in Task 1.
- **Panic path (Task 6):** never blocks on any `Mutex`/`RwLock`; only `try_lock`/atomic snapshot reads; omits busy/poisoned/missing sections; no Docker/AI query, no listener/process OS scan, no frontend IPC, no `SHGetKnownFolderPath`, no directory discovery/creation, no ZIP, no DPAPI, no arbitrary filesystem enumeration; does not depend on the normal `LOG_FILE` mutex (emergency writer has its own prepared `Mutex<File>` and uses `try_lock`); degrades to the existing redacted breadcrumb if the emergency store was not prepared at startup.
- **Capture worker isolation (Task 4):** one capture-job panic is caught at the job boundary (`std::panic::catch_unwind` + `AssertUnwindSafe` around the injected build closure ONLY — never around the whole application or unrelated panics); the coalescing key is released on success, error, and panic; exactly one bounded redacted breadcrumb records the panic (fixed template, raw panic payload never written); the worker loop and subsequent queued captures continue; the originating subsystem is unaffected.
- **Structural privacy (Task 7, 13, 19):** profile transforms operate on the typed bundle model before serialization, across every structured field/array/map/filename/summary/issue body; free-text redaction is defense-in-depth only; Safe Share GitHub title/body originate only from an already-transformed DTO; Full Forensics never bypasses token/password/API-key/argv/env/auth-header/cookie/private-key prohibitions.
- **Network (§18):** no automatic/background diagnostics egress, no telemetry, no bundle upload, no crash upload; only existing loopback observability plus user-actioned fixed GitHub new-issue URL carrying Safe-Share-transformed, length-bounded, URL-encoded title/body.
- **Trust boundary (Task 14):** opaque IDs only across the bridge; no `read_file(path)`/`delete_file(path)`/`decrypt_file(path)`/`open_any_folder(path)`/`open_url(user_input)`/`run_probe(command)`/arbitrary copy commands; `safetyGuards.test.ts` stays green; Rust-side audit (Task 20) plus `#[cfg(windows)]` live verification (Task 21).
- **Full Forensics confirmation is a user-intent/UX gate** (Task 17 UI per-export confirm; Task 13 backend validates bundle ID + profile + trusted destination flow; never claimed as cryptographic authorization).
- **Managed output (Task 11):** reuse existing `LogRing` (`workspace/rules.rs:450`) snapshots via one new read-only registry accessor; no CreatePipe changes, no second capture pipeline, no external-process scraping.
- **Retention transaction (Task 10):** temp build → lock → evict-candidate computation → finalize new object → durable atomic index commit (includes new, excludes evictions, **adds evicted IDs to `pending_deletions` tombstones**) → only then best-effort delete of tombstoned trusted files → atomic tombstone-clear commit for successfully deleted IDs → unlock; bounded overshoot allowed; startup reconciliation: a tombstoned owned file is **DELETED, never re-indexed** (a crash between index commit and delete must not resurrect evicted diagnostics); a missing tombstoned file clears its tombstone; a failed deletion retains its tombstone for bounded retry; unindexed owned bundles WITHOUT a tombstone are bounded recovery candidates; orphan temp cleanup; malformed/unsupported index per spec §33.1. Index loss ≠ payload corruption.
- **No tag, no GitHub Release, no version bump anywhere in this plan.**

---

## Review Focus (five highest-risk input/edge classes; each has a named test in its owning task)

1. **Malicious/malformed owned diagnostics filesystem state (reparse/junction escape, foreign files, orphan temp).** Tests: `escape_rejected` (Task 8), `orphan_reconciliation_strict_shape` (Task 10), `reparse_junction_escape_rejected` (Task 21 live).
2. **Secret-bearing nested structured data escaping Safe Share.** Tests: `structural_safe_share_nested_secrets` (Task 7), `export_defense_in_depth_fixture` (Task 13), live `Safe Share privacy fixture absent` (Task 21).
3. **Panic while diagnostics caches/locks are busy/poisoned.** Tests: `panic_context_busy_omitted` / `panic_log_mutex_held_still_writes_marker` (Task 6).
4. **Index write failure during the retention transaction must not lose old bundles.** Test: `index_commit_failure_preserves_evicted_candidates` (Task 10).
5. **Notification/export/GitHub external-action failure without affecting the core app.** Tests: `notify_failure_is_swallowed` (Task 18), `export_pipeline_partial_failure_yields_typed_error` (Task 13), `github_workflow_failure_state` (Task 19).

---

## §43 Deferred Decisions — RESOLVED (repo-verified)

- **A. Export destination:** `tauri-plugin-dialog = "2"` — **Rust crate only; NO npm package (`@tauri-apps/plugin-dialog` is NOT added) and NO capability permission.** Grounding: the local `tauri-plugin-dialog 2.7.2` registry source exposes the Rust-side API `DialogExt::blocking_save_file()` / `blocking_save_file_builder(...)` (dialog_ext impl, line 768) designed for non-main-thread callers; the plugin's `permissions/default.toml` gates only the plugin's own JS-invoked commands, which this plan never grants or uses — Rust-side `DialogExt` calls do not traverse the WebView capability system, so the frontend gains no dialog/filesystem authority. Plugin registration `.plugin(tauri_plugin_dialog::init())` in `lib.rs` is still required (the Rust API needs the plugin state on `AppHandle`). The only dialog trigger is LocalStack's own narrow `export_support_bundle` command in response to explicit user action — never the background capture worker. ZIP via `zip = { version = "2", default-features = false, features = ["deflate"] }` — no `arbitrary filesystem copy` command exists; the ZIP is written by Rust directly to the user-confirmed destination. `zip` must resolve on the 2.x line (MSRV 1.73 ≤ repo `rust-version 1.82`; the 8.x line requires 1.88) — Task 13 verifies the Cargo.lock entry. No new cryptography crate.
- **B. Windows notifications:** `tauri-plugin-notification = "2"` — **Rust crate only; NO npm package and NO capability permission.** Rust-side API `NotificationExt::notification().show()` (docs.rs-verified during planning; same 2.x line, `rust-version ≥ 1.77.2`) called from Task 18's injected native closure; the WebView never invokes plugin commands. Plugin registration `.plugin(tauri_plugin_notification::init())` required. Foreground → in-app banner only (Zustand flag); background → plugin toast; **focus/visibility query failure → `Unknown` → in-app banner (conservative fallback — never guess background and toast)** (Task 18). Degrade path per spec §28: if the plugin's `is_permission_granted()`/registration fails, log `diagnostics::warn` and keep the in-app banner only — no lifecycle complexity added.
- **C. Capture worker substrate:** dedicated OS **worker thread** (`std::thread::Builder::name("diag-capture")`), not a tokio task — bundle building does blocking FS + DPAPI work; tokio's async runtime must not be stalled. Queue: **`std::sync::mpsc::sync_channel::<CaptureRequest>(8)`** — exact capacity 8, multi-producer/one-consumer; producers use `SyncSender::try_send` (never blocks; `TrySendError::Full` → `QueueFull` + one bounded breadcrumb; `TrySendError::Disconnected` → log once, stop enqueueing); worker uses `Receiver::recv_timeout(100ms)` so the shutdown `AtomicBool` terminates it within ~100 ms even when idle; coalescing (`Mutex<HashSet<IncidentKey>>` with **`try_lock`** on the producer path, lock held only for insert/remove — never during build) is channel-independent. **YAGNI: NO `crossbeam-channel` direct dependency** — std provides identical bounded/non-blocking/timeout semantics for this one-consumer design. **Job-boundary panic isolation:** every build invocation runs under `std::panic::catch_unwind(AssertUnwindSafe(|| build(req)))` — a panicking job does not terminate the loop; its coalescing key is released on success/error/panic alike; exactly one bounded redacted breadcrumb (fixed template, raw payload never written) records it; the next queued capture proceeds; the originating subsystem is unaffected. Shutdown: flag checked at loop top; sender handles dropped with the app so `Disconnected` is the natural exit; no join on hot paths and nothing holds the process alive (no bundle outlives Tauri shutdown).
- **D. DPAPI via windows-sys:** verified in the local registry source `windows-sys-0.60.2/src/Windows/Win32/Security/Cryptography/mod.rs`:
  - `fn CryptProtectData(pdatain: *const CRYPT_INTEGER_BLOB, szdatadescr: PCWSTR, poptionalentropy: *const CRYPT_INTEGER_BLOB, pvreserved: *const c_void, ppromptstruct: *const CRYPTPROTECT_PROMPTSTRUCT, dwflags: u32, pdataout: *mut CRYPT_INTEGER_BLOB) -> BOOL` (line 283)
  - `fn CryptUnprotectData(... ppszdatadescr: *mut PWSTR, ...) -> BOOL` (line 315) — same module
  - `pub const CRYPTPROTECT_UI_FORBIDDEN: u32 = 1` (line 5272)
  - blob type is `CRYPT_INTEGER_BLOB { cbData: u32, pbData: *mut u8 }`.
  Cargo feature change (exact): in `src-tauri/Cargo.toml` the `windows-sys` target dependency gains features `"Win32_Security_Cryptography"` (pulls `Win32_Security`) and `"Win32_System_VariantFlags"` (for `VariantBool` used by some DPAPI callers — only if the compiler demands it; add in Task 8's RED cycle if `cargo check` names it). Cleanup semantics: `LocalFree`-free — the blob buffers are allocated by `CryptProtectData` via `LocalAlloc`; free with `windows_sys` `Win32_Foundation` `LocalFree(HLOCAL(pbData))` (`LocalFree` is already exposed under `Win32_Foundation`, enabled by the crate's existing features). Empty/zero payload: `CryptProtectData` accepts `cbData = 0` but the API is defined only for payloads ≥ 1 byte in our contract (test asserts `Err(EmptyPayload)`), corrupt ciphertext → `Err` mapped to `BundleIntegrityState::Corrupt`.
- **E. Snapshot/read-only accessor map (no duplicate polling ownership):**
  - listeners → `src-tauri/src/listeners.rs` discovery cache (existing polling owner); Task 5 adds `pub(crate) fn cached_listener_snapshot() -> Vec<ListenerSummary>` reading the existing cache under its existing lock, never re-scanning.
  - services → managed-process registry `src-tauri/src/workspace/registry.rs` (existing polling owner); `pub(crate) fn cached_service_snapshot() -> Vec<ServiceSummary>`.
  - projects → `src-tauri/src/project/` registry cache; `pub(crate) fn cached_project_snapshot() -> Vec<ProjectSummary>`.
  - workspaces → `src-tauri/src/workspace/registry.rs` cache; `pub(crate) fn cached_workspace_snapshot() -> Vec<WorkspaceSummary>`.
  - subsystem status → composed in Task 5 from the three above + docker/AI cached status values already held by `docker.rs` / `ai_runtime.rs` modules (read their existing cached status only).
  - managed LogRing output → `pub(crate) fn snapshot_managed_log_tails()` added beside the registry that owns `logs: LogRing` (`workspace/mod.rs:487`), implemented as `logs.since(0)` under the registry's existing lock; Task 11 defines exact placement next to that owner.
  All five accessors are pure reads of already-cached state; none schedules a poll or scan.
- **F. GPU / WebView2 / machine identity:** WebView2 version → registry read `HKCU\Software\Microsoft\EdgeWebView\BLBeacon\version` via existing `windows-sys` `Win32_System_Registry` usage pattern (module already used for the startup autostart entry). CPU/RAM/arch → `Win32_System_SystemInformation` (`GetSystemInfo`, `GlobalMemoryStatusEx`) — same crate, features `Win32_System_SystemInformation`. Windows version/build → `Win32_System_SystemInformation::GetVersionExW` equivalent (`RtlGetVersion`). GPU → `Win32_Graphics_Dxgi::CreateDXGIFactory1` + `GetDesc` (feature `Win32_Graphics_Dxgi`), bounded to first adapter description; unavailable → field `Skipped`. Local IPs/MAC → `GetAdaptersAddresses` under `Win32_NetworkManagement_IpHelper` (already a windows-sys module; add feature only if not transitively enabled). **No PowerShell/WMI runner, no shell command execution anywhere.** Username via `GetUserNameW` (`Win32_System_Threading`). **SID (corrected from the first draft — `GetUserNameExW(NameSamCompatible)` yields an account NAME, not a SID):** exact verified chain, all read-only/non-elevated/current-process-only — `GetCurrentProcess()` (`Win32_System_Threading::GetCurrentProcess`, windows-sys-0.60.2 line 133) → `OpenProcessToken(handle, TOKEN_QUERY, &mut token)` (`Win32_System_Threading::OpenProcessToken`, line 244; `TOKEN_QUERY: TOKEN_ACCESS_MASK = 8` at `Win32/Security/mod.rs:1096`) → `GetTokenInformation(token, TokenUser, buf, len, &mut ret)` (`Win32/Security/mod.rs:100`; `pub struct TOKEN_USER` at line 1129) → `ConvertSidToStringSidW(token_user.User.Sid, &mut str_sid)` (`Win32/Security/Authorization/mod.rs:54`, feature `Win32_Security_Authorization`) → copy the PWSTR → `LocalFree` the returned string (Win32_Foundation, already enabled) and `CloseHandle` the token. Bounded allocation: first call with len 0 for `ERROR_INSUFFICIENT_BUFFER`, then one allocation of the returned size (≤ a few KB). Every failure → `Skipped`/omitted field, never a crash; test seams inject a token-info failure to exercise the `Skipped` mapping. No WMI/PowerShell/cmd anywhere. MachineGuid via registry read `HKLM\SOFTWARE\Microsoft\Cryptography\MachineGuid` (existing `Win32_System_Registry` pattern). All behind `identity.rs` with per-field `Result<Option<String>>`; omissions are `Skipped`, never errors. Each feature flag is verified by `cargo check` in Task 12's RED/GREEN cycle; any flag the compiler reports missing is added in that same task (flag names above are exact windows-sys 0.60 module names).
- **G. Diagnostics UI information architecture:** **tabbed** — the app shell renders one feature page per `ViewId` and existing pages (Settings) use in-page tabbed sections; Diagnostics renders 4 in-page tabs: Overview / Health / Incidents / Support Bundles (`src/features/diagnostics/components/DiagnosticsTabs.tsx` with `role="tablist"`). Deep-check progress is text ("Running deep diagnostics… 7 of 12 checks complete").
- **H. Export reveal capability:** backend-held `ExportCapability { export_id: String /* opaque 24-hex via Task 2's BCryptGenRandom-backed generator — NOT content-derived, NOT from a "getrandom" direct import (transitive deps are not an approved API) */, destination: PathBuf, created_at_ms: u64 }` stored in an in-memory `Mutex<HashMap<String, ExportCapability>>` (max 8 entries, FIFO eviction, 10-minute TTL). Semantics PINNED: **in-memory only — invalid after process restart** (a new registry does not know old IDs); **one-shot after successful reveal** — `reveal_export_result(export_id)` verifies membership + TTL, opens the folder containing exactly that stored destination via `tauri_plugin_opener::open_path(parent_dir)` (opener crate added alongside per §43-A family; needed anyway for `open_diagnostics_folder`), and on success **removes the capability immediately**; on opener failure the capability REMAINS until retry/TTL/eviction; expired/evicted/restarted → typed `Unknown`; the frontend cannot renew or create a capability by submitting any path (display text of the chosen filename may be returned by `export_support_bundle` for UI display only — it never becomes filesystem authority). The export-specific blocking thread that hosts the save dialog is NOT the automatic diagnostics capture worker. Registry size: max 8; eviction is FIFO by insertion.
- **I. Active-view diagnostics context (CORRECTED against repository reality):** today `src/app/AppLayout.tsx` owns a **local `useState<ViewId>('dashboard')`** while `src/stores/appStore.ts` independently holds `activeView`/`setActiveView`, and `DashboardPage` already writes the store — a live dual-source-of-truth defect this plan REMOVES (Task 16): `useAppStore.activeView`/`useAppStore.setActiveView` become the SINGLE canonical navigation state; AppLayout drops its local `useState` and reads from the store; `Sidebar.onSelectView` and the `ErrorBoundary` dashboard navigation both call the same canonical `setActiveView`; the `VIEWS` lookup derives from store `activeView`; diagnostics context is a fire-and-forget side effect of `setActiveView` (store update is synchronous — navigation NEVER waits for backend IPC; an `updateDiagnosticsContext` failure is swallowed and cannot undo navigation; no panic-time IPC). Rust-side cache lives in the Task 5 `CacheHandle` as `active_view: Mutex<Option<u8>>` (validated ViewId discriminant, non-blocking swap; panic hook reads it under `try_lock`). RED regression tests A–D live in Task 16.

---

## Planned File Structure (locked before Task 1)

**Rust — Create (all under `src-tauri/src/`):**

| Path | Responsibility |
|---|---|
| `diagnostics/mod.rs` | Facade: re-exports former `diagnostics.rs` API verbatim + `pub mod` tree |
| `diagnostics/paths.rs` | Diagnostics root/dir resolution, emergency-store preparation, lazy-dir helpers |
| `diagnostics/incidents.rs` | `IncidentRecord`, `IncidentIndex`, fingerprint (blake3 over normalized fields), bounds/eviction, atomic index persistence |
| `diagnostics/policy.rs` | Pure severe/critical decision engine (2-in-2-min trigger, 10-min cooldown) |
| `diagnostics/worker.rs` | Bounded capture queue (8), worker thread, coalescing, `CaptureDecision` consumption |
| `diagnostics/cache.rs` | `AppSnapshot` caches (listeners/services/projects/workspaces/status/active view) + `CacheHandle` |
| `diagnostics/emergency.rs` | Prepared emergency writer + panic-hook replacement + bounded serialization |
| `diagnostics/recovery.rs` | Startup crash-marker recovery pipeline (validate→redact→enrich→bundle→3-attempt cap→`failed/`) |
| `diagnostics/bundle.rs` | Typed `BundleModel` v1, manifest, collector errors, 50 MiB budget + priority truncation |
| `diagnostics/redact_export.rs` | Structural profile transforms (Safe Share / Developer Detail / Full Forensics) |
| `diagnostics/crypto.rs` | DPAPI encrypt/decrypt (windows-sys), `.lsdiag` frame, blake3 digest bookkeeping |
| `diagnostics/store.rs` | Bundle registry/index, retention transaction, startup reconciliation, orphan rules |
| `diagnostics/health.rs` | `HealthCheckResult`, deep-health engine, per-probe timeout/isolation |
| `diagnostics/collectors.rs` | System/identity/managed-LogRing/Windows collectors (bounded, read-only) |
| `diagnostics/export.rs` | ZIP assembly, dialog save, `ExportCapability` registry, `reveal_export_result` support |
| `diagnostics/commands.rs` | All new `#[tauri::command]` fns (thin, validate, delegate) |

**Rust — Modify:** `src-tauri/src/lib.rs` (delete old `diagnostics` module declaration path → directory module; `.plugin(...)` additions; pre-app cache/emergency prep; register new commands; recovery call), `src-tauri/src/workspace/registry.rs` + `src-tauri/src/workspace/mod.rs` (one read-only accessor each), `src-tauri/src/listeners.rs`, `src-tauri/src/project/` (one read-only accessor each), `src-tauri/Cargo.toml` (deps/features above).

**Frontend — Create:** `src/features/diagnostics/DiagnosticsPage.tsx`, `src/features/diagnostics/components/{DiagnosticsTabs.tsx,OverviewTab.tsx,HealthTab.tsx,IncidentsTab.tsx,BundlesTab.tsx,ExportProfileDialog.tsx,FullForensicsConfirm.tsx,NotificationBanner.tsx,GitHubIssueDialog.tsx}`, `src/features/diagnostics/types.ts` (UI-only view models), `src/stores/diagnosticsStore.ts`, `src/services/native/diagnostics.ts`.

**Frontend — Modify:** `src/types/domain.ts` (DTO types), `src/app/navigation.ts` (add `{ id: 'diagnostics', label: 'Diagnostics', feature: 'diagnostics' }` after `settings`), `src/app/AppLayout.tsx` (route + context hook), `src/stores/appStore.ts` (fire context update in `setActiveView` — single choke point for I), `src/safetyGuards.test.ts` (no change required — audit only; extend only if a new forbidden shape is warranted).

**Tests:** Rust co-located `#[cfg(test)] mod tests` in each new module (repo convention) + `#[ignore]`-marked live Windows tests in `diagnostics/live_windows_tests.rs`. Frontend: colocated `DiagnosticsPage.test.tsx`, `diagnosticsStore.test.ts`, `diagnostics.test.ts`, `navigation.test.ts` updates.

---

### Task 1: Split `diagnostics.rs` into a directory module preserving the Phase 10B facade

**Files:**
- Create: `src-tauri/src/diagnostics/mod.rs`, `src-tauri/src/diagnostics/paths.rs`, `src-tauri/src/diagnostics/logger.rs` (content moved verbatim from `diagnostics.rs`, split by current internal sections), `src-tauri/src/diagnostics/ids.rs` (opaque-ID generator; produced in Task 2 but its module file is created here so `paths::write_atomic` can compile)
- Modify: `src-tauri/src/lib.rs` (only the `mod diagnostics;` declaration stays identical — directory modules need no change; if the current declaration is `mod diagnostics;` no edit is required — verify), `src-tauri/src/diagnostics.rs` (deleted via `git mv` semantics below)
- Test: `src-tauri/src/diagnostics/mod.rs` (facade test), `src-tauri/src/diagnostics/paths.rs` (path tests)

**Interfaces:**
- Consumes: existing `diagnostics::{info, warn, error, local_app_data_dir, ...}` (verify the full pub surface with `grep -n "pub fn\|pub const\|pub static" src-tauri/src/diagnostics.rs` before moving).
- Produces: `diagnostics::paths::diag_storage_root() -> PathBuf` (`%LOCALAPPDATA%\localstack-control-center\diagnostics`), `diagnostics::paths::bundles_dir() -> PathBuf`, `emergency_dir()`, `failed_dir()`, `temp_dir()`, `diagnostics::paths::ensure_dir(p: &Path) -> io::Result<()>` (creates only beneath the diagnostics root), and the **owned atomic-write helper** (NO `tempfile` crate — YAGNI): `diagnostics::paths::write_atomic(dest: &Path, bytes: &[u8]) -> io::Result<()>` — writes `temp/<dest-filename>.<16-hex-random>.tmp` with `OpenOptions::new().write(true).create_new(true)`, `write_all`, `flush` + `file.sync_all()`, drops the handle (Windows release-before-rename), then `std::fs::rename(tmp, dest)` (same volume → atomic replacement of an existing destination), and **best-effort deletes the temp file on every pre-rename failure path**; plus `diagnostics::paths::purge_stale_temp(max_age_ms: u128) -> usize` — deletes ONLY files in `temp/` matching `^[A-Za-z0-9._-]+\.[0-9a-f]{16}\.tmp$` (strict owned shape, regular files, no reparse points, containment verified), never touching foreign names. Later tasks (2/8/10) consume exactly `write_atomic` and `purge_stale_temp`.

- [ ] Step 1: RED — add to `src-tauri/src/diagnostics.rs` (before split) a test proving the facade contract and the new paths API:
```rust
#[test]
fn facade_surface_unchanged() {
    // info/warn/error are callable and cheap (best-effort, never panic):
    info("test", "facade probe");
    warn("test", "facade probe");
    error("test", "facade probe");
}

#[test]
fn diag_paths_are_under_local_app_data() {
    let root = paths::diag_storage_root();
    assert!(root.starts_with(local_app_data_dir()));
    assert!(root.ends_with("diagnostics"));
    assert!(paths::bundles_dir().starts_with(&root));
    assert!(paths::emergency_dir().starts_with(&root));
}

#[test]
fn write_atomic_replaces_existing_destination_and_cleans_temp() {
    let d = temp_dir().join(format!("wa-test-{}", std::process::id()));
    ensure_dir(&d).unwrap();
    let dest = d.join("state.json");
    write_atomic(&dest, b"v1").unwrap();
    write_atomic(&dest, b"v2").unwrap(); // existing destination replaced atomically
    assert_eq!(std::fs::read(&dest).unwrap(), b"v2");
    assert_eq!(std::fs::read_dir(&d).unwrap().count(), 1); // no leftover .tmp
}

#[test]
fn write_atomic_failed_rename_leaves_no_temp_and_reported_error() {
    // dest parent made unwritable via an injected failure seam OR (host-agnostic)
    // dest is an existing DIRECTORY -> rename fails; temp cleaned up, old content intact
    let d = temp_dir().join(format!("wa-fail-{}", std::process::id()));
    ensure_dir(&d).unwrap();
    let dest_dir = d.join("dest"); std::fs::create_dir(&dest_dir).unwrap();
    assert!(write_atomic(&dest_dir, b"x").is_err());
    assert_eq!(std::fs::read_dir(&d).unwrap().count(), 1); // only dest dir remains, no .tmp
}

#[test]
fn purge_stale_temp_only_touches_owned_shape() {
    let d = temp_dir();
    std::fs::write(d.join("bundle-abc1234567890abcdef0123.0123456789abcdef.tmp"), b"x").unwrap();
    std::fs::write(d.join("not-ours.txt"), b"x").unwrap();
    purge_stale_temp(0);
    assert!(!d.join("bundle-abc1234567890abcdef0123.0123456789abcdef.tmp").exists());
    assert!(d.join("not-ours.txt").exists()); // foreign name survives
}
```
  Run: `cd src-tauri && cargo test --locked diagnostics::` → **RED: `paths` module and `write_atomic`/`purge_stale_temp` do not exist (E0433 unresolved module)** — a compile-level RED proving the new API is missing, not a logic failure.
- [ ] Step 2: Create `diagnostics/` module: `mkdir src-tauri/src/diagnostics`, `git mv src-tauri/src/diagnostics.rs src-tauri/src/diagnostics/logger.rs`; in `logger.rs` keep everything except the tests moved into `mod.rs`; write `mod.rs` with `mod logger; pub use logger::*;` plus `pub mod paths;`. Do not alter any moved function body.
- [ ] Step 3: GREEN — implement `paths.rs`:
```rust
use std::path::PathBuf;
pub fn local_app_data_dir() -> PathBuf { crate::diagnostics::local_app_data_dir_base() } // existing helper re-exposed
pub fn diag_storage_root() -> PathBuf { local_app_data_dir().join("diagnostics") }
pub fn bundles_dir() -> PathBuf { diag_storage_root().join("bundles") }
pub fn emergency_dir() -> PathBuf { diag_storage_root().join("emergency") }
pub fn failed_dir() -> PathBuf { diag_storage_root().join("failed") }
pub fn temp_dir() -> PathBuf { diag_storage_root().join("temp") }
pub fn ensure_dir(p: &std::path::Path) -> std::io::Result<()> {
    if p.exists() { return Ok(()); }
    std::fs::create_dir_all(p)
}

/// Owned temp+rename atomic write. NO tempfile crate: same-volume rename under
/// the LocalStack-owned diagnostics root; best-effort temp cleanup on failure.
pub fn write_atomic(dest: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp_name = format!("{}.{}.tmp", dest.file_name().unwrap_or_default().to_string_lossy(), rand16hex());
    let tmp = temp_dir().join(&tmp_name);
    let write_res = (|| -> std::io::Result<()> {
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        use std::io::Write;
        f.write_all(bytes)?;
        f.flush()?;
        f.sync_all()?;
        drop(f); // release handle before rename (Windows requirement)
        std::fs::rename(&tmp, dest)
    })();
    if write_res.is_err() { let _ = std::fs::remove_file(&tmp); } // best-effort cleanup
    write_res
}

/// Delete ONLY strictly-shaped stale temp files (regex: ^[A-Za-z0-9._-]+\.[0-9a-f]{16}\.tmp$),
/// regular files only, inside temp_dir; never follows reparse points; never arbitrary paths.
pub fn purge_stale_temp(max_age_ms: u128) -> usize { /* strict-shape scan + age filter; bounded; returns count */ }

fn rand16hex() -> String { crate::diagnostics::ids::random_hex(8) } // 8 random bytes -> 16 hex chars (Task 2 RNG seam)
```
  Tests: the Step-1 RED tests now run GREEN against these bodies (`diag_paths_are_under_local_app_data`, `write_atomic_replaces_existing_destination_and_cleans_temp`, `write_atomic_failed_rename_leaves_no_temp_and_reported_error`, `purge_stale_temp_only_touches_owned_shape`, `ensure_dir_creates_missing_dir`). `write_atomic` consumers in Tasks 2/8/10 pass `dest` paths that already exist beneath the diagnostics root — replacing an existing destination via same-volume rename is the verified Windows behavior (last-writer-wins, atomic visibility).
- [ ] Step 4: Run `cd src-tauri && cargo test --locked diagnostics::` → GREEN.
- [ ] Step 5: Regression subset: `cargo test --locked` (full suite — 5 caller files must compile unmodified) and `cargo check --locked`.
- [ ] Step 6: Commit: `git add src-tauri/src/diagnostics src-tauri/src/diagnostics.rs && git commit -m "refactor: split diagnostics into directory module preserving facade"`

### Task 2: Typed incident model, fingerprint, bounded persistent index

**Files:**
- Create: `src-tauri/src/diagnostics/incidents.rs`
- Test: `#[cfg(test)] mod tests` in `incidents.rs`

**Interfaces:**
- Produces: `pub struct IncidentKey { pub subsystem: String, pub fingerprint: String }`; `pub struct IncidentRecord { key: IncidentKey, code: String, severity: Severity, operation: String, summary: String /* redacted */, first_seen_ms: u64, last_seen_ms: u64, occurrences: SaturatingU64, review_state: ReviewState, trigger_reason: String, bundle_ids: Vec<String> }`; `pub enum Severity { Info, Warning, Severe, Critical }` (ordered); `pub struct SaturatingU64(u64)` with `AddAssign` saturating; `pub struct IncidentIndex { schema_version: u32 /* =1 */, incidents: Vec<IncidentRecord>, pending_deletions: Vec<String> /* tombstoned owned bundle filenames — consumed by Task 10 */ }`; `pub fn fingerprint(subsystem: &str, code: &str, operation: &str, severity: Severity) -> String` (blake3 hex of `"v1\0{subsystem}\0{code}\0{operation}\0{severity}"`); `IncidentIndex::record(...) -> (IncidentIndexMutation, Option<IncidentRecord>)`; `IncidentIndex::load_or_default(path) -> (IncidentIndex, IndexHealth)`; `IncidentIndex::save_atomic(&self, path) -> io::Result<()>` (Task 1 `write_atomic` — no tempfile crate); `pub struct IndexHealth { state: IndexState, repaired_from_backup: bool }`; `pub enum IndexState { Valid, Corrupt, Unsupported, RecoveryRequired }`.
- Produces (`ids.rs`, opaque-ID generator — §43-H RNG resolved): `pub fn random_hex(bytes: usize) -> String` backed by **`BCryptGenRandom(NULL, buf, len, BCRYPT_USE_SYSTEM_PREFERRED_RNG)`** (windows-sys 0.60.2 `Win32/Security/Cryptography/mod.rs:38`, `BCRYPT_USE_SYSTEM_PREFERRED_RNG = 2` at line 1293 — the SAME `Win32_Security_Cryptography` feature Task 8 already requires; `NTSTATUS` failure → `Err`, never fallback to a time-seeded source); collision-resistant, unpredictable (capability guessing is not an authority path), **not derived from project names/paths/errors/secrets**; deterministic test seam: `pub fn with_source(src: &dyn Fn(usize) -> Vec<u8>) -> String` used by tests; bundle IDs (Task 10), incident opaque IDs (here), export IDs (Task 13), temp suffixes (Task 1) all consume this one generator — no new randomness crate; existing control-target ID behavior untouched.
- Consumes: Task 1 `paths::diag_storage_root()`.

- [ ] Step 1: RED — tests in `incidents.rs` test module:
```rust
fn rec(sub: &str, sev: Severity) -> IncidentRecord { IncidentRecord::new_for_test(sub, "ERR_X", sev, "op", "summary") }

#[test] fn random_hex_is_opaque_and_not_content_derived() {
    let a = ids::random_hex(12); assert_eq!(a.len(), 24);
    assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    assert_ne!(a, ids::random_hex(12)); // distinct draws
    // (BCryptGenRandom failure path exercised via with_source seam returning Err-mapped input)
}
#[test] fn fingerprint_is_stable_and_discriminates_normalized_fields() {
    let a = fingerprint("docker", "ERR_X", "probe", Severity::Severe);
    let b = fingerprint("docker", "ERR_X", "probe", Severity::Severe);
    assert_eq!(a, b); // deterministic across calls/processes
    assert_ne!(fingerprint("docker", "ERR_X", "probe", Severity::Warning), a);
    assert_ne!(fingerprint("docker", "ERR_Y", "probe", Severity::Severe), a);
    assert_ne!(fingerprint("workspace", "ERR_X", "probe", Severity::Severe), a);
    // Domain is exactly the 5-field NUL-joined string "v1\0{subsystem}\0{code}\0{operation}\0{severity}" —
    // no PID/timestamp/username/path/message input exists in the signature, so volatile data cannot enter.
}
#[test] fn occurrences_saturate() { let mut o = SaturatingU64::new(u64::MAX); o += 5; assert_eq!(o.get(), u64::MAX); }
#[test] fn global_cap_500_evicts_deterministically() { /* build 501 records, assert len == 500 and the evicted one is chosen by: lowest severity, then oldest last_seen, then lexicographically-greatest opaque id tie-break */ }
#[test] fn per_subsystem_cap_100_evicts_first() { /* 101 same-subsystem records -> 100 remain */ }
#[test] fn index_save_and_load_roundtrip() { /* save to a file under paths::temp_dir() via write_atomic; load back; records equal */ }
#[test] fn failed_index_write_leaves_previous_file_intact() { /* load valid index; attempt save to a dest that is an existing directory (rename fails) -> previous file content unchanged; no .tmp residue in temp/ */ }
#[test] fn malformed_index_loads_as_corrupt_without_deleting_file() { /* write "not json"; load -> state Corrupt, file still present */ }
#[test] fn future_major_schema_is_unsupported_not_corrupt() { /* schema_version: 2 -> Unsupported */ }
#[test] fn evicted_candidate_is_lowest_severity_then_oldest() { /* one Info(oldest) + one Critical(newer): adding a 3rd to a cap-2 index evicts the Info */ }
```
  Run: `cd src-tauri && cargo test --locked diagnostics::incidents diagnostics::ids` → **RED: `IncidentRecord`/`fingerprint`/`ids` not found (E0433/E0425)**.
- [ ] Step 2: GREEN — implement `incidents.rs` exactly per signatures above. Eviction: sort candidates by `(severity ascending, last_seen_ms ascending, opaque_id descending)` and pop from the front until caps hold (per-subsystem cap applied before global cap). Opaque id: `ids::random_hex(12)` (BCryptGenRandom-backed, Task 2 module) — collision-resistant, unpredictable, not content-derived; mechanism final (no new randomness crate). `save_atomic`: serialize → Task 1 `paths::write_atomic(path, bytes)` (create_new temp in `temp/` → sync_all → close handle → same-volume atomic rename; existing-destination replacement verified; best-effort temp cleanup on failure). `load_or_default`: parse failure → `Corrupt` (file untouched); `schema_version != 1` → `Unsupported`; unknown extra fields ignored via `serde(default)`; `pending_deletions` deserialized with `serde(default)`.
- [ ] Step 3: `cargo test --locked diagnostics::incidents` → GREEN.
- [ ] Step 4: Regression: `cargo test --locked && cargo check --locked`.
- [ ] Step 5: Commit: `git add src-tauri/src/diagnostics/incidents.rs && git commit -m "feat: bounded persistent incident index with deterministic eviction"`

### Task 3: Severe/critical capture decision engine (pure policy)

**Files:**
- Create: `src-tauri/src/diagnostics/policy.rs`
- Test: `policy.rs` tests

**Interfaces:**
- Produces: `pub struct CapturePolicyState` (per-fingerprint: `severe_timestamps_ms: Vec<u64>`, `cooldown_until_ms: Option<u64>`); `pub enum CaptureDecision { None, RequestBundle }`; `impl CapturePolicyState { pub fn on_event(&mut self, key: &IncidentKey, severity: Severity, now_ms: u64) -> CaptureDecision }` — severe: record timestamp, keep last 2, if 2 within 120_000 ms and not in cooldown → `RequestBundle` + set `cooldown_until = now + 600_000`; during cooldown → `None` (occurrence counting already happened in Task 2). Critical: immediate `RequestBundle` (bypass threshold) but honor the same cooldown/anti-spam.

- [ ] Step 1: RED tests:
```rust
#[test] fn two_severe_within_two_minutes_request_bundle() { /* t=0 severe, t=60_000 severe -> second returns RequestBundle */ }
#[test] fn two_severe_outside_window_do_not() { /* t=0, t=120_001 -> None */ }
#[test] fn cooldown_blocks_second_bundle_for_10_minutes() { /* trigger at t=60_000; more severes at t=120_000..600_000 -> None; at t=660_001 -> RequestBundle again */ }
#[test] fn critical_bypasses_threshold_but_not_cooldown() { /* first critical -> RequestBundle immediately; critical 1s later -> None; after cooldown -> RequestBundle */ }
#[test] fn warning_and_info_never_capture() { /* 100 warnings/info -> None */ }
#[test] fn hundred_matching_severe_events_produce_at_most_one_bundle_per_cooldown() { /* hammer 100 events in 5s -> exactly 1 RequestBundle */ }
```
  Run: `cargo test --locked diagnostics::policy` → **RED: `CapturePolicyState` not found**.
- [ ] Step 2: GREEN — implement exactly per signature; pure functions, no I/O, no clock reads (time injected as `now_ms`).
- [ ] Step 3: `cargo test --locked diagnostics::policy` → GREEN.
- [ ] Step 4: `cargo test --locked && cargo check --locked`.
- [ ] Step 5: Commit: `git add src-tauri/src/diagnostics/policy.rs && git commit -m "feat: severe-incident capture policy with threshold and cooldown"`

### Task 4: Bounded capture worker (queue capacity 8, coalescing, non-blocking producers)

**Files:**
- Create: `src-tauri/src/diagnostics/worker.rs`
- Test: `worker.rs` tests

**Interfaces:**
- Consumes: Task 2 `IncidentKey`, Task 3 `CapturePolicyState`/`CaptureDecision`.
- Produces: `pub enum CaptureRequest { Bundle { key: IncidentKey, trigger: String, severity: Severity } }`; `pub struct CaptureWorker { tx: std::sync::mpsc::SyncSender<CaptureRequest> }`; `impl CaptureWorker { pub fn spawn(rx: std::sync::mpsc::Receiver<CaptureRequest>, coalesce: Arc<Mutex<HashSet<IncidentKey>>>, build: Arc<dyn Fn(CaptureRequest) + Send + Sync>, shutdown: Arc<AtomicBool>) -> std::thread::JoinHandle<()> }`; `pub fn try_enqueue(&self, req: CaptureRequest, coalesce: &Mutex<HashSet<IncidentKey>>) -> EnqueueOutcome` with `pub enum EnqueueOutcome { Accepted, CoalescedDuplicate, QueueFull, QueueClosed }` — `std::sync::mpsc::TrySendError::Full` → `QueueFull`, `TrySendError::Disconnected` → `QueueClosed` (worker gone; caller logs the one bounded breadcrumb, never retries blindly). The `build` closure is injected — Tasks 7–13 supply the real bundle builder; Task 4 injects a counting fake. **Job panic isolation:** the worker runs each build inside `std::panic::catch_unwind(AssertUnwindSafe(...))`; a panicking job never terminates the loop, its coalescing key is released, one bounded redacted breadcrumb (`"capture job panicked; job skipped"` — fixed template, payload never written) is emitted, and the next queued capture proceeds. Shutdown: `AtomicBool` checked at loop top; channel `recv_timeout(100ms)` so a set flag terminates within ~100 ms even when idle; channel disconnect ends the loop.

- [ ] Step 1: RED tests:
```rust
#[test] fn capacity_is_exactly_8() {
    // with no consumer draining, 8 try_enqueue calls return Accepted, the 9th returns QueueFull
}
#[test] fn duplicate_fingerprint_coalesces() {
    // same IncidentKey twice (accepted then before build completes) -> second is CoalescedDuplicate
}
#[test] fn queue_full_does_not_block_producer() {
    // time a loop of 50 enqueue attempts against a full queue: completes < 100 ms, all QueueFull
}
#[test] fn worker_drains_and_consumes_requests() {
    // counting build fn: enqueue 3 distinct requests, drop tx, join worker -> counter == 3
}
#[test] fn shutdown_flag_terminates_idle_worker() {
    // spawn with empty channel, set flag, join returns within 2 s
}
#[test] fn worker_survives_a_panicking_build_job() {
    // build closure: call 1 panics (poison test string), later calls record into a shared counter
    // enqueue req A (panics) then req B; join worker
    // assert: B executed (counter == 1); worker alive through both; coalescing set EMPTY after each
    // job (key released on panic); exactly one breadcrumb with the fixed redacted template and NO
    // payload text; second request completed normally
}
#[test] fn queue_closed_after_worker_death_reports_queueclosed() {
    // drop the receiver side (worker exited); try_enqueue -> QueueClosed, producer returns immediately
}
```
  Run: `cargo test --locked diagnostics::worker` → **RED: `CaptureWorker` not found**.
- [ ] Step 2: GREEN — implement: `std::sync::mpsc::sync_channel::<CaptureRequest>(8)`; `try_enqueue` first `try_lock`s the coalesce set — if contended, treat as `CoalescedDuplicate` (never block; spec §10) — insert key, `tx.try_send` → map `TrySendError::Full` to `QueueFull` (remove the inserted key in this case so retry is possible) and `TrySendError::Disconnected` to `QueueClosed`, and log via `diagnostics::warn` exactly once per 30 s window for full-queue breadcrumbs (one bounded breadcrumb, spec §5); worker loop: `rx.recv_timeout(Duration::from_millis(100))` — `Timeout` → check shutdown flag and loop; `Disconnected` → exit; on request: `let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| build(req.clone())));` then — for BOTH `Ok` and `Err` — remove the key from the coalesce set (release on success/error/panic); on `Err(_)` discard the payload un-inspected (never downcast/log it — it may carry secret-bearing data), emit the fixed-template breadcrumb once, and continue the loop; loop top checks shutdown flag.
- [ ] Step 3: `cargo test --locked diagnostics::worker` → GREEN.
- [ ] Step 4: `cargo test --locked && cargo check --locked`.
- [ ] Step 5: Commit: `git add src-tauri/src/diagnostics/worker.rs && git commit -m "feat: bounded diagnostics capture worker with coalescing"`

### Task 5: Snapshot cache + active-view diagnostics context

**Files:**
- Create: `src-tauri/src/diagnostics/cache.rs`
- Modify: `src-tauri/src/listeners.rs`, `src-tauri/src/workspace/registry.rs`, `src-tauri/src/project/` (owner module), `src-tauri/src/workspace/mod.rs` — one read-only snapshot accessor each (§43-E exact placements; helper signatures only, zero behavior change)
- Test: `cache.rs` tests

**Interfaces:**
- Consumes: existing caches (§43-E map); Task 1 `paths`.
- Produces: `pub struct AppSnapshot { pub listeners: Vec<ListenerSummary>, pub services: Vec<ServiceSummary>, pub projects: Vec<ProjectSummary>, pub workspaces: Vec<WorkspaceSummary>, pub subsystem_status: Vec<SubsystemStatusSummary>, pub captured_at_ms: u64 }` (summary structs: id/name/status/stable counts only — no paths, no pids, no env); `pub struct CacheHandle { snapshot: Mutex<Option<Arc<AppSnapshot>>>, active_view: Mutex<Option<u8>> /* ViewId discriminator */ }`; `impl CacheHandle { pub fn publish(&self, s: AppSnapshot)`, `pub fn try_snapshot(&self) -> Option<Arc<AppSnapshot>>` (**`try_lock` — busy → None**), `pub fn set_active_view(&self, v: u8) -> bool` (validates against known range 0..NAV_LEN), `pub fn try_active_view(&self) -> Option<u8>` (**`try_lock`**), `pub fn try_recent_log_tail(&self, max_lines: usize) -> Vec<String>` (**`try_lock` on the log-tail ring — busy → empty**), `pub fn publish_log_tail(&self, lines: Vec<String>)`. Bounds enforced at publish: listeners 100, services 100, projects 50, workspaces 50, log tail 50 (truncate to last N before storing).

- [ ] Step 1: RED tests in `cache.rs`:
```rust
#[test] fn publish_enforces_exact_bounds() { /* 120 listener summaries -> snapshot.listeners.len() == 100; 60 projects -> 50; 60 workspaces -> 50; 120 services -> 100 */ }
#[test] fn busy_mutex_reads_return_none_not_block() {
    // take the MutexGuard manually in the test, then try_snapshot with a timeout-measured call -> returns None quickly (<50 ms)
}
#[test] fn active_view_validated() { /* set_active_view(99) -> false; set_active_view(0) -> true; try_active_view -> Some(0) */ }
#[test] fn log_tail_capped_at_50() { /* publish 60 lines -> try_recent_log_tail(50).len() == 50, first line is #11 */ }
```
  Run: `cargo test --locked diagnostics::cache` → **RED: `CacheHandle` not found**.
- [ ] Step 2: GREEN — implement `cache.rs`. ViewId validation: a `const NAV_LEN: u8 = 10;` mirror (Task 16 wires the real mapping; the discriminant is positional per `src/app/navigation.ts` NAV_ITEMS order). Snapshot accessor wiring: in each owner module add exactly one fn, e.g. in `listeners.rs`:
```rust
pub(crate) fn cached_listener_snapshot() -> Vec<crate::diagnostics::cache::ListenerSummary> {
    // read the EXISTING discovery cache under its EXISTING lock; map to summaries; no scan
}
```
  (same shape for `workspace/registry.rs` services+workspaces, project owner, and one `snapshot_managed_log_tails()` beside `workspace/mod.rs:487`'s `logs: LogRing` — bodies are pure cache reads; if an owner's lock is already `Mutex`, keep it and rely on callers never holding it long; these run on the capture worker thread, never on the polling thread.)
- [ ] Step 3: `cargo test --locked diagnostics::cache` → GREEN; then full `cargo test --locked && cargo check --locked` (owner modules compile unchanged).
- [ ] Step 4: Commit: `git add src-tauri/src/diagnostics/cache.rs src-tauri/src/listeners.rs src-tauri/src/workspace/registry.rs src-tauri/src/workspace/mod.rs src-tauri/src/project && git commit -m "feat: diagnostics snapshot cache with non-blocking reads and view context"`

### Task 6: Panic emergency-record writer (release-critical, non-blocking)

**Files:**
- Create: `src-tauri/src/diagnostics/emergency.rs`
- Modify: `src-tauri/src/lib.rs` (call `emergency::prepare_once()` + `emergency::install()` from the existing setup path where the Phase 10B panic hook is currently installed; keep the old hook's redacted-breadcrumb behavior as the degradation path), `src-tauri/src/diagnostics/mod.rs` (wire modules)
- Test: `emergency.rs` tests

**Interfaces:**
- Consumes: Task 1 `paths::emergency_dir()`/`ensure_dir` (called in **prepare**, not in the hook), Task 5 `CacheHandle`, existing redaction.
- Produces: `pub struct PreparedEmergency { pub file_path: PathBuf }`; `pub fn prepare_once() -> Result<PreparedEmergency, String>` (resolve + create `emergency/` dir + open/create the marker file handle ONCE at normal startup); `pub fn install(cache: Arc<CacheHandle>, prepared: Option<PreparedEmergency>)` (replaces the current panic hook; internally wraps the old breadcrumb hook as fallback); `pub struct EmergencyRecordV1 { schema_version: u32 /*=1*/, timestamp_ms, app_version, panic_summary_redacted, source_location, active_view: Option<u8>, fingerprint: String, recent_log_lines: Vec<String> /*≤50*/, cached_listeners, cached_services, cached_projects, cached_workspaces, subsystem_status_summary }`; `pub fn render_record(record: &EmergencyRecordV1) -> Result<Vec<u8>, Bounded` — internal; hard cap enforced post-serialization: if JSON exceeds 524,288 bytes, drop sections in priority order (cached_* → subsystem_status → log lines) then re-serialize, and finally truncate-with-marker to exactly ≤ 524,288. Panic hook behavior (contract, proven by component tests below): `try_lock` only; busy/poisoned → omit section; if `prepared == None` → write the old redacted breadcrumb (existing `diagnostics::error` path is NOT called — it takes the LOG_FILE mutex — instead write the breadcrumb via the old hook's own mechanism kept verbatim).

- [ ] Step 1: RED tests:
```rust
#[test] fn record_serialization_is_capped_at_524_288_bytes() {
    // build a record with 60 log lines and 4 KB summaries everywhere; render; assert bytes.len() <= 524_288 AND schema field intact
}
#[test] fn oversized_record_drops_lowest_priority_sections_first() {
    // giant log tail + giant cached_listeners -> rendered record keeps panic_summary + schema; cached_listeners omitted
}
#[test] fn busy_cache_sections_are_omitted() {
    // hold the CacheHandle mutex guards in the test; call render path used by hook; sections omitted, no wait (wall-clock < 100 ms)
}
#[test] fn unprepared_emergency_degrades_to_breadcrumb() {
    // install with prepared=None; invoke the hook fn directly (extracted as fn for testability); assert breadcrumb text written via injected sink, no panic
}
#[test] fn prepared_writer_uses_try_lock_only() {
    // hold LOG_FILE-equivalent lock (the logger's file mutex) while invoking the hook fn; assert the marker STILL lands (emergency writer independent of logger mutex)
}
#[test] fn panic_summary_is_redacted() {
    // payload string containing "password=hunter2 token=abc" -> rendered record contains "password=<redacted>"-style output and never "hunter2"
}
```
  Run: `cargo test --locked diagnostics::emergency` → **RED: module/fns missing**.
- [ ] Step 2: GREEN — implement. Hook body sketch:
```rust
fn hook_impl(info: &std::panic::PanicHookInfo<'_>, cache: &CacheHandle, prepared: &Option<PreparedEmergency>) {
    let summary = redact(&fmt_panic(info));           // existing deterministic redactor
    let fp = incidents::fingerprint("panic", "PANIC", source_location(info), Severity::Critical);
    let record = EmergencyRecordV1 {
        schema_version: 1,
        timestamp_ms: now_ms(),
        app_version: app_version().to_string(),
        panic_summary_redacted: summary,
        source_location: source_location(info),
        active_view: cache.try_active_view(),
        fingerprint: fp,
        recent_log_lines: cache.try_recent_log_tail(50),
        cached_listeners: cache.try_snapshot().map(|s| s.listeners.clone()).unwrap_or_default(),
        cached_services: cache.try_snapshot().map(|s| s.services.clone()).unwrap_or_default(),
        cached_projects: cache.try_snapshot().map(|s| s.projects.clone()).unwrap_or_default(),
        cached_workspaces: cache.try_snapshot().map(|s| s.workspaces.clone()).unwrap_or_default(),
        subsystem_status_summary: cache.try_snapshot().map(|s| s.subsystem_status.clone()).unwrap_or_default(),
    };
    let bytes = match render_record(&record) { Ok(b) => b, Err(_) => fallback_marker(&record) };
    match prepared {
        Some(p) => { if let Ok(mut f) = p.file.try_lock() { let _ = write_all_and_fsync(&mut f, &bytes); } else { old_breadcrumb(&record); } }
        None => old_breadcrumb(&record),
    }
}
```
  No `SHGetKnownFolderPath`, no dir creation, no registry, no Docker/AI, no IPC, no ZIP/DPAPI anywhere in this file's hook path. `write_all_and_fsync` writes ≤ 512 KiB then `f.sync_all()`.
- [ ] Step 3: `cargo test --locked diagnostics::emergency` → GREEN.
- [ ] Step 4: Full `cargo test --locked && cargo check --locked`.
- [ ] Step 5: Commit: `git add src-tauri/src/diagnostics/emergency.rs src-tauri/src/diagnostics/mod.rs src-tauri/src/lib.rs && git commit -m "feat: non-blocking panic emergency record writer"`

### Task 7: Typed bundle model + structural privacy transform foundation

**Files:**
- Create: `src-tauri/src/diagnostics/bundle.rs`, `src-tauri/src/diagnostics/redact_export.rs`
- Test: tests in both files

**Interfaces:**
- Consumes: Task 2 `IncidentRecord`/fingerprint; existing deterministic redactor (Phase 10B).
- Produces (bundle.rs): `pub const BUNDLE_SCHEMA_VERSION: u32 = 1;` `pub struct BundleModel { pub manifest: BundleManifest, pub sections: Vec<BundleSection> }`; `pub struct BundleManifest { schema_version, bundle_id: String, app_version, created_at_ms, trigger, subsystem, severity, fingerprint, collection_errors: Vec<TypedCollectionError>, truncation: TruncationState, section_sizes: BTreeMap<String, u64>, redaction_applied: bool, privacy_profile_note: String, blake3_digest_note: String }`; `pub struct BundleSection { name: SectionName, content: SectionContent }`; `pub enum SectionName { Manifest, Summary, Health, Incidents, System, Inventory, Projects, Workspaces, Docker, AiRuntimes, DiagnosticsLog, ManagedOutput, CrashEmergency }`; `pub enum SectionContent { Json(serde_json::Value), Text(String), ManagedOutput(Vec<ManagedServiceOutput>) }`; `pub struct TypedCollectionError { section: SectionName, code: String, detail_redacted: String }`; budget: `pub const BUNDLE_LOGICAL_BUDGET_BYTES: u64 = 52_428_800;` + `pub fn apply_budget(model: BundleModel) -> BundleModel` (priority order: manifest+incidents core → crash emergency → health → diagnostics log → system → inventory/projects/workspaces → docker/ai → managed output; drop/trim lowest priority first; sets `truncation.truncated = true` + names truncated sections; **never silent**). Produces (redact_export.rs): `pub enum PrivacyProfile { SafeShare, DeveloperDetail, FullForensics }`; `pub struct IdentityField { kind: IdentityKind, value: String }`; `pub enum IdentityKind { Username, Hostname, Sid, MachineGuid, MacAddress, LocalIp, DeviceSerial, UserHomePath, ProjectPath, ExecutablePath }`; `pub fn transform(model: &BundleModel, profile: PrivacyProfile, identity: &[IdentityField]) -> BundleModel` — STRUCTURAL: walks the typed model; for `SectionContent::Json` it recursively rewrites Values (objects/arrays/maps/strings) replacing matching identity values with the spec's generalizations (`<USER_HOME>`, `<PROJECT>`, `<HOSTNAME>`, `<MAC>`, `<LOCAL_IP>`, `<SID>`, `<MACHINE_GUID>`, `<DEVICE_SERIAL>`); for `Text` sections it applies the same value substitution (value-based, not regex-only — the identity list is typed); filenames/entry names derive from section names only (never content). Full Forensics: `transform` keeps identity fields verbatim but STILL runs the existing secret redactor over all strings (defense-in-depth). Managed output and diagnostics log always pass through the Phase 10B redactor in every profile.

- [ ] Step 1: RED tests (`bundle.rs`):
```rust
#[test] fn manifest_records_truncation_state_and_never_truncates_silently() { /* oversize model -> apply_budget -> manifest.truncation.truncated == true, truncated_sections non-empty */ }
#[test] fn budget_is_exactly_50_MiB() { /* section of 51 MiB alone is reduced; 49 MiB model untouched */ }
#[test] fn collector_error_is_typed_and_kept_in_manifest() { /* section errors appear in manifest.collection_errors with redacted detail */ }
#[test] fn bundle_id_is_opaque_24_hex() { /* matches ^[0-9a-f]{24}$ */ }
```
  RED tests (`redact_export.rs`):
```rust
fn model_with_sensitive() -> BundleModel {
    // COMPLETE synthetic fixture (no real credentials). Identity values:
    //   username "alice"; hostname "DESKTOP-XYZ"; user path "C:\\Users\\alice";
    //   project name/path "C:\\Users\\alice\\Projects\\SecretApp"; SID "S-1-5-21-1004336348-1177248915-682003330-1001";
    //   MachineGuid "1234abcd-12ab-34cd-56ef-1234567890ab"; MAC "AA-BB-CC-DD-EE-FF";
    //   local/private IP "192.168.1.50"; DeviceSerial "WD-EXAMPLE-SERIAL-77413" (concrete fixture even though
    //   the Phase 11C collector returns Skipped — the structural schema/transform must still handle it).
    // Secret-shaped values: "token=abc123secret"; "password=hunter2"; "api_key=sk-live-EXAMPLE-000";
    //   "Authorization: Bearer eyJEXAMPLE.TOKEN.999"; "Cookie: session=EXAMPLE-5150";
    //   private-key-shaped content "-----BEGIN PRIVATE KEY-----\nEXAMPLEBLOCK\n-----END PRIVATE KEY-----".
    // PLACEMENTS (each value appears in ALL of): a direct System JSON field; a nested object (two levels);
    //   an array element; a map value; ManagedOutput (LogRing) lines; the generated Summary text.
    // The GitHub issue title/body are derived in the test from BOTH the raw and transformed models
    //   (raw derivation must never be used — asserted), and a content-derived candidate filename input
    //   ("bundle-alice-SecretApp.lsdiag" style) is fed to the entry-name rule and must be rejected for fixed names.
}
#[test] fn safe_share_structural_transform_hits_nested_json_arrays_and_maps() { let out = transform(&model_with_sensitive(), PrivacyProfile::SafeShare, &identity()); let s = serde_json::to_string(&out).unwrap(); for absent in ["alice","DESKTOP-XYZ","S-1-5-21","1234abcd","AA-BB-CC","192.168.1.50","SecretApp","WD-EXAMPLE-SERIAL","hunter2","abc123secret","sk-live-EXAMPLE","eyJEXAMPLE","session=EXAMPLE-5150","BEGIN PRIVATE KEY"] { assert!(!s.contains(absent)); } assert!(s.contains("<USER_HOME>")); assert!(s.contains("<LOCAL_IP>")); assert!(s.contains("<DEVICE_SERIAL>")); }
#[test] fn developer_detail_keeps_paths_but_strips_unique_ids() { /* paths + project names + exe present; sid/guid/mac/ip/username/device-serial absent; ALL six secret shapes absent */ }
#[test] fn full_forensics_keeps_identity_but_enforces_secret_invariant() { /* identity values (incl. DeviceSerial fixture) present; "password=hunter2", "token=abc123secret", "api_key=sk-live-EXAMPLE", "Authorization: Bearer", "Cookie: session=", "BEGIN PRIVATE KEY" still redacted */ }
#[test] fn safe_share_issue_body_derives_only_from_transformed_model() { /* build issue body from transformed model; assert no identity values in title/body */ }
#[test] fn filenames_never_contain_content() { /* section entry name derivation == fixed names, assert no project/user content */ }
```
  Run: `cargo test --locked diagnostics::bundle diagnostics::redact_export` → **RED: types/fns missing**.
- [ ] Step 2: GREEN — implement both modules per signatures. `apply_budget` measures serialized section sizes into `manifest.section_sizes`, then trims/drops in reverse priority. `transform` implements `fn rewrite_json(v: &serde_json::Value, subs: &[(String, String)]) -> serde_json::Value` recursively (object/array/string/number/bool/null preserved, strings substituted); text sections use the same substitution list built from `identity` (+ redactor pass); substitution list built once per export.
- [ ] Step 3: `cargo test --locked diagnostics::bundle diagnostics::redact_export` → GREEN.
- [ ] Step 4: `cargo test --locked && cargo check --locked`.
- [ ] Step 5: Commit: `git add src-tauri/src/diagnostics/bundle.rs src-tauri/src/diagnostics/redact_export.rs && git commit -m "feat: typed bundle model with structural privacy transforms"`

### Task 8: DPAPI encryption + `.lsdiag` frame (windows-sys, current-user scope)

**Files:**
- Create: `src-tauri/src/diagnostics/crypto.rs`
- Modify: `src-tauri/Cargo.toml` (windows-sys features `Win32_Security_Cryptography`; add `Win32_System_VariantFlags` only if cargo check demands it)
- Test: `crypto.rs` tests (roundtrip gated `#[cfg(windows)]`, always-compiled negative tests host-independent)

**Interfaces:**
- Consumes: Task 1 `paths`; spec §18 (DPAPI current-user only, no custom key/entropy beyond optional static description string).
- Produces: `pub fn dpapi_protect(plain: &[u8]) -> Result<Vec<u8>, CryptoError>`; `pub fn dpapi_unprotect(cipher: &[u8]) -> Result<Vec<u8>, CryptoError>`; `pub enum CryptoError { EmptyPayload, PayloadTooLarge, ApiFailed(u32) }`; **TOTAL length conversion:** `pub(crate) fn checked_cb_data(len: usize) -> Result<u32, CryptoError>` — `if len > u32::MAX as usize { return Err(CryptoError::PayloadTooLarge); } Ok(len as u32)`; BOTH DPAPI entry points fill `CRYPT_INTEGER_BLOB.cbData` ONLY through `checked_cb_data?` (no unchecked `as u32` anywhere — correct independently of the 50 MiB budget); frame: `pub fn write_lsdiag(path: &Path, payload_plain: &[u8]) -> Result<(), CryptoError>` (persisted via Task 1 `paths::write_atomic` — no tempfile crate) and `pub fn read_lsdiag(path: &Path) -> Result<Vec<u8>, CryptoError>` using on-disk frame `"LSDIAG1"` magic (7 bytes) + `u32` LE cipher length + DPAPI blob; blake3 digest of the **fully serialized pre-encryption payload bytes** (the exact bytes passed to `dpapi_protect`) is recorded by callers in the manifest as `blake3_digest_note` — the digest never hashes anything containing itself (manifest stores it adjacent to the section payload digest, not over a structure that embeds it).

- [ ] Step 1: RED tests:
```rust
#[test] fn empty_payload_is_rejected_without_api_call() { assert!(matches!(dpapi_protect(&[]), Err(CryptoError::EmptyPayload))); }
#[cfg(windows)] #[test] fn dpapi_roundtrip_recovers_payload() { let m = b"phase-11c-dpapi-roundtrip"; let c = dpapi_protect(m).unwrap(); assert_ne!(c, m.to_vec()); assert_eq!(dpapi_unprotect(&c).unwrap(), m.to_vec()); }
#[cfg(windows)] #[test] fn lsdiag_file_roundtrip_and_magic() { /* write_lsdiag to a path under paths::temp_dir(); bytes start with b"LSDIAG1"; read_lsdiag returns the plaintext */ }
#[test] fn corrupt_ciphertext_is_an_error_not_a_panic() { /* feed dpapi_unprotect(known-good-magic-prefix + garbage) under cfg(windows); must return Err, never panic; on non-Windows this test is cfg'd out with a comment */ }
#[test] fn encrypted_frame_never_contains_plaintext() { /* cfg(windows): plaintext "PLAINTEXT-CANARY-9f3a" inside payload; assert !frame.contains(canary) */ }
#[test] fn oversized_length_rejected_before_api_call() { /* checked_cb_data(usize::MAX) -> Err(PayloadTooLarge); checked_cb_data(64) -> Ok(64); seam-tested directly — no >4 GiB allocation ever occurs in tests */ }
```
  Run: `cd src-tauri && cargo test --locked diagnostics::crypto` → **RED: unresolved `dpapi_protect` + missing windows-sys feature** (cargo error names the feature — add `Win32_Security_Cryptography` exactly there).
- [ ] Step 2: GREEN — implement with the verified signatures (§43-D):
```rust
#[cfg(windows)]
pub fn dpapi_protect(plain: &[u8]) -> Result<Vec<u8>, CryptoError> {
    use windows_sys::Win32::Security::Cryptography::{CryptProtectData, CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN};
    if plain.is_empty() { return Err(CryptoError::EmptyPayload); }
    let cb = checked_cb_data(plain.len())?; // total conversion — PayloadTooLarge BEFORE any Windows call
    unsafe {
        let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
        let inp = CRYPT_INTEGER_BLOB { cbData: cb, pbData: plain.as_ptr() as *mut u8 };
        let ok = CryptProtectData(&inp, std::ptr::null(), std::ptr::null(), std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut out);
        if ok == 0 { return Err(CryptoError::ApiFailed(/* GetLastError() as in existing native modules */0)); }
        let owned = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        windows_sys::Win32::Foundation::LocalFree(out.pbData as _); // always free the DPAPI-allocated buffer
        Ok(owned)
    }
}
```
  (Symmetric `dpapi_unprotect` copies its output blob, then `LocalFree`s both the output buffer and the returned `ppszdatadescr` PWSTR.) Frame: `write_lsdiag` serializes payload → `dpapi_protect` → writes magic+len+blob via Task 1 `paths::write_atomic` (temp in `diagnostics/temp/` + same-volume rename + best-effort cleanup) for atomicity.)
- [ ] Step 3: `cargo test --locked diagnostics::crypto` → GREEN (roundtrip/canary tests run on this Windows host).
- [ ] Step 4: `cargo check --locked && cargo test --locked` (whole suite; Cargo.lock change is features-only — verify `git diff src-tauri/Cargo.lock` shows only the windows-sys entry).
- [ ] Step 5: Commit: `git add src-tauri/src/diagnostics/crypto.rs src-tauri/Cargo.toml src-tauri/Cargo.lock && git commit -m "feat: DPAPI encryption for diagnostics bundles via windows-sys"`

### Task 9: Startup crash-recovery pipeline

**Files:**
- Create: `src-tauri/src/diagnostics/recovery.rs`
- Modify: `src-tauri/src/lib.rs` (invoke `recovery::finalize_pending()` on the startup path after emergency prep, before window show; outcome surfaced to frontend via Task 14 command)
- Test: `recovery.rs` tests

**Interfaces:**
- Consumes: Task 6 `EmergencyRecordV1`, Task 1 `paths::{emergency_dir, bundles_dir}`, Task 8 `write_lsdiag`, Task 2 index.
- Produces: `pub struct RecoveryOutcome { pub recovered_bundle_id: Option<String>, pub attempts_used: u8, pub attempts_recorded: u8, pub moved_to_failed: bool }`; `pub fn finalize_pending(deps: &RecoveryDeps) -> RecoveryOutcome` with injectable `RecoveryDeps { record_bytes: Option<Vec<u8>>, record_exists: bool, app_version: String, attempt_state_path: PathBuf, write_bundle: Box<dyn Fn(&[u8], &str) -> Result<String, String>>, set_banner: Box<dyn Fn(RecoveryOutcome)> }` — sequence: validate size (≤524,288) + schema → re-redact (defense-in-depth) → enrich (Task 5 snapshot if available, omission-safe) → build bundle → `write_bundle` (encrypt+persist+index; Task 10 closure) → **only after durable success delete the marker** → banner. Attempts counter persisted at `attempt_state_path` (max 3): failure increments; on 3rd failure move marker + attempt state into `failed/` (typed `FailedRecoveryRecord`), stop retrying, keep banner state "recovery failed". Fatal-startup failures reuse the same emergency/finalization model (record path already in place from Task 6).

- [ ] Step 1: RED tests:
```rust
#[test] fn successful_recovery_deletes_marker_only_after_bundle_written() { /* fake write_bundle records call order: marker deletion observed AFTER write_bundle Ok; on Err the marker survives */ }
#[test] fn three_failed_attempts_move_to_failed_and_stop() { /* 3 Err runs -> moved_to_failed, 4th call does nothing */ }
#[test] fn oversized_or_wrong_schema_marker_is_discarded_safely() { /* bytes > 524_288 or schema_version != 1 -> no bundle, marker removed into failed/, banner "invalid marker" */ }
#[test] fn enrichment_failure_still_recovers_core() { /* enrichment closure panics/errs -> bundle built without enrichment section */ }
```
  Run: `cargo test --locked diagnostics::recovery` → **RED: missing fns**.
- [ ] Step 2: GREEN — implement per sequence; `set_banner` is a boxed closure so Task 18's notifier can be injected later without re-plumbing.
- [ ] Step 3: `cargo test --locked diagnostics::recovery` → GREEN.
- [ ] Step 4: `cargo test --locked && cargo check --locked`.
- [ ] Step 5: Commit: `git add src-tauri/src/diagnostics/recovery.rs src-tauri/src/lib.rs && git commit -m "feat: startup crash recovery with 3-attempt bound and failed/ escalation"`

### Task 10: Bundle registry, crash-safe retention transaction, reconciliation

**Files:**
- Create: `src-tauri/src/diagnostics/store.rs`
- Modify: `src-tauri/src/diagnostics/mod.rs` (wire)
- Test: `store.rs` tests

**Interfaces:**
- Consumes: Task 8 frame + `Task 2 ids::random_hex` (bundle IDs), Task 2 `IncidentIndex.pending_deletions` contract, Task 1 `paths::{write_atomic, purge_stale_temp}`.
- Produces: `pub struct BundleMeta { bundle_id, created_at_ms, trigger, subsystem, severity, fingerprint, encrypted_size_bytes: u64, integrity: BundleIntegrityState, review_state, app_version, schema_version: u32 }`; `pub enum BundleIntegrityState { Valid, Corrupt, Unsupported, RecoveryRequired }`; `pub struct BundleRegistry { index_path: PathBuf, bundles_dir: PathBuf }`; bundle-index schema v1 gains `pending_deletions: Vec<String>` (**tombstoned owned bundle filenames** — durable deletion intent). `pub fn commit_bundle(&self, plain_payload: &[u8], meta_fields: MetaFields) -> Result<CommitOutcome, CommitError>` implementing the **crash-safe tombstoned sequence**: (1) `bundle_id` = `ids::random_hex(12)` (opaque, content-independent); (2) serialize `BundleModel` → blake3 digest → `dpapi_protect` → Task 1 `write_atomic` directly into `bundles/bundle-<id>.lsdiag` (owned; temp+rename handled inside the helper) → validate size (50 MiB logical budget already applied by Task 7; encrypted cap enforced here: reject > 50 MiB with `CommitError::TooLarge`) → (3) lock registry `Mutex` → compute eviction candidates (retention: count > 20, retained encrypted bytes > 1 GiB **target** — bounded transaction overshoot measured against temp+final bytes is tolerated and logged) ordered oldest-`created_at_ms`-first (trusted index timestamps, **never mtime**) → (4) **first durable index commit**: includes the new bundle, excludes evictions from the active set, and APPENDS every evicted bundle filename to `pending_deletions` → (5) **only then** best-effort delete each tombstoned file (strict shape `bundle-[0-9a-f]{24}.lsdiag`, regular file, resolved-path containment beneath `bundles_dir`, reject reparse/junction/symlink via metadata checks) → (6) **second durable index commit**: remove successfully deleted entries from `pending_deletions` (failed deletions stay tombstoned for bounded startup retry) → (7) unlock. A temporary transactional overshoot is acceptable and is safer than deleting user diagnostics before the replacement state is durable. `pub fn reconcile_startup(&self)` handles: **tombstoned owned file present → DELETE it, NEVER re-index** (deletion intent is durable; an old evicted file must never resurrect as an active bundle); tombstoned ID whose file is missing → clear the tombstone; tombstoned file that failed deletion → retain the tombstone for the next bounded retry; indexed valid file (no-op); **unindexed owned bundle WITHOUT a tombstone** (strict-shape owned file present but absent from index and from `pending_deletions` → ONE bounded decrypt/integrity attempt → success: rebuild meta from manifest → `Valid`; unsupported major schema → `Unsupported`; DPAPI/integrity failure → `Corrupt`; deliberately skipped → `RecoveryRequired`); missing indexed bundle (drop meta entry); malformed index (`Corrupt` state, rebuild minimal metadata from successful decrypts only); orphan `*.tmp` in temp/ via Task 1 `purge_stale_temp` (strict owned shape only); reparse-point escape attempt (reject + breadcrumb, never delete). Delete command path uses the same containment rules (fail closed).

- [ ] Step 1: RED tests:
```rust
#[test] fn evicted_files_deleted_only_after_durable_index_commit() { /* instrumented fs: persist index fails -> evicted files still present; success path -> deleted after */ }
#[test] fn index_commit_failure_preserves_evicted_candidates() { /* Review-Focus #4: make index save fail (injected failure closure) -> old bundles remain, new bundle file remains as unindexed owned artifact, error typed */ }
#[test] fn retention_enforces_20_bundle_cap_oldest_first() { /* 21 commits -> oldest removed, by trusted createdAt not mtime (set mtimes misleadingly; assert unaffected) */ }
#[test] fn retention_enforces_1GiB_target_with_overshoot_tolerance() { /* sized bundles summing > 1 GiB -> trimmed; an in-flight transaction may momentarily exceed; final retained <= 1 GiB + bounded overshoot */ }
#[test] fn delete_rejects_foreign_filename_shapes() { /* index entry manipulated to point at "evil.txt" in bundles dir -> delete refuses */ }
#[test] fn delete_rejects_reparse_escape() { /* create a junction/symlink under bundles_dir pointing outside -> delete refuses + breadcrumb; on non-Windows use symlink; cfg-gate junction variant to windows */ }
#[test] fn crash_A_finalize_without_index_commit_recovers_new_keeps_old() { /* bundle file finalized, index commit injected-fail -> reconcile_startup: new bundle recovered Valid (no tombstone), ALL old active bundles intact */ }
#[test] fn crash_B_tombstone_committed_file_delete_pending_never_recovers() { /* index committed with pending_deletions + old file still on disk, then simulated crash -> reconcile_startup DELETES the tombstoned file; assert it is NEVER re-indexed and NEVER appears as active meta */ }
#[test] fn crash_C_deletion_failure_keeps_tombstone() { /* injected delete failure for a tombstoned file -> file remains, tombstone REMAINS in pending_deletions, breadcrumb once */ }
#[test] fn crash_D_partial_deletion_before_tombstone_clear_is_safe() { /* some tombstoned files deleted before clear-commit crash -> reconcile: missing files clear their tombstones, still-present tombstoned files are deleted, never re-indexed */ }
#[test] fn crash_E_malformed_or_foreign_tombstone_fails_closed() { /* tombstone entry with non-strict shape / reparse target -> no deletion, breadcrumb, fail closed */ }
#[test] fn unindexed_owned_bundle_without_tombstone_recovered_to_valid() { /* valid encrypted file missing from index AND absent from pending_deletions -> reconcile -> meta Valid + indexed; a file LISTED IN pending_deletions is never recovered */ }
#[test] fn unsupported_schema_not_labeled_corrupt() { /* manifest schema 2 -> Unsupported; index loss does NOT mark payloads corrupt */ }
#[test] fn dpapi_corrupt_bundle_marked_corrupt() { /* magic ok, ciphertext garbage -> Corrupt; second reconcile run does NOT retry decrypt (breadcrumb once) */ }
#[test] fn missing_index_rebuilds_minimal_metadata() { /* delete index.json; reconcile -> minimal metas from decrypt-validated bundles; NOT Corrupt payloads; tombstones (if present) still honored */ }
#[test] fn orphan_tmp_cleaned_strictly() { /* temp/atw-<16hex>.tmp (Task 1 owned shape) + temp/not-ours.txt -> only the owned strict-shape tmp removed via purge_stale_temp */ }
```
  Run: `cargo test --locked diagnostics::store` → **RED: missing types/fns**.
- [ ] Step 2: GREEN — implement per tombstoned sequence (Interfaces above); index schema v1 with `serde(default)` field tolerance; **evicted files are never deleted before the first durable index commit, and a `pending_deletions` tombstone is only ever cleared after the file is confirmed gone.**
- [ ] Step 3: `cargo test --locked diagnostics::store` → GREEN.
- [ ] Step 4: `cargo test --locked && cargo check --locked`.
- [ ] Step 5: Commit: `git add src-tauri/src/diagnostics/store.rs src-tauri/src/diagnostics/mod.rs && git commit -m "feat: crash-safe bundle retention with deletion tombstones and startup reconciliation"`

### Task 11: Managed LogRing snapshot collector

**Files:**
- Create: collector fn inside `src-tauri/src/diagnostics/collectors.rs` (created in Task 12; to keep Task 11 self-contained, create `collectors.rs` here with ONLY this fn and grow it in Task 12)
- Modify: `src-tauri/src/workspace/mod.rs` (one `pub(crate) fn snapshot_managed_log_tails() -> Vec<(String /*service id*/, Vec<LogLine>)>` beside the owner at `mod.rs:487`, implemented via `logs.since(0)` under the existing registry lock — pure read)
- Test: `collectors.rs` tests

**Interfaces:**
- Consumes: `LogLine` (`workspace/rules.rs:440`), the Task-5-era accessor; Task 7 `ManagedServiceOutput` section content.
- Produces: `pub fn collect_managed_output() -> SectionContent::ManagedOutput(Vec<ManagedServiceOutput>)` where `ManagedServiceOutput { service_id: String, lines: Vec<String> /*≤100*/, truncated_lines: bool, bytes: u64, truncated_bytes: bool }`; constants `MANAGED_LINES_PER_SERVICE = 100`, `MANAGED_BYTES_PER_SERVICE = 262_144`, `MANAGED_TOTAL_BYTES = 5_242_880`.

- [ ] Step 1: RED tests:
```rust
#[test] fn per_service_line_cap_100() { /* feed 150 LogLines -> 100 kept, truncated_lines == true, kept are the LAST 100 */ }
#[test] fn per_service_byte_cap_256kib() { /* lines totalling > 262_144 bytes -> trimmed at byte boundary from the tail with explicit marker line "[TRUNCATED: original output exceeded diagnostic limit]" */ }
#[test] fn combined_cap_5MiB_across_services() { /* 25 services x 256 KiB -> total trimmed to 5_242_880 with per-service markers preserved; lowest-priority services trimmed first (deterministic: largest byte count first, id tie-break) */ }
#[test] fn redaction_pass_applies_to_every_line() { /* line containing token fixture -> redacted in output */ }
#[test] fn no_scraping_of_external_processes() { /* collector signature takes no pid/handle argument; test asserts the accessor list used == the managed registry only (compile-time: fn signature) */ }
```
  Run: `cargo test --locked diagnostics::collectors` → **RED: missing fn/constants**.
- [ ] Step 2: GREEN — implement; tail truncation deterministic (keep last 100 lines / last 256 KiB; combined budget trims services deterministically as tested).
- [ ] Step 3: `cargo test --locked diagnostics::collectors` → GREEN.
- [ ] Step 4: `cargo test --locked && cargo check --locked`.
- [ ] Step 5: Commit: `git add src-tauri/src/diagnostics/collectors.rs src-tauri/src/workspace/mod.rs && git commit -m "feat: managed output collector with deterministic bounded tails"`

### Task 12: Deep-health engine + bounded Windows collectors (incl. identity)

**Files:**
- Create: full `src-tauri/src/diagnostics/collectors.rs` growth + `src-tauri/src/diagnostics/health.rs`
- Modify: `src-tauri/Cargo.toml` (windows-sys features: `Win32_System_SystemInformation`, `Win32_Graphics_Dxgi` — exact names; `Win32_System_Threading`/`Win32_NetworkManagement_IpHelper`/`Win32_Networking_WinSock` are already enabled (Cargo.toml:50-66), and `Win32_Security_Authorization` is added for the SID chain; add any flag cargo names during the RED cycle)
- Test: `health.rs` + `collectors.rs` tests

**Interfaces:**
- Consumes: Task 1 `paths`, existing docker/AI cached status, §43-E accessors, §43-F API choices.
- Produces: `pub struct HealthCheckResult { id: String, subsystem: &'static str, label: String, status: HealthStatus, code: String, summary: String, detail: Option<String>, checked_at_ms: u64, duration_ms: u64 }`; `pub enum HealthStatus { Healthy, Degraded, Unavailable, Skipped }`; `pub fn run_deep_checks(timeout_per_probe_ms: u64) -> Vec<HealthCheckResult>` — each probe on its own thread with `recv_timeout` join (one stuck probe cannot block the rest; on timeout → `Degraded` + code `TIMEOUT`), categories A/B/C per spec §22 (internal/integrations/windows); collectors: `collect_system_identity()` (`IdentityField` list via §43-F APIs, every field `Result<Option<_>>` → omitted→`Skipped`), `collect_webview2_version()` (registry `BLBeacon\version`), `collect_gpu()`, `collect_network_identity()`, `collect_windows_version()`, plus LocalStack-owned write test (tiny file inside `temp_dir()`, write+delete, `Skipped` never faked as error); SID via the exact §43-F chain (`OpenProcessToken` → `GetTokenInformation(TokenUser)` → `ConvertSidToStringSidW` + `LocalFree`, errors → `Skipped`). **DEVICE/DISK SERIAL — EXPLICIT PHASE 11C DECISION: the collector returns `Skipped`/`Unsupported` by design** — no existing narrow repository API exists and storage-device enumeration would materially expand the native API surface for low support value; `IdentityKind::DeviceSerial` and its generalization remain in the structural-privacy schema/fixtures (Task 7) and Full Forensics semantics so a future collector slots in without schema change; NO WMI/PowerShell/WMIC is added for it. **No WMI/PowerShell/shell runner anywhere; Docker read-only; AI loopback-only observability; all non-elevated.**

- [ ] Step 1: RED tests:
```rust
#[test] fn stuck_probe_is_isolated_by_timeout() { /* register a probe sleeping 10 s with timeout 100 ms -> result Degraded/TIMEOUT, wall clock < 2 s, other probes still return */ }
#[test] fn optional_dependency_absent_yields_skipped() { /* inject unavailable docker status -> Docker probe returns Skipped with explanatory code, NOT Unavailable */ }
#[test] fn health_status_text_is_explicit() { /* every HealthStatus maps to a distinct human string used by UI (Healthy/Degraded/Unavailable/Skipped) */ }
#[test] fn identity_collectors_return_skipped_not_error_when_unavailable() { /* inject failing registry/token handles -> field omitted, Skipped; SID token-info failure seam maps to Skipped */ }
#[test] fn device_serial_collector_is_skipped_by_design() { /* collect_system_identity() includes NO DeviceSerial value in Phase 11C; the field reports Skipped/Unsupported with an explanatory code, not an error */ }
#[test] fn write_probe_confined_to_diagnostics_temp() { /* probe path starts_with paths::temp_dir(); creates+deletes exactly one file; assert no other fs effect (count dir entries before/after) */ }
```
  Run: `cargo test --locked diagnostics::health diagnostics::collectors` → **RED: missing types**.
- [ ] Step 2: GREEN — implement engine (thread-per-probe + timeout join via channel) and collectors per §43-F; features added to Cargo.toml in this task (RED cycle names them).
- [ ] Step 3: `cargo test --locked diagnostics::health diagnostics::collectors` → GREEN.
- [ ] Step 4: `cargo test --locked && cargo check --locked` (Cargo.lock diff = windows-sys feature flags only).
- [ ] Step 5: Commit: `git add src-tauri/src/diagnostics/health.rs src-tauri/src/diagnostics/collectors.rs src-tauri/Cargo.toml src-tauri/Cargo.lock && git commit -m "feat: deep health engine with bounded non-elevated Windows collectors"`

### Task 13: Export pipeline — ZIP assembly, native save dialog, export capability

**Files:**
- Create: `src-tauri/src/diagnostics/export.rs`
- Modify: `src-tauri/Cargo.toml` (add `zip = { version = "2", default-features = false, features = ["deflate"] }`, `tauri-plugin-dialog = "2"`, `tauri-plugin-opener = "2"` — **Rust crates only: NO npm `@tauri-apps/plugin-dialog`/`plugin-opener`, NO capability/permission grants** (plugin permission sets gate only their own JS commands, which are never granted or used — §43-A)), `src-tauri/src/lib.rs` (`.plugin(tauri_plugin_dialog::init())`, `.plugin(tauri_plugin_opener::init())`)
- Test: `export.rs` tests (pure parts; dialog parts behind injected trait)

**Interfaces:**
- Consumes: Task 7 `transform`, Task 8 frame, Task 10 registry, Task 12 identity fields.
- Produces: `pub struct ExportCapability { pub export_id: String /* opaque 24-hex via Task 2 ids::random_hex — non-content-derived */, pub destination: PathBuf, pub created_at_ms: u64 }`; in-memory `Mutex<HashMap<String, ExportCapability>>` — **semantics PINNED: max 8 entries (FIFO eviction), 10-minute TTL, in-memory only (invalid after process restart), bound to exactly one trusted user-selected export destination, ONE-SHOT after successful reveal** — `pub fn reveal_export_result(cap: &ExportCapabilityStore, export_id: &str) -> Result<(), ExportError>`: verify membership + TTL → `tauri_plugin_opener::open_path(dest_parent)` → **on opener success REMOVE the capability immediately**; on opener failure the capability REMAINS for retry until TTL/eviction; expired/evicted/restarted → `ExportError::UnknownExport`; **no frontend path can ever create or renew a capability** (only `export_bundle` — which itself only runs after a trusted native save — registers one); `pub fn export_bundle(registry: &BundleRegistry, bundle_id: &str, profile: PrivacyProfile, confirm_full_forensics: bool, save: &dyn Fn() -> Result<Option<PathBuf>, String>) -> Result<ExportOutcome, ExportError>`; `pub enum ExportError { UnknownBundle, UnknownExport, CorruptSource, TooLarge, SaveCancelled, SaveFailed(String), FullForensicsUnconfirmed }`; `pub struct ExportOutcome { pub export_id: String, pub file_name: String, pub profile: PrivacyProfile }`; pure ZIP builder `pub fn build_zip_bytes(model: &BundleModel) -> Vec<u8>` (entry names = fixed section names; manifest + `README-PRIVACY.txt` documenting profile). Full Forensics: `confirm_full_forensics == false` → `Err(FullForensicsUnconfirmed)` — **user-intent gate, not cryptographic authorization** (UI provides the fresh per-export confirm; backend validates bundle ID + profile + trusted flow). NOTE: the dedicated blocking thread that hosts the save dialog is the export command's own — it is NOT the automatic diagnostics capture worker (Task 4).

- [ ] Step 1: RED tests:
```rust
#[test] fn full_forensics_without_confirmation_is_rejected() { assert!(matches!(export_bundle(..., PrivacyProfile::FullForensics, false, ...), Err(FullForensicsUnconfirmed))); }
#[test] fn safe_share_export_contains_no_identity_fixture() { /* Review-Focus #2: bundle with alice/DESKTOP-XYZ/SID fixtures -> build zip from transformed model -> assert !bytes contain fixtures; README-PRIVACY present */ }
#[test] fn corrupt_source_refuses_export() { /* integrity Corrupt bundle -> Err(CorruptSource), no file written */ }
#[test] fn cancelled_save_writes_nothing() { /* save closure returns Ok(None) -> SaveCancelled, no capability registered */ }
#[test] fn export_capability_registry_is_bounded_and_expiring() { /* 9 exports -> oldest evicted; capability older than 10 min -> reveal rejects */ }
#[test] fn successful_reveal_consumes_capability_one_shot() { /* reveal -> Ok, capability REMOVED; second reveal of the same export_id -> Err(UnknownExport) */ }
#[test] fn failed_opener_preserves_capability_for_retry() { /* injected opener failure -> reveal -> Err; capability STILL present; retry with succeeding opener -> Ok */ }
#[test] fn restart_invalidates_all_export_ids() { /* fresh ExportCapabilityStore::new() (the restart shape) -> old export_id -> Err(UnknownExport) */ }
#[test] fn frontend_path_never_creates_or_renews_capability() { /* no public fn accepts a PathBuf for registration besides export_bundle's injected trusted save; a fabricated export_id shaped like a path simply fails membership; registry mutation is private */ }
#[test] fn reveal_never_accepts_frontend_path() { /* reveal signature takes export_id only; passing path-like strings as export_id simply fails membership */ }
#[test] fn github_safe_share_body_originates_from_transformed_model() { /* build issue title/body fn; assert fixtures absent, length-bounded (title ≤ 120 chars, body ≤ 8_000 chars) */ }
```
  Run: `cargo test --locked diagnostics::export` → **RED: missing types**.
- [ ] Step 2: GREEN — implement; dialog integration: `save` injected in tests, real impl wraps `tauri_plugin_dialog`'s blocking save-file builder called from the worker thread (§43-A).
- [ ] Step 3: `cargo test --locked diagnostics::export` → GREEN.
- [ ] Step 4: `cargo test --locked && cargo check --locked` (plugin deps resolve; Cargo.lock reviewed for zip/dialog/opener entries only).
- [ ] Step 5: Commit: `git add src-tauri/src/diagnostics/export.rs src-tauri/Cargo.toml src-tauri/Cargo.lock src-tauri/src/lib.rs && git commit -m "feat: privacy-profiled ZIP export with native save and reveal capability"`

### Task 14: Rust Tauri command surface (narrow, capability-shaped)

**Files:**
- Create: `src-tauri/src/diagnostics/commands.rs`
- Modify: `src-tauri/src/lib.rs` (`generate_handler![...]` additions — exact list below), `src-tauri/src/app_commands.rs` (NO change expected; verify)
- Test: `commands.rs` tests + `src/safetyGuards.test.ts` (stays green, run as regression)

**Interfaces:**
- Consumes: Tasks 2–13 public APIs.
- Produces — the complete final command list (exact signatures; all frontend inputs are opaque IDs or enums; **no path parameters anywhere**):
```rust
get_diagnostics_overview() -> DiagnosticsOverviewDto
run_deep_health_checks() -> Vec<HealthCheckResultDto>
list_incidents() -> Vec<IncidentDto>
mark_incident_reviewed(incident_id: String) -> Result<(), String>
list_support_bundles() -> Vec<BundleMetaDto>
get_support_bundle_detail(bundle_id: String) -> Result<BundleDetailDto, String>
export_support_bundle(bundle_id: String, profile: ExportProfileDto, confirmed: bool) -> Result<ExportOutcomeDto, String>
delete_support_bundle(bundle_id: String) -> Result<(), String>
open_diagnostics_folder() -> Result<(), String>
get_support_summary(bundle_id: Option<String>) -> SupportSummaryDto // Safe-Share-only
prepare_localstack_github_issue(bundle_id: Option<String>) -> GitHubIssueDraftDto // title/body Safe-Share, fixed URL assembled frontend-side from constant
update_diagnostics_context(view_id: u8) -> ()                       // validated 0..NAV_LEN, navigation-time only, no panic-time IPC
reveal_export_result(export_id: String) -> Result<(), String>       // capability-shaped; exact export destination only
```
  Every handler: validate → delegate → map error to typed string codes; `prepare_localstack_github_issue` returns title/body/URL fragment; the fixed base URL `https://github.com/Zehna/Lokalstack/issues/new` is a Rust-side constant echoed in the DTO (never user input).

- [ ] Step 1: RED tests:
```rust
#[test] fn command_signatures_take_no_filesystem_paths() { /* compile-time: a test module asserting via type-level helper that each exported command fn's parameters contain no PathBuf (implemented as a const assertion over a generated signature list or explicit per-fn type aliases) */ }
#[test] fn mark_reviewed_with_unknown_id_fails_typed() { /* -> Err("unknown-incident") */ }
#[test] fn update_diagnostics_context_validates_range() { /* 200 -> ignored/false; valid -> cached */ }
#[test] fn overview_aggregates_real_state() { /* seeded registry/incidents -> overview counts correct */ }
```
  Run: `cargo test --locked diagnostics::commands` → **RED: module missing**.
- [ ] Step 2: GREEN — implement handlers as thin delegations; register exactly the 13 fns in `lib.rs::generate_handler!`.
- [ ] Step 3: `cargo test --locked diagnostics::commands && cd .. && npm run test:run -- src/safetyGuards.test.ts` → both GREEN.
- [ ] Step 4: `cargo test --locked && cargo check --locked`.
- [ ] Step 5: Commit: `git add src-tauri/src/diagnostics/commands.rs src-tauri/src/lib.rs && git commit -m "feat: narrow diagnostics Tauri command surface"`

### Task 15: Frontend native diagnostics client + DTO contract

**Files:**
- Create: `src/services/native/diagnostics.ts`
- Modify: `src/types/domain.ts` (DTO types: `DiagnosticsOverviewDto`, `HealthCheckResultDto`, `IncidentDto`, `BundleMetaDto`, `BundleDetailDto`, `ExportProfileDto`, `ExportOutcomeDto`, `SupportSummaryDto`, `GitHubIssueDraftDto`)
- Test: `src/services/native/diagnostics.test.ts`

**Interfaces:**
- Consumes: Task 14 command names/signatures (exact string literals).
- Produces: typed wrappers `getDiagnosticsOverview()`, `runDeepHealthChecks()`, `listIncidents()`, `markIncidentReviewed(id)`, `listSupportBundles()`, `getSupportBundleDetail(id)`, `exportSupportBundle(id, profile, confirmed)`, `deleteSupportBundle(id)`, `openDiagnosticsFolder()`, `getSupportSummary(id?)`, `prepareLocalstackGithubIssue(id?)`, `updateDiagnosticsContext(view: ViewId)`, `revealExportResult(exportId)` — each a single `invoke<T>('cmd_name', { args })` with DTO types from `domain.ts`.

- [ ] Step 1: RED test (`diagnostics.test.ts`):
```ts
it('wraps exact backend command names with typed DTOs', () => {
  vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async (cmd: string) => ({ cmd })) }))
  // call each wrapper; assert invoke called with exact Task-14 command names and arg keys
})
it('updateDiagnosticsContext sends validated ViewId', () => { /* 'diagnostics' -> arg view_id matches ViewId position */ })
```
  Run: `npx vitest run src/services/native/diagnostics.test.ts` → **RED: module missing**.
- [ ] Step 2: GREEN — implement wrappers (port pattern from `src/services/native/ports.ts`).
- [ ] Step 3: `npx vitest run src/services/native/diagnostics.test.ts` → GREEN.
- [ ] Step 4: Regression: `npx vitest run src/safetyGuards.test.ts src/services/native` → GREEN (invoke boundary intact).
- [ ] Step 5: Commit: `git add src/services/native/diagnostics.ts src/types/domain.ts src/services/native/diagnostics.test.ts && git commit -m "feat: typed diagnostics native client and DTOs"`

### Task 16: Canonical active view (store as single source of truth) + diagnostics context choke point + Zustand diagnostics store

**Files:**
- Create: `src/stores/diagnosticsStore.ts`
- Modify: `src/app/AppLayout.tsx` (**remove the local `useState<ViewId>('dashboard')`**; read `activeView` from `useAppStore`; `Sidebar.onSelectView` and the `ErrorBoundary` `onNavigateDashboard` both call the canonical `useAppStore.setActiveView`; the `VIEWS` lookup derives from store `activeView`), `src/stores/appStore.ts` (`setActiveView` fires Task 15 `updateDiagnosticsContext(view)` fire-and-forget — the SINGLE choke point for §43-I: update React/Zustand state synchronously first, then `updateDiagnosticsContext(view).catch(() => {})`; navigation NEVER awaits backend IPC and an IPC failure cannot undo navigation), `src/types/domain.ts` (ViewId extension if not landed in Task 15)
- Test: `src/stores/diagnosticsStore.test.ts`, `src/app/AppLayout.navigation.test.tsx` (new; patterns from existing Settings feature tests)

**Interfaces:**
- Consumes: Task 15 client (`updateDiagnosticsContext`).
- Produces: ONE canonical navigation state — `useAppStore.activeView` / `useAppStore.setActiveView` — consumed by AppLayout render, Sidebar, ErrorBoundary, DashboardPage, and the diagnostics context side effect; no second independent active-view state remains (a repo-wide search for `useState<ViewId>` in app layout must return nothing — enforced by test D). Plus `useDiagnosticsStore` with state `{ overview, healthResults, incidents, bundles, deepCheckRunning, deepCheckProgressText, lastBanner, recoveredFromCrash }` and actions `{ loadOverview, runDeepChecks (sets progress text), loadIncidents, markReviewed, loadBundles, markBundleReviewed, dismissBanner }` — all actions call only the Task-15 client; no invoke import (safetyGuards).

- [ ] Step 1: RED tests (canonical navigation — §43-I RED set A–D):
```tsx
// AppLayout.navigation.test.tsx
it('A: store-driven setActiveView(\'ports\') changes the AppLayout-visible active view', () => {
  // render AppLayout with the real appStore; act(() => useAppStore.getState().setActiveView('ports'))
  // assert the ports view is rendered (and NOT the dashboard)
})
it('B: ErrorBoundary dashboard navigation uses the same canonical store setter', () => {
  // trigger the ErrorBoundary onNavigateDashboard path; assert useAppStore activeView === 'dashboard'
  // and NO second state source exists (spy: only the store was written)
})
it('C: updateDiagnosticsContext failure does not prevent the visible view change', () => {
  // mock Task 15 updateDiagnosticsContext to reject; call setActiveView('settings')
  // assert the settings view IS rendered anyway and the rejection was swallowed
})
it('D: no AppLayout-local activeView useState remains', () => {
  // static source assertion: AppLayout.tsx source contains no "useState<ViewId>"
  // (read via import.meta/raw import; mirrors safetyGuards static-check pattern)
})
// diagnosticsStore.test.ts
it('runDeepChecks exposes textual progress', async () => { /* mock client returning 12 checks; assert deepCheckProgressText transitions 'Running deep diagnostics… 0 of 12 checks complete' -> '… 12 of 12 …' and deepCheckRunning true->false */ })
it('banner state survives until dismissed', () => { /* set, dismiss, assert */ })
it('markReviewed updates incident review state locally', () => { /* mock; optimistic update; assert */ })
```
  Run: `npx vitest run src/app/AppLayout.navigation.test.tsx src/stores/diagnosticsStore.test.ts` → **RED: store lacks activeView consolidation (A/B/C fail) and diagnosticsStore is missing**.
- [ ] Step 2: GREEN — remove AppLayout's local `useState<ViewId>`; wire Sidebar/ErrorBoundary/VIEWS to `useAppStore`; add the fire-and-forget context call inside `appStore.setActiveView`; implement `diagnosticsStore` (Zustand pattern from `src/stores/appStore.ts`/`settingsStore.ts`).
- [ ] Step 3: targeted runs → GREEN. Step 4: `npx vitest run && npm run typecheck` → GREEN (existing navigation/dashboard tests still pass — they already write the store). Step 5: Commit: `git add src/app/AppLayout.tsx src/app/AppLayout.navigation.test.tsx src/stores/appStore.ts src/stores/diagnosticsStore.ts src/stores/diagnosticsStore.test.ts && git commit -m "refactor: canonical active view in appStore with diagnostics context choke point; add diagnostics store"`

### Task 17: Diagnostics page, tabs, accessibility, Full Forensics confirmation UX

**Files:**
- Create: `src/features/diagnostics/DiagnosticsPage.tsx`, `src/features/diagnostics/components/{DiagnosticsTabs,OverviewTab,HealthTab,IncidentsTab,BundlesTab,ExportProfileDialog,FullForensicsConfirm,NotificationBanner,GitHubIssueDialog}.tsx`, `src/features/diagnostics/types.ts`, `src/features/diagnostics/DiagnosticsPage.test.tsx`
- Modify: `src/app/navigation.ts` (`{ id: 'diagnostics', label: 'Diagnostics', feature: 'diagnostics' }`), `src/app/AppLayout.tsx` (render DiagnosticsPage for the new ViewId — canonical active view already landed in Task 16; NO local state here)
- Test: `src/features/diagnostics/DiagnosticsPage.test.tsx`, `src/navigation.test.ts` (or existing nav test location)

**Interfaces:**
- Consumes: Task 16 store; existing a11y test patterns (Settings tests).
- Produces: tabbed UI (`role="tablist"`/`tab`/`tabpanel`), all statuses as text (never color-only), labelled filters, keyboard-accessible rows/actions, `FullForensicsConfirm` modal with fresh per-export confirm + focus management + sensitive-category warning, `ExportProfileDialog`, banner (`role="status"`).

- [ ] Step 1: RED tests (key excerpt — full set in test file):
```tsx
it('exposes tabbed navigation with accessible names', () => { /* getByRole('tab', { name: 'Health' }) etc. */ })
it('health status is text, not color-only', () => { /* each row shows 'Healthy'/'Degraded'/'Unavailable'/'Skipped' text */ })
it('full forensics requires fresh confirmation per export', () => { /* open ExportProfileDialog -> choose Full Forensics -> confirm modal appears with category list; cancel aborts; confirm passes confirmed=true to store action */ })
it('export failure surfaces typed error without crashing', () => { /* mock export rejection -> error text visible, page intact */ })
it('banner announces recovery', () => { /* getByRole('status') with recovery text */ })
```
  Run: `npx vitest run src/features/diagnostics` → **RED: components missing**.
- [ ] Step 2: GREEN — implement page/components (Settings page patterns).
- [ ] Step 3: targeted → GREEN.
- [ ] Step 4: Regression: `npx vitest run && npm run typecheck && npm run build` → GREEN.
- [ ] Step 5: Commit: `git add src/features/diagnostics src/app/navigation.ts src/app/AppLayout.tsx src/stores/appStore.ts src/types/domain.ts && git commit -m "feat: diagnostics page with accessible tabs and full-forensics confirmation"`

### Task 18: Notification decision logic (foreground banner vs native toast)

**Files:**
- Create: `src-tauri/src/diagnostics/notify.rs`
- Modify: `src-tauri/Cargo.toml` (`tauri-plugin-notification = "2"`), `src-tauri/src/lib.rs` (`.plugin(tauri_plugin_notification::init())`)
- Test: `notify.rs` tests

**Interfaces:**
- Consumes: foreground state read at decision time by the worker after a capture — **`Window::is_focused()`** on the main app window; **any query error (or no window) → `Focus::Unknown`**; Task 10 commit outcomes.
- Produces: `pub enum Focus { Foreground, Background, Unknown }`; `pub enum NotificationDecision { InAppBanner, NativeToast, Suppress }`; `pub fn decide(focus: Focus, already_notified_this_capture: bool, cooldown_active: bool) -> NotificationDecision` — `Foreground` && !already-notified → `InAppBanner`; `Background` && !already-notified && !cooldown_active → `NativeToast`; **`Unknown` → `InAppBanner` (conservative fallback — never guess background and toast; the banner remains visible when the user returns)**; else `Suppress`. `pub fn focus_of(win: Option<&tauri::Window>) -> Focus` maps `is_focused()` `Ok(true)`→Foreground, `Ok(false)`→Background, `Err(_)`/None→Unknown (pure mapping, independently unit-tested). `pub fn dispatch(decision, text: &str, native: &dyn Fn(&str))` — native closure injected (real impl: `tauri_plugin_notification` Rust API per §43-B — **Rust crate only, no npm package, no capability grant**); all failures swallowed with one bounded `diagnostics::warn` (core app unaffected; a notification-plugin failure never affects bundle persistence). Notification text: fixed templates ("A support bundle was captured.") — never paths/IDs/errors.

- [ ] Step 1: RED tests:
```rust
#[test] fn focus_query_error_falls_back_to_in_app_banner() { /* focus_of(None) == Unknown and decide(Unknown, false, false) == InAppBanner — never NativeToast */ }
#[test] fn foreground_prefers_in_app_banner() { assert!(matches!(decide(Foreground, false, false), InAppBanner)); }
#[test] fn background_uses_native_toast_once() { assert!(matches!(decide(Background, false, false), NativeToast)); }
#[test] fn duplicates_and_cooldowns_are_suppressed() { assert!(matches!(decide(Background, true, false), Suppress)); assert!(matches!(decide(Background, false, true), Suppress)); }
#[test] fn native_failure_is_swallowed() { /* native closure returns Err/panics-isolated? -> closure returns Err; dispatch logs once, no panic, no propagation */ }
#[test] fn notification_text_has_no_sensitive_content() { /* assert template == fixed constant; no interpolation of paths/errors */ }
```
  Run: `cargo test --locked diagnostics::notify` → **RED**.
- [ ] Step 2: GREEN — implement; worker (Task 4 build closure) calls `decide` after successful commit using a captured foreground-check closure.
- [ ] Step 3: targeted → GREEN. Step 4: full suite + check. Step 5: Commit: `git add src-tauri/src/diagnostics/notify.rs src-tauri/Cargo.toml src-tauri/Cargo.lock src-tauri/src/lib.rs && git commit -m "feat: context-aware diagnostics notifications with anti-spam"`

### Task 19: GitHub Safe Share issue workflow (frontend)

**Files:**
- Create: `src/features/diagnostics/components/GitHubIssueDialog.tsx` (if not already created in Task 17, this task owns its behavior), `src/features/diagnostics/GitHubIssueDialog.test.tsx`
- Modify: none beyond Task 17 files
- Test: colocated test

**Interfaces:**
- Consumes: Task 15 `prepareLocalstackGithubIssue`, `exportSupportBundle`, Task 14 DTOs.
- Produces: user flow — explicit button → `prepareLocalstackGithubIssue` (Safe-Share title/body from backend) → dialog shows summary + warning → user confirms → open `draft.issueUrl` (window.open of the **backend-provided fixed URL**) → call `exportSupportBundle(id, SafeShare, false)` → show exported file name → `revealExportResult(exportId)` button. Failure states: prepare fails → typed error, no navigation; export cancelled → message, flow ends cleanly.

- [ ] Step 1: RED tests:
```tsx
it('opens only the backend-provided fixed GitHub URL', () => { /* mock prepare -> issueUrl 'https://github.com/Zehna/Lokalstack/issues/new?title=...' ; assert window.open called with exactly that URL; any other URL never constructed frontend-side */ })
it('failure to prepare shows typed error and never navigates', () => { /* reject -> error text, window.open NOT called */ })
it('cancel leaves no partial export', () => { /* user cancels at confirm -> no export call */ })
```
  Run: `npx vitest run src/features/diagnostics/GitHubIssueDialog.test.tsx` → **RED**.
- [ ] Step 2: GREEN — implement dialog. Step 3: targeted → GREEN. Step 4: `npx vitest run` + typecheck + build. Step 5: Commit: `git add src/features/diagnostics && git commit -m "feat: safe-share GitHub issue preparation workflow"`

### Task 20: Failure-injection / security / privacy source audits

**Files:**
- Create: `src-tauri/src/diagnostics/audits.rs` (`#[cfg(test)]`-only module; no production code), extension to `src/safetyGuards.test.ts` **only if** a new forbidden shape is warranted (default: none)
- Test: `audits.rs` + frontend suite

**Interfaces:**
- Consumes: all diagnostics modules.
- Produces: compile-time/runtime audit tests:
```rust
#[test] fn no_raw_argv_or_env_collection() { /* grep-style: diagnostics modules reference no std::env::args / no command-line argv storage (walk source via include_str! of module files; assert absent) */ }
#[test] fn no_authorization_cookie_or_private_key_collection() { /* include_str! scan for forbidden collection symbols (reqwest absent, no header-map types) */ }
#[test] fn no_arbitrary_url_open_or_fs_delete() { /* assert 'open_url' / 'delete_file(path' / 'remove_dir_all' absent from diagnostics modules except tested contained delete helper */ }
#[test] fn invoke_boundary_frontend() { /* covered by existing safetyGuards.test.ts — run it */ }
```
  Plus §32/§33 gate checklist embedded as comments mapping each spec §38/§39 gate to its owning test (traceability block).

- [ ] Step 1: RED — audits fail on any violation (initially pass trivially only if no violations exist; each assertion cites the spec gate ID).
- [ ] Step 2: run `cargo test --locked diagnostics::audits && npm run test:run -- src/safetyGuards.test.ts` → GREEN.
- [ ] Step 3: Commit: `git add src-tauri/src/diagnostics/audits.rs && git commit -m "test: security and privacy source audits for diagnostics"`

### Task 21: Windows live verification (marked `#[ignore]` / manual)

**Files:**
- Create: `src-tauri/src/diagnostics/live_windows_tests.rs` (`#[cfg(windows)] #[ignore]` tests)
- Test: live run documented in verification report

**Interfaces:**
- Consumes: Tasks 8/10/12/18.
- Produces: live evidence checklist (run locally on this Windows host, non-elevated):
```rust
#[test] #[ignore] fn live_dpapi_roundtrip_current_user() { /* real CryptProtectData/CryptUnprotectData roundtrip in temp dir */ }
#[test] #[ignore] fn live_lsdiag_encrypted_at_rest_and_plaintext_absent() { /* write bundle with canary plaintext; read raw bytes; assert canary absent; assert DPAPI blob header (magic + high-entropy) */ }
#[test] #[ignore] fn live_reparse_junction_escape_rejected() { /* junction under bundles_dir -> outside target; delete/refuse path exercises containment */ }
#[test] #[ignore] fn live_deep_health_non_elevated() { /* run_deep_checks completes without elevation; identity fields populated or Skipped */ }
#[test] #[ignore] fn live_notification_decision_smoke() { /* background decision -> one toast observed manually; record in verification report (no flaky CI automation) */ }
```
  Manual (non-automated) checks recorded in the final verification report: emergency marker → next-startup recovery; max-recovery-attempt behavior; corrupt-bundle UI state; exported ZIP opens in Explorer; Safe Share sanitized; Developer Detail paths retained; Full Forensics fields retained + secret invariant.

- [ ] Step 1: implement tests. Step 2: run `cargo test --locked -- --ignored diagnostics::live` on the Windows host → GREEN evidence. Step 3: Commit: `git add src-tauri/src/diagnostics/live_windows_tests.rs && git commit -m "test: live Windows verification suite for diagnostics"`

### Task 22: Documentation updates + spec/status flip (only after gates pass)

**Files:**
- Modify: `docs/architecture.md` (new "Diagnostics & supportability (Phase 11C)" section describing module map, data flow, privacy profiles, DPAPI, retention; no historical rewrites), `docs/roadmap.md` (Phase 11C entry → implemented status), `README.md` (one short Diagnostics feature bullet), `docs/install-windows.md` (no change expected — verify; adjust only if diagnostics storage affects documented paths), `docs/superpowers/specs/2026-09-18-phase-11c-diagnostics-supportability-design.md` (**header status ONLY**: flip `Status: Review candidate — awaiting approval for implementation planning` → `Status: Implemented (Phase 11C gates passed)` in Task 22 AFTER Task 23's gates are green — never before; the spec body itself is NOT edited)
- Test: n/a (docs)

- [ ] Step 1: write sections grounded in the shipped module map (mirror the Planned File Structure above). **Ordering rule:** Task 22 Step 1 runs BEFORE Task 23; the spec header flip runs AFTER Task 23 reports green (executed as a small follow-up commit inside Task 22's scope, disclosed in the verification report; do not claim implementation status before final gates pass). Step 2: `git diff --check`. Step 3: Commit: `git add docs/architecture.md docs/roadmap.md README.md docs/install-windows.md docs/superpowers/specs/2026-09-18-phase-11c-diagnostics-supportability-design.md && git commit -m "docs: Phase 11C diagnostics and supportability"`

### Task 23: Final regression + security review gates

**Files:**
- Modify: none (verification only)
- Test: full suites

**Interfaces:**
- Consumes: everything.

- [ ] Step 1: `npm run typecheck && npm run test:run && npm run build` → all GREEN (fresh output).
- [ ] Step 2: `cd src-tauri && cargo check --locked && cargo test --locked` → GREEN, zero new warnings.
- [ ] Step 3: `git diff --check` clean.
- [ ] Step 4: Source audits (from Task 20) re-run: `invoke(` resolves only to `src/services/native/`; no forbidden command shapes (`read_file(path)`/`delete_file(path)`/`decrypt_file(path)`/`open_any_folder(path)`/`open_url(user_input)`/`run_probe(command)`); no `Config.Env`/argv/env collection; no auth-header/cookie/private-key collection; no network/upload code paths beyond loopback observability + fixed GitHub URL; no lifecycle/process-control additions; no firewall/ACL/service/registry mutation; no elevation.
- [ ] Step 5: **Phase 11C MUST NOT create:** tag, GitHub Release, version bump — verify `git tag --list 'v1.0.*'` shows exactly `v1.0.0`, `v1.0.1` and `git status --short` clean.
- [ ] Step 6: Open PR → required check **Quality gates (windows-latest)** green → merge via PR (branch protection intact). No post-merge tag/release actions.

### Task 24: Implementation verification report

**Files:**
- Create: `docs/qa/phase-11c-verification-report.md` (project convention: dated QA/verification reports under `docs/qa/`)

**Interfaces:**
- Consumes: all task outputs.
- Produces: the per-task RED→GREEN evidence table, Review Focus outcomes, live Windows results (Task 21), §38/§39 gate sign-offs, any disclosed limitations (e.g., CI-untestable toast rendering verified manually).

- [ ] Step 1: write report with fresh evidence. Step 2: Commit: `git add docs/qa/phase-11c-verification-report.md && git commit -m "docs: Phase 11C verification report"`

---

## Verification matrix (spec § → owning task(s))

| Spec section | Task(s) |
|---|---|
| §4 baseline facade | 1 |
| §6 data model + bounds/eviction | 2 |
| §8 severity model | 2, 3 |
| §9 trigger/cooldown | 3 |
| §10 capture worker | 4 |
| §11–12 panic + recovery | 5, 6, 9 |
| §13 bundle content | 7 |
| §14 internal detail | 12 |
| §15 redaction | 7, 13 |
| §16 managed output | 11 |
| §17 export profiles | 7, 13 |
| §18 DPAPI | 8 |
| §19 size/storage | 7, 10 |
| §20 safe filesystem | 1, 10 |
| §21 index/corruption | 2, 10 |
| §22–23 health | 12 |
| §24 UI model | 17 |
| §25 trust boundary | 14, 15 |
| §26 export model | 13 |
| §27 copy summary | 14, 17 |
| §28 notifications | 18, 17 |
| §29 GitHub workflow | 19 |
| §30 accessibility | 17 |
| §31 performance | 4, 5, 12 (non-blocking, cached, timeout-isolated) |
| §32 failure isolation | 9, 10, 13, 18, 20 |
| §33 retention safety | 10 |
| §34 migration | 1 (lazy dirs; emergency dir prepared), 6 |
| §35 TDD | all tasks (RED→GREEN steps) |
| §36 Windows verification | 21 |
| §37 regression gates | 23 |
| §38/§39 security/privacy gates | 7, 8, 10, 13, 20, 21 |
| §40 scope | 23 (no tag/release/version) |
| §41 definition of done | 23, 24 |
| §42 acceptance criteria | 23, 24 |
| §43 decisions A–I | resolved in this plan (see §43 section) |

## Plan self-review (executed during authoring)

1. **Spec coverage:** every spec section §1–§43 maps to ≥1 task (verification matrix above).
2. **Placeholder scan:** automated pattern sweep (forbidden marker words, open-ended phrases) over the plan returns zero production-content hits; every task carries concrete tests, signatures, and commands. (§43-D notes a conditional secondary windows-sys feature `Win32_System_VariantFlags` — that is a compiler-verified conditional, resolved inside Task 8's RED cycle, not an open decision; §43-F likewise names exact flags verified in Task 12's cycle.)
3. **Placeholder re-sweep after amendment:** `grep -E "TBD|TODO:|implement later|similar to Task|write tests for this|handle errors appropriately"` over the amended plan → zero hits.
3. **Type consistency:** each task's "Consumes" references only interfaces produced by earlier tasks or already existing in the repo (`LogRing` at `workspace/rules.rs:450`, `LogLine` at 440, facade fns, `NAV_ITEMS`, plugin crates added by the owning task).
4. **Review Focus:** all five risk classes have named tests (Review Focus section).
6. **Dependency ordering:** module split → incidents → policy → worker → cache → emergency → bundle model → crypto → recovery → store → collectors → health → export → commands → frontend client → store + canonical view → UI → notify → GitHub flow → audits → live → docs → gates.
6. **TDD:** every task begins with a named RED test and exact command + expected failure reason.
7. **Security/privacy:** §38/§39 gates mapped to tasks 7/8/10/13/20/21 (matrix).
8. **Scope:** no tag/release/version-bump/telemetry/remote work anywhere; Task 23 Step 5 enforces.
9. **Amendment pass (post-review, this commit):** §43-I/Task 16 canonical active view (A–D RED) · Task 4 job-panic isolation (RED) · §43-C/Task 4 std `sync_channel(8)` replaces crossbeam (YAGNI) · Task 1 `write_atomic`/`purge_stale_temp` replaces tempfile (YAGNI) · Task 2 `ids::random_hex` BCryptGenRandom (§43-H; no getrandom direct import) · §43-F/Task 12 real SID chain + explicit device-serial Skipped · §43-A/B/H no npm plugin packages, no capability grants (Rust-side plugin APIs only) · Task 10 tombstones (crash A–E RED; §33) · Task 7/13 full privacy fixture set incl. device-serial + 6 secret shapes · Task 13 one-shot/TTL/restart reveal semantics pinned · Task 18 `Focus::Unknown` → banner (RED) · Task 8 `checked_cb_data` total conversion · Task 22/24 spec-status flip ordering enforced.

## Final handoff

- **Plan path:** `docs/superpowers/plans/2026-09-19-phase-11c-diagnostics-supportability.md`
- **Task count:** 24 tasks + verification matrix + self-review. 5 dependency-verification steps (Task 8 `checked_cb_data`, Task 12 SID/device-serial, Task 13 zip-2.x lock pin) carry explicit GREEN-verification commands.
- **Commit as:** `docs: plan Phase 11C diagnostics supportability`
- **STOP after commit.** No implementation, no push, no PR, no merge, no tag, no release.
