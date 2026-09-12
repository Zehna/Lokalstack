# Phase 10D — Performance, Long-Run Stability & Accessibility

Supplements `docs/architecture.md` (Phases 1–10B) and the Phase 10C notes.

## Polling cost & hidden-to-tray decision (§K, §L)

Measured steady-state cost of all pollers combined (ports 3 s, conflicts/readiness 2 s,
AI 10 s, Docker 5 s, workspace monitoring 1 s, managed-log polling 500 ms while a log
panel is open): **~126 ms CPU per 2 s window (~6% of one core)**, max sampled window
219 ms — no synchronized spike at the 30 s cadence alignment. Timers start at different
phases (frontend mount vs backend monitor boot), so alignment spikes do not occur in
practice; no jitter was added (jitter would make the Phase 10A fake-timer tests flaky).

**Hidden-to-tray decision (§L): keep all cadences unchanged.** Core monitoring is the
product: conflicts, managed-workspace state, and service/process state must stay
current while the window is hidden — that is the point of close-to-tray. Idle cost
while hidden is ~5.8% of one core (see soak below), dominated by backend discovery,
which the product requires. UI-only re-render savings were not worth the correctness
risk of splitting cadence states. When the window is re-shown, the frontend pollers
resume on their existing timers and the first tick renders fresh state; the Phase 10C
show path also triggers nothing destructive.

## Lock contention (§J)

- AI registry: cache lock is now held only to **copy** cached entries out; HTTP probes
  run unlocked; store-back re-locks briefly at the end. A hung runtime costs its own
  2 s probe timeout, never a lock hold.
- Workspace creation: duplicate fast-path check releases the list lock before spec
  derivation (filesystem reads); the authoritative duplicate check re-runs under the
  lock after derivation (racing creators → the loser is refused).
- Docker: the engine lock remains the deliberate single-flight gate around pipe I/O,
  bounded by `REQUEST_TIMEOUT` per request and bounded parse counts; documented in code.
- All slow I/O elsewhere was already outside broad locks (Phase 10B audit held).

## Settings write-churn (§R) & log bounds (§S)

Settings persist only on explicit save/reset/startup-toggle (store test proves load
never writes). The diagnostic log gained a cross-session size cap: on open, a file
larger than 256 KB is truncated; a full session (~2,000 lines × ~120 B ≈ 240 KB)
always fits under the cap, so total log growth is bounded forever. Redaction is
unchanged and tested.

## Large-list decision (§F, §G, §20)

All large-fixture renders complete far below the broad regression ceiling
(`largeFixtureRender.test.tsx`): 300 ports ~0.7 s first render in jsdom, 500 AI models
~0.3 s, 100 Docker containers ~0.4 s, 100 history events ~0.3 s. jsdom timings
overstate real WebView cost; nothing pathological (no O(n²) per-row work) was found,
so **no virtualization/pagination was added** — derivation is memoized and pages use
narrow Zustand selectors (no whole-store subscriptions exist).

## Accessibility (§U–AK)

Fixes: ports search input has an accessible name; the conflict dialog pulls focus
inside on open and closes on Escape (non-destructive); a global `:focus-visible`
outline guarantees visible keyboard focus; `prefers-reduced-motion: reduce` disables
non-essential animation (loading text states keep meaning). Statuses/badges were
already text-bearing (READY/CONFLICT/external/container labels), destructive reset
never autofocuses the destructive button, tables are semantic HTML in scroll
containers, and settings switches are `role="switch"` with accurate `aria-checked`.
Regression tests live in `accessibility.test.tsx`; the two-step `ConfirmButton`
control confirmation already exposes action + identity as text.

## Release profile & sizes (§AM–AO)

`[profile.release]`: `lto = true`, `codegen-units = 1`, `opt-level = "s"`,
`panic = "abort"`, `strip = true` — suitable for v1.0 (size-optimized desktop binary;
panic-abort is acceptable because Phase 10B removed expected-environment panic paths
and the panic hook still records diagnostics). Frontend bundle and release EXE sizes
are recorded in the Phase 10D report.
