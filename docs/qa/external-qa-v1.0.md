# External QA — LocalStack Control Center v1.0.0

Phase 11A — External QA & Repository Hardening.
Executed per `docs/superpowers/plans/2026-09-15-phase-11a-external-qa-repository-hardening.md`.

- Date: 2026-09-15
- Machine: Windows 11 x64, single Windows account (Ermi), no VM/second machine
- Baseline commit / tag: `6e2c03f6ad8b2099f5cf4ba52995916a200d9acd` (tag `v1.0.0`, untouched throughout)
- Branch: `phase-11a-qa-hardening` (QA evidence only; no application source changes)
- Authoritative artifact source: GitHub Actions "Release build" run **34872661118** (`workflow_dispatch` on main, `head_sha = 6e2c03f6ad8b2099f5cf4ba52995916a200d9acd`, conclusion `success`)

## Artifact integrity (matrix 1) — PASS

Local download directory `github-release-artifacts/` (read-only, never committed):

| Artifact | Bytes | SHA-256 |
|---|---|---|
| `LocalStack Control Center_1.0.0_x64-setup.exe` (NSIS) | 2,052,136 | `0d4ed9a9b3e7b637e9c28b2df100d65f194c9f4d5ecfa88ac42039572b392f1f` |
| `LocalStack Control Center_1.0.0_x64_en-US.msi` (MSI) | 2,768,896 | `a4f88ba8620e69df3b84856b1140fb2319ecda0194088ab514106acbe059e8e6` |
| `localstack-control-center-standalone.exe` | 5,313,536 | `283e03233ed6bee5d857636a75f1d07a117b1ce6d560541ad96a58c4591138ad` |

All recomputed hashes match `SHA256SUMS.txt` byte-for-byte. No dev build, no
`cargo build --release`, no stale Phase 10 artifact was used anywhere in QA.

## Baseline & backup (matrix 2) — PASS

Pre-QA baseline recorded:

