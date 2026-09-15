# Phase 11A — External QA & Repository Hardening

Status: APPROVED DESIGN — PLANNING ONLY (this document was written before any Phase 11A QA execution)

## GOAL

Increase confidence in LocalStack Control Center after the v1.0.0 release
candidate by simulating a clean first-run environment on the current Windows
account, documenting evidence, cleaning repository noise, and preparing main
for safer post-v1.0 development.

## NON-GOALS

- no new product features
- no theme implementation
- no updater
- no code signing
- no macOS/Linux
- no Docker lifecycle controls
- no AI mutation/inference
- no architectural product refactors
- no modification of v1.0.0 tag
- no public GitHub Release

## ENVIRONMENT CONSTRAINT

Only the current Windows machine/account is available.
No second PC, VM, Windows Sandbox, or temporary Windows user will be used.
Consequently every QA claim is scoped to: "one Windows 10/11 x64 account with
this machine's existing dev environment". Clean-state behavior is simulated by
resetting LocalStack-owned state only — never by altering other users' data.

## APPROVED CLEAN-STATE STRATEGY

Use a controlled same-account clean-room simulation.

Before QA:

1. record whether LocalStack is installed
2. record whether LocalStack is currently running
3. record the exact existing autostart Run entry if present
4. back up:
   - `%LOCALAPPDATA%\localstack-control-center`
   - `%LOCALAPPDATA%\com.localstack.controlcenter`
   - LocalStack's specific HKCU Run value (value name
     `LocalStack Control Center` under
     `HKCU:\Software\Microsoft\Windows\CurrentVersion\Run`)
5. store backup OUTSIDE the repository
6. record baseline running dev processes/listeners needed to ensure LocalStack
   does not disturb external processes

The backup must be restorable.

QA may reset ONLY LocalStack-owned state.

Never use broad wildcard deletion.

## APPROVED RESTORE POLICY

At the end of QA, restore the user's pre-QA LocalStack state automatically.

Restore:

- application installed/uninstalled state to match baseline
- settings directory
- WebView/app-data directory where practical
- LocalStack-specific startup registration if it existed at baseline

Do not restore stale QA-generated state over the user's backup.

## AUTHORITATIVE RELEASE ARTIFACT

Use the artifact set produced by GitHub Actions Release Build run:
**34872661118**

Artifact source commit:
**6e2c03f6ad8b2099f5cf4ba52995916a200d9acd** (identical to local HEAD, remote
main, and tag v1.0.0)

Expected locally downloaded artifact directory:
`github-release-artifacts/`

Expected binaries:

- `LocalStack Control Center_1.0.0_x64-setup.exe` (NSIS installer)
- `LocalStack Control Center_1.0.0_x64_en-US.msi` (MSI installer)
- `localstack-control-center-standalone.exe` (portable EXE)
- `SHA256SUMS.txt`

Never substitute:

- a dev build
- plain `cargo build --release`
- stale Phase 10 artifacts

All artifact usage is read-only inspection; the directory must never be
committed, moved, or deleted by Phase 11A.

## QA STATUS VALUES

Each scenario must be recorded as exactly one of:

- `PASS`
- `FAIL`
- `N/A — environment prerequisite unavailable`

`N/A` is acceptable only when the prerequisite genuinely does not exist, for
example Docker not installed. `N/A` must never be used to skip a scenario whose
prerequisite is present.

## QA MATRIX

1. **Artifact integrity**
   PASS: hashes match SHA256SUMS and files are non-empty.

2. **Clean-state preparation**
   PASS: backup exists and only LocalStack-owned state is reset.

3. **NSIS installation**
   PASS: installation succeeds, expected EXE/shortcut/uninstall registration exist.

4. **First launch without dev server**
   PASS: application launches with no Vite/dev server and no localhost:1420 dependency.

5. **Ports**
   PASS: real listeners render without crash/ErrorBoundary.

6. **Services**
   PASS: real service data renders without crash and unknown states remain honest.

7. **Projects**
   PASS: project ownership is evidence-based and no unsafe false ownership appears.

8. **Safe process control**
   PASS: external and managed process boundaries remain enforced.
   Raw-PID control must not appear.

9. **Tray lifecycle**
   PASS: close-to-tray, Open, Refresh, Exit work and external dev processes survive.

10. **Single instance**
    PASS: second launch restores/focuses the existing instance instead of creating
    two independent app instances.

11. **Autostart enablement**
    PASS: HKCU Run registration points to the installed EXE with `--startup`.