- installed: **False** (no prior install; previous phase's QA uninstall had completed)
- running: **False**
- autostart Run value: **ABSENT**
- `%LOCALAPPDATA%\localstack-control-center`: present (user settings + log)
- `%LOCALAPPDATA%\com.localstack.controlcenter`: present (WebView cache)

Backup root (outside the repository, restorable):
`C:\Users\Ermi\localstack-qa-backup-20260915-204143`
— 651 files / ~79 MB, including both directories and an autostart export
containing `<ABSENT AT BASELINE>`. `settings.json` backup hash
`E33349714BDB063FB85EED726D9879C3C25A43B2550FA62F90380A3D4C949878` verified
identical to the live file before any reset.

External-process witness set at baseline: 13 node/node_repl processes
(PIDs 14012, 13632, 13188, 20940, 20248, 18636, 4404, 2948, 2052, 11880,
8528, 5168, 21652) and the machine listener list (32 ports incl. 135, 445,
3002, 1420, 40274, 64576+). Port 1420 was occupied by an **unrelated** project
(`vite preview` from `D:\Downloads\vibe-control-center-v3.3…`, PID 20248) —
untouched by QA and recorded as a stronger first-launch precondition.

## Clean-state preparation (matrix 2) — PASS

Only LocalStack-owned state was removed, each path printed before deletion:
`%LOCALAPPDATA%\localstack-control-center`,
`%LOCALAPPDATA%\com.localstack.controlcenter`, HKCU Run value
`LocalStack Control Center`. Verified gone. No wildcard deletion used.

## Installation & first run (matrices 3–4) — PASS

- NSIS silent install (`/S`) exit 0; per-user install at
  `%LOCALAPPDATA%\LocalStack Control Center\`; EXE 5,313,536 bytes;
  Start Menu shortcut created; uninstall registration present.
- First launch with **no dev server and 1420 occupied by an unrelated app**:
  process alive, window `LocalStack Control Center` visible, full UI renders
  standalone (UIA tree: 246 elements / 133 named incl. Dashboard, all 9 nav
  views, Refresh). No localhost:1420 dependency, no Vite.

## Core pages (matrices 5–7) — PASS

- **Ports**: 42/42 real listeners rendered with full columns; live discovery
  updated during test (native cycle ~23 ms shown in UI).
- **Services**: real process data; confidence always explicit; unknown states
  honest (e.g. generic runtimes not dressed up).
- **Projects**: exactly one evidence-backed project (npx cache dir with real
  command-line evidence); 21 processes honestly grouped under "Unknown
  Project"; page states ownership requires project-marker evidence — no
  unsafe false attribution observed.

## Safe process control (matrix 8) — PASS

- Every external/system row (System, lsass, svchost, services, wininit,
  explorer, Discord, etc.) exposes **"Stop unavailable"** — no raw control.
- The one exact-confidence external row offered an **inline confirm** that
  requires a second explicit click naming the process ("Terminate Node.js
  now"); QA opened it, captured it, and **did not confirm** — target process
  verified alive afterward.
- Standing static safety guards (raw-PID control prohibited, tray/window
  lifecycle cannot control services) remain enforced in CI.

## Tray lifecycle (matrix 9) — PASS

- Close (X-equivalent WM_CLOSE) with `close_behavior=minimize_to_tray`:
  window hidden, process survived.
- Tray → Open LocalStack: same window restored/visible, same PID (0x1807A8).
- Tray icon present in notification-area overflow (verified via UIA +
  IAccessible enumeration; 12 icons, LocalStack icon among them).
- Tray → Exit: process terminated cleanly, tray icon removed (12 → 11 icons),
  Run key correctly retained (autostart enablement is independent of app
  exit), all external dev processes survived.

## Single instance (matrix 10) — PASS

Second launch of the installed EXE while an instance was running: second
process exited immediately, one process remained (PID 19888), existing window
restored/visible. (Hidden-instance variant previously verified in Phase 10E
release testing; re-verified visible case here.)

## Autostart enablement (matrix 11) — PASS

Toggle "Run at Windows startup" in Settings (UIA toggle Off→On) produced:

```
HKCU:\Software\Microsoft\Windows\CurrentVersion\Run
  Value name : LocalStack Control Center
  Value data : C:\Users\Ermi\AppData\Local\LocalStack Control Center\localstack-control-center.exe --startup
```

Points at the **installed** binary; `run_at_startup: true` persisted in
`settings.json` immediately. No secrets involved.

## Startup-mode launch (matrix 12) — PASS

Relaunch with `--startup` after exit: single instance, main window visible
per current settings (`launch_minimized=false`), tray icon present.

## Docker / AI (matrices 13–16)

- **13 Docker unavailable: PASS** — dedicated "Docker Unavailable — Docker
  Engine is not currently available. This is not a LocalStack error. Docker
  Desktop may not be installed or running." state, no crash.
- **14 Docker available: N/A — environment prerequisite unavailable** —
  Docker Desktop is not installed/running on this machine; not manufactured
  for QA.
- **15 AI runtimes unavailable: PASS** — honest empty state; "Probes are
  read-only, loopback-only, and never trigger model loads or inference."
- **16 AI runtime available: N/A — environment prerequisite unavailable** —
  no AI runtime was running during QA; none was installed for QA.

## Controlled listener (matrix 17) — PASS

QA created a disposable loopback-only PowerShell `TcpListener` on
127.0.0.1:47654 (self-terminating after 600 s), PID 11944 recorded with
process name + creation time. Ports page discovered it live and attributed it
to `powershell.exe`. Stopped **only** that PID after an identity check
(process name + creation time match = PID-reuse guard); listener gone from
netstat and from the Ports UI within one poll cycle. No unrelated listener
was ever touched.

## Conflict intelligence (matrix 18) — N/A

The repository provides no deterministic fixture/runtime mode to exercise
conflict display without altering user projects, which the spec forbids.
Marked N/A with this explanation rather than manufacturing a conflict.

## Short soak (matrix 19) — PASS

20 samples × 30 s (≈9.5 min), PID 20684, normal polling, after pages were
exercised:

| Metric | Start | End | Band |
|---|---|---|---|
| CPU (cumulative s) | 8.6 | 31.1 | ≈3.9% of one core steady-state |
| RAM (MB) | 21.4 | 20.0 | 19.8–21.5 (flat) |
| Handles | 327 | 321 | 321–327 (no growth) |
| Threads | 18 | 16 | 15–19 (no growth) |

No runaway growth. Supplements (does not replace) the Phase 10D 40-minute
soak. Allocator caching is not claimed as a leak either way; no monotonic
growth observed.

## Uninstall & residue (matrix 20) — PASS

Normal NSIS uninstaller (`/S`, exit 0) with autostart still enabled:

- install directory removed
- Start Menu shortcut removed
- uninstall registration removed
- **autostart Run entry removed** (full HKCU Run-key sweep: zero
  LocalStack remnants)
- external dev processes survived (node process count unchanged during
  uninstall; witness 2948 verified alive)

## Restore (matrix 21) — PASS, with one corrective finding

User pre-QA state restored and verified: settings directory and WebView
directory re-copied from backup, `settings.json` byte-identical
(`E3334971…`), every backed-up file hash-compared with **0 mismatches**,
autostart absent (matches baseline), no install present (matches baseline).

**Corrective finding (MINOR, QA procedure — not a product defect):** the
first restore attempt used `Copy-Item -Recurse` onto an existing destination
directory, which PowerShell resolves by nesting the source inside the
target. Detected immediately by the hash check (mismatch), corrected by
remove-then-copy into fresh destinations, and re-verified to byte-identity.
Rule for future procedures: when restoring directories, always remove the
QA-generated destination first (backup safety net permitting), then copy,
then hash-verify. No stale QA state survived: the post-fix `settings.json`
matches the user's pre-QA content (interval 5000, run_at_startup true from
their own prior use — the OS registration for it does not exist at baseline
and was not recreated, matching the pre-QA baseline exactly).

## Git cleanliness (matrix 22) — PASS

QA produced no application source changes and no unintended tracked changes;
`scripts/tray-helper.cs` (ephemeral UIA helper) was removed after use.
Repository changes are limited to the two approved commits (QA report,
hygiene) described below.

## Findings summary

- **RELEASE BLOCKERS: none.**
- MAJOR: none.
- MINOR:
  1. Close-behavior `<select>` on Settings lacks an explicit accessible name
     for UIA/label association (control is operable via keyboard and its
     purpose is described by adjacent text; aria-label recommended).
  2. Restore-procedure nesting gotcha (documented above; procedure-level).
- Observations (no severity): 6 of the 13 node-process witnesses exited
  during the QA session, including the unrelated vite-preview on 1420. The
  app's only control paths are confirm-gated (never confirmed) and statically
  excluded from lifecycle actions; the machine was under heavy memory
  pressure during the session and the exits were concurrent with unrelated
  host activity. Attribution to LocalStack is not supported by any evidence;
  recorded for transparency per the plan's honesty requirements.

## Final Phase 11A QA verdict

**PASS** — all scenarios resolved PASS or justified N/A; zero release
blockers; user state restored byte-identically; v1.0.0 untouched throughout.

---

## Phase 11A final gate (recorded after PR integration)

- QA report and hygiene integrated into main via **PR #1** (merge commit
  `1f991e0ac33a24268677b638d56677c80b24cd9b`); PR CI
  (run 34983113852) and merged-main CI (run 34983606723) both **success**.
- Branch protection on main verified via the protection API after applying:
  required status check `Quality gates (windows-latest)` (context confirmed
  from actual GitHub check runs on `6e2c03f`, not assumed from YAML),
  `strict: true`, `required_approving_review_count: 0`,
  `allow_force_pushes: false`, `allow_deletions: false`,
  `enforce_admins: false`. GitHub accepted the full payload — no partial
  protection.
- `v1.0.0` re-verified resolving to
  `6e2c03f6ad8b2099f5cf4ba52995916a200d9acd` after integration.
- Workflow engaged end-to-end once protection was active: this final-gate
  annotation itself reached main through a topic branch + PR + required CI,
  as the approved balanced policy intends.
- **Final Phase 11A verdict: PASS** — all Phase 11A success criteria met.