12. **Startup-mode launch**
    PASS: `--startup` behavior follows product settings without spawning duplicate
    instances.

13. **Docker unavailable**
    PASS: graceful unavailable state and no crash.
    N/A if Docker is available and this case cannot safely be produced.

14. **Docker available**
    PASS: read-only observation only; no lifecycle/exec/mutation.
    N/A if Docker is not installed/running.

15. **AI runtimes unavailable**
    PASS: graceful unavailable/unknown states.
    Do not install runtimes just for QA.

16. **AI runtime available**
    PASS: read-only health/model observation.
    No inference/model mutation.
    N/A if none is running.

17. **Controlled listener/port ownership**
    Prefer a disposable loopback-only test process created by QA.
    Record its PID/creation time.
    Stop only the QA-created process.
    Never terminate an unrelated listener.

18. **Conflict intelligence**
    Use an existing safe fixture/workspace only if the repository already
    provides a deterministic way to exercise it.
    Do not alter user projects merely to force a conflict.
    Otherwise mark N/A with explanation.

19. **Short soak**
    Run approximately 10 minutes with normal polling.
    Observe CPU/RAM/handles for obvious runaway growth.
    This supplements, not replaces, prior Phase 10D soak testing.

20. **Uninstall**
    PASS: binaries, shortcut, uninstall registration, and LocalStack autostart entry
    are removed while external development processes survive.

21. **Restore**
    PASS: pre-QA LocalStack state is restored successfully.

22. **Git cleanliness**
    PASS: QA itself produces no unintended tracked source changes.

## SEVERITY POLICY

### RELEASE BLOCKER

- crash in core page
- first-run failure
- installer unusable
- release binary requires dev server
- unsafe raw process control
- unrelated external process killed
- stale autostart after successful uninstall
- restore procedure loses existing user state

### MAJOR

- important core feature unusable but safety boundary intact
- severe project attribution errors
- Docker/AI page unusable in otherwise supported environment
- broken tray/single-instance behavior without data/safety risk

### MINOR

- copy/text issues
- visual polish
- minor UX inconsistency
- harmless cache/state behavior

## STOP RULE

If a RELEASE BLOCKER is discovered:

- stop destructive/continuing QA
- restore user state first
- collect evidence
- report the blocker
- do NOT fix source automatically

Major/minor findings may be recorded while remaining QA continues.

## REPOSITORY HYGIENE SCOPE

After QA passes sufficiently:

- add `github-release-artifacts/` to `.gitignore`
- inspect `tom protocol verification` (actual on-disk name ends with a
  private-use Unicode character, U+F022 — deletion must reference the real
  bytes, e.g. via a glob such as `tom protocol verification*`, and must be
  confirmed as accidental junk before removal)
- delete it only if clearly confirmed to be accidental junk
- verify no `.exe` or `.msi` release binaries are tracked
- perform a non-destructive heuristic secret scan of tracked content/history
- redact any potential secret values from reports/output
- create `docs/qa/external-qa-v1.0.md`
- keep v1.0.0 untouched
- do not rewrite history

## BRANCH HARDENING TARGET

Use a balanced solo-developer model.

Desired end state:

- development occurs on topic branches
- main integrates through PRs
- GitHub CI must pass before merge
- force pushes to main disabled
- branch deletion disabled
- no mandatory second-person approval
- do NOT configure protection until Phase 11A changes themselves are ready
  to integrate
- verify protection configuration after applying it

Because requiring status checks can interfere with direct-push workflows,
the design explicitly adopts **PR-based integration with zero required human
approvals** rather than pretending required checks and unrestricted direct
pushes are simultaneously guaranteed. In practice: all post-v1.0 changes land
via PR into main; the required status check is the existing CI quality gate;
no human review is mandatory for a solo developer; force-push and deletion
protections guard main's history.

## DOCUMENTATION OUTPUT

Create:
`docs/qa/external-qa-v1.0.md`

It must eventually record:

- Windows environment
- baseline commit/tag
- workflow run ID
- artifact hashes
- QA matrix with PASS/FAIL/N/A
- evidence for failures
- backup location
- restore result
- repository hygiene result
- branch hardening result
- final Phase 11A verdict

## PHASE 11A SUCCESS CRITERIA

- no unresolved release blocker
- clean-state simulation completed
- original user state restored
- QA evidence documented
- repository artifact noise ignored
- accidental junk handled safely
- no release binary tracked
- heuristic secret scan reviewed
- main protected using the approved balanced policy
- CI still green after Phase 11A integration
- v1.0.0 unchanged
