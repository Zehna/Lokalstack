# Phase 11A External QA & Repository Hardening Implementation Plan

> **For agentic workers:** Execute this plan task-by-task with a review
> checkpoint after each independently testable task.

**Goal:** Validate the v1.0.0 artifact under a controlled clean-state
simulation and harden the repository for post-v1.0 development without
changing the v1.0.0 tag.

**Architecture:** Treat QA, repository hygiene, and branch protection as
separate gates. User state is backed up outside the repository before any
reset and restored before Phase 11A can pass. Application source is not
modified unless a separately approved bug-fix task is opened.

**Tech Stack:** Windows 10/11, PowerShell/CMD, Git, GitHub CLI, GitHub Actions,
Tauri v2, Rust, React/TypeScript.

**Spec:** docs/superpowers/specs/2026-09-15-phase-11a-external-qa-repository-hardening-design.md

---

## Global rules (apply to every task)

- The v1.0.0 tag is immutable. Never delete, move, re-point, or re-create it.
  Any command output that shows the tag resolving to something other than
  `6e2c03f6ad8b2099f5cf4ba52995916a200d9acd` is an immediate STOP condition.
- Never push unless the task explicitly says so. Only TASK 12 pushes, and it
  pushes the `phase-11a-qa-hardening` branch — never main, never tags.
- Destructive steps are permitted ONLY against LocalStack-owned state:
  `%LOCALAPPDATA%\localstack-control-center`,
  `%LOCALAPPDATA%\com.localstack.controlcenter`, and the LocalStack-specific
  HKCU Run value. No wildcards, no broad deletion, no registry sweeps.
  Every destructive step has a rollback path via the TASK 2 backup.
- If any command errors while deleting/restoring user state: STOP, do not
  improvise, report the exact command and output.
- The untracked junk file (real name ends with U+F022:
  `tom protocol verification<PUA>`) and `github-release-artifacts/` are
  read-only for the worker until TASK 11.
- Known environment risk from prior sessions: the machine can run under
  severe commit memory pressure, which once caused a PowerShell OOM and a
  stalled NSIS uninstaller self-relaunch. Prefer lightweight shell commands
  for destructive steps; if an uninstaller appears to have not completed,
  verify actual state before retrying and report honestly.
- Every QA scenario is recorded as `PASS`, `FAIL`, or
  `N/A — environment prerequisite unavailable` per the spec.

---

## TASK 1 — Baseline and artifact verification

**Type:** read-only. No mutation of any file, registry value, or process.

- [ ] Verify repository baseline:

  ```bash
  git status --short
  git branch --show-current
  git rev-parse HEAD
  git rev-parse v1.0.0^{commit}
  git log -1 --oneline --decorate
  git remote -v
  ```

  **Expected:** branch `phase-11a-qa-hardening` (created from main at
  `6e2c03f`); HEAD and `v1.0.0^{commit}` both
  `6e2c03f6ad8b2099f5cf4ba52995916a200d9acd`; no tracked modifications;
  untracked items limited to `github-release-artifacts/` and the junk file.
  **Stop condition:** tag mismatch or tracked modifications → STOP, report.

- [ ] Verify GitHub CI state for the baseline commit:

  ```bash
  gh run list --branch main --limit 5
  ```

  **Expected:** the quality-gates run for `6e2c03f` shows `success`.

- [ ] Verify the authoritative artifact set exists and is intact:

  ```bash
  ls -la github-release-artifacts/
  cat github-release-artifacts/SHA256SUMS.txt
  sha256sum "github-release-artifacts/LocalStack Control Center_1.0.0_x64-setup.exe" \
            "github-release-artifacts/LocalStack Control Center_1.0.0_x64_en-US.msi" \
            "github-release-artifacts/localstack-control-center-standalone.exe"
  gh api repos/Zehna/Lokalstack/actions/runs/34872661118 --jq '{head_sha, conclusion, id}'
  ```

  **Expected:** exactly the four spec files present and non-empty; every
  recomputed SHA-256 matches `SHA256SUMS.txt`; run `34872661118` shows
  `head_sha = 6e2c03f6ad8b2099f5cf4ba52995916a200d9acd` and
  `conclusion = success`.

- [ ] Record the baseline commit SHA and artifact hashes in the eventual QA
      report notes (no file written yet).

**Rollback/stop:** nothing to roll back (read-only). Stop on any mismatch.

**Commit checkpoint:** none (no repository files changed).

---

## TASK 2 — Safe user-state backup

**Type:** state observation + backup creation OUTSIDE the repository.

- [ ] Detect and record install/running/autostart baseline:

  ```powershell
  # Install state
  Test-Path "$env:LOCALAPPDATA\LocalStack Control Center\localstack-control-center.exe"
  # Running state
  Get-Process localstack-control-center -ErrorAction SilentlyContinue | Select-Object Id, StartTime
  # Autostart entry (exact value, recorded verbatim)
  Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' |
    Select-Object 'LocalStack Control Center'
  # App-data presence
  Test-Path "$env:LOCALAPPDATA\localstack-control-center"
  Test-Path "$env:LOCALAPPDATA\com.localstack.controlcenter"
  ```

  **Expected:** boolean/string results recorded verbatim in the QA evidence
  notes. The autostart value (if any) is recorded exactly — it contains no
  secrets (an EXE path plus optional `--startup`).

- [ ] Record baseline external dev processes/listeners (the survival witness
      set used again in TASK 8):

  ```powershell
  Get-Process | Where-Object { $_.ProcessName -match 'node|python|ollama|vite' } |
    Select-Object Id, ProcessName, StartTime
  Get-NetTCPConnection -State Listen | Select-Object LocalAddress, LocalPort, OwningProcess
  ```

- [ ] Create a timestamped backup directory OUTSIDE the repository:

  ```powershell
  $ts  = Get-Date -Format 'yyyyMMdd-HHmmss'
  $bak = "$env:USERPROFILE\localstack-qa-backup-$ts"
  New-Item -ItemType Directory -Path $bak | Out-Null
  ```

  **Expected:** `$bak` is like `C:\Users\<user>\localstack-qa-backup-<ts>`
  and is NOT under `D:\Projects\localstack`.

- [ ] Copy the exact LocalStack-owned directories (only if they exist):

  ```powershell
  if (Test-Path "$env:LOCALAPPDATA\localstack-control-center") {
    Copy-Item -Recurse -Force "$env:LOCALAPPDATA\localstack-control-center" "$bak\localstack-control-center"
  }
  if (Test-Path "$env:LOCALAPPDATA\com.localstack.controlcenter") {
    Copy-Item -Recurse -Force "$env:LOCALAPPDATA\com.localstack.controlcenter" "$bak\com.localstack.controlcenter"
  }
  ```

  (Both dirs contain settings and WebView cache only — no secrets; sizes are
  small. Record byte counts after copy.)

- [ ] Export the LocalStack-specific HKCU Run state (only that value):

  ```powershell
  $run = Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
  $ls  = $run.'LocalStack Control Center'
  if ($null -ne $ls) {
    Set-Content -Path "$bak\autostart-run-value.txt" -Value $ls -Encoding UTF8
  } else {
    Set-Content -Path "$bak\autostart-run-value.txt" -Value '<ABSENT AT BASELINE>' -Encoding UTF8
  }
  ```

- [ ] Verify the backup before any reset:

  ```powershell
  Get-ChildItem -Recurse $bak | Measure-Object -Property Length -Sum
  # + spot-check settings.json content equals the live file
  ```

  **Expected:** backup directory non-empty (or explicitly recorded as
  "nothing to back up — clean baseline"), settings file byte-identical to the
  live file, autostart export written. Backup path and SHA-256 of the copied
  `settings.json` recorded.

- [ ] Record rollback instructions in the QA notes: restore = reverse of
      TASK 9 using exactly `$bak`.

**Rollback/stop:** deleting `$bak` is never part of this task. Stop if any
copy errors — do not proceed to TASK 3 with an unverified backup.

**Commit checkpoint:** none (backup lives outside the repository).

---

## TASK 3 — Clean-state preparation

**Type:** destructive, narrowly scoped to LocalStack-owned state.

- [ ] If LocalStack is currently running, exit it through its own tray Exit
      (preferred) or close the window per current settings; verify the
      process is gone:

  ```powershell
  Get-Process localstack-control-center -ErrorAction SilentlyContinue
  ```

  **Expected:** no process. **Rollback:** none needed (app not modified).

- [ ] If a previous install exists, run its normal uninstaller in silent
      mode (NSIS convention), then verify removal:

  ```powershell
  & "$env:LOCALAPPDATA\LocalStack Control Center\uninstall.exe" /S
  # wait, then verify
  Test-Path "$env:LOCALAPPDATA\LocalStack Control Center"
  ```

  **Rollback/stop:** the TASK 2 backup restores everything; if the
  uninstaller does not complete (see memory-pressure risk in Global rules),
  verify actual state before retrying; never force-delete arbitrary files.

- [ ] Reset ONLY the approved LocalStack state (no wildcards beyond the two
      exact named directories):

  ```powershell
  Remove-Item -Recurse -Force "$env:LOCALAPPDATA\localstack-control-center" -ErrorAction SilentlyContinue
  Remove-Item -Recurse -Force "$env:LOCALAPPDATA\com.localstack.controlcenter" -ErrorAction SilentlyContinue
  Remove-ItemProperty -Path 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' `
    -Name 'LocalStack Control Center' -ErrorAction SilentlyContinue
  ```

  **Forbidden:** `Remove-Item *`, registry-tree deletion, deletion of anything
  outside the two named directories and the one named Run value.

- [ ] Verify the clean baseline:

  ```powershell
  Test-Path "$env:LOCALAPPDATA\localstack-control-center"          # False
  Test-Path "$env:LOCALAPPDATA\com.localstack.controlcenter"       # False
  (Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run').'LocalStack Control Center' -eq $null  # True
  ```

  **Expected:** all three checks confirm clean state. Record in QA notes as
  the evidence for matrix item 2 (plus the verified backup from TASK 2).

**Commit checkpoint:** none.

---

## TASK 4 — Clean first-run/install QA

**Type:** uses the authoritative artifact; read-only toward everything else.

- [ ] Matrix 3 — Install from the NSIS artifact (NEVER a dev build or plain
      cargo build; NEVER a Phase 10-era artifact):

  ```powershell
  & "D:\Projects\localstack\github-release-artifacts\LocalStack Control Center_1.0.0_x64-setup.exe" /S
  ```

  **Expected:** per-user install at
  `%LOCALAPPDATA%\LocalStack Control Center\`; EXE present; Start Menu
  shortcut exists; uninstall registration exists under
  `HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall`. Record as
  matrix item 3.

- [ ] Matrix 4 — First launch with NO dev server:

  ```powershell
  # Preconditions: no node/vite dev server on 1420; check first
  Get-NetTCPConnection -LocalPort 1420 -State Listen -ErrorAction SilentlyContinue  # must be empty
  Start-Process "$env:LOCALAPPDATA\LocalStack Control Center\localstack-control-center.exe"
  Start-Sleep -Seconds 4
  Get-Process localstack-control-center | Select-Object Id, MainWindowTitle
  ```

  **Expected:** process alive with a real main window; UI renders
  (verifiable via UIA window tree). Any failure = RELEASE BLOCKER (first-run
  failure / dev-server dependency).

- [ ] Matrix 5/6/7 — Navigate Ports, Services, Projects via UI Automation;
      confirm real data renders with no crash/ErrorBoundary and that
      Projects shows evidence-based ownership only (spot-check: a project is
      attributed only when its directory is actually served by a listener).

- [ ] Matrix 8 — Safe process control boundary spot-check: confirm the UI
      exposes confirmation-gated control for managed processes only and that
      external/unknown processes offer no raw control. Evidence = UIA dump +
      the standing static safety guards (CI green).

- [ ] Collect evidence (screenshots/UIA dumps/console state) into the QA
      evidence notes. Do not write anything into the repository.

**Rollback/stop:** uninstall (TASK 8 procedure) if installation corrupts
anything; restore via TASK 2 backup.

**Commit checkpoint:** none.

---

## TASK 5 — Lifecycle/safety QA

- [ ] Matrix 9 — Tray lifecycle: set close-behavior to tray in Settings
      (UIA), close the window via the X button, verify the process survives
      and the window is hidden; Tray → Open restores the same window; Tray →
      Refresh performs an observation-only refresh; Tray → Exit terminates
      the app cleanly. Verify all baseline external dev processes
      (TASK 2 witness set) are still alive afterward.

- [ ] Matrix 10 — Single instance: launch the installed EXE a second time;
      **expected:** the second process exits and the existing window is
      focused; exactly one effective instance. Repeat once with the window
      hidden in tray (hidden single-instance case).

- [ ] Matrix 11 — Autostart enablement: toggle "Run at Windows startup" in
      Settings; verify:

      ```powershell
      (Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run').'LocalStack Control Center'
      # Expected: "...localstack-control-center.exe" --startup   (installed path)
      ```

- [ ] Matrix 12 — Startup-mode launch: exit the app, relaunch the installed
      EXE with `--startup`, verify it starts per product settings (window
      per close-behavior setting), and that no second instance spawns.

- [ ] Verify external dev processes survived every lifecycle operation.

**Rollback/stop:** any external process death = RELEASE BLOCKER → STOP RULE
of the spec (restore first, evidence, report, no auto-fix).

**Commit checkpoint:** none.

---

## TASK 6 — Integration/environment QA

- [ ] Matrix 13/14 — Docker: record whether Docker is actually installed and
      running. If unavailable → record `N/A — environment prerequisite
      unavailable` for 14 and PASS/observed for 13's graceful state. If
      running → Docker page must show read-only observation only (no
      lifecycle/exec/mutation affordances); `N/A` for 13 only if the
      unavailable state cannot be produced without touching Docker itself.

- [ ] Matrix 15/16 — AI runtimes: record actual state. If none running →
      16 is `N/A`; page must show honest unavailable/unknown states (15).
      Never install a runtime just for QA. If a runtime IS running →
      read-only health/model observation only.

- [ ] Matrix 17 — Controlled listener: create a disposable loopback-only
      listener owned by QA (PowerShell TcpListener — no external dependency,
      self-terminates after 10 min even if QA forgets it):

      ```powershell
      $p = Start-Process powershell -ArgumentList '-NoProfile','-Command',
        '$l=[System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback,47654); $l.Start(); Start-Sleep -Seconds 600' -PassThru
      "PID=$($p.Id) name=$($p.ProcessName) start=$($p.StartTime)"   # record all three
      ```

      Verify Ports shows the 47654 loopback listener with correct
      attribution; then stop ONLY this QA-created process — by the recorded
      PID, after confirming `Get-Process -Id <pid>` still shows the same
      process name and creation time (PID-reuse guard) — and verify it
      disappears from the Ports page. **Never terminate any other
      listener.**

- [ ] Matrix 18 — Conflict intelligence: check whether the repository
      provides a deterministic fixture to exercise conflict display. If yes,
      use it read-only; if no, mark `N/A` with the explanation. Do NOT alter
      user projects to force a conflict.

**Rollback/stop:** QA-created listener is the only QA-created process; it is
stopped by TASK 6's end regardless of scenario outcome.

**Commit checkpoint:** none.

---

## TASK 7 — Short soak

- [ ] With the app running and normal polling active, sample process metrics
      every 30 s for ~10 minutes:

      ```powershell
      1..20 | ForEach-Object {
        $p = Get-Process localstack-control-center -ErrorAction SilentlyContinue
        if (-not $p) { "{0:HH:mm:ss} PROCESS GONE" -f (Get-Date); break }
        "{0:HH:mm:ss} CPU={1} RAM={2} Handles={3} Threads={4}" -f (Get-Date), $p.TotalProcessorTime, $p.WorkingSet64, $p.HandleCount, $p.Threads.Count
        Start-Sleep -Seconds 30
      }
      ```

- [ ] Compare start vs end: no obvious runaway growth (memory band, handle
      count, thread count). This supplements — never replaces — the Phase 10D
      40-minute soak.

- [ ] Record the start/end table in QA evidence. No performance claims
      beyond what these samples show; explicitly note that occasional GC/
      allocator caching is not a leak without monotonic evidence.

**Rollback/stop:** if the process dies or metrics explode → record FAIL,
STOP per severity policy, restore state (TASK 9).

**Commit checkpoint:** none.

---

## TASK 8 — Uninstall and residue verification

- [ ] Exit the app if running. Run the normal uninstaller:

      ```powershell
      & "$env:LOCALAPPDATA\LocalStack Control Center\uninstall.exe" /S
      ```

- [ ] Verify residue removal (all four):

      ```powershell
      Test-Path "$env:LOCALAPPDATA\LocalStack Control Center"                # False (binaries)
      # Start Menu shortcut:
      Test-Path "$env:APPDATA\Microsoft\Windows\Start Menu\Programs\LocalStack Control Center*"  # False
      # Uninstall registration:
      Get-ChildItem 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall' |
        Where-Object { $_.GetValue('DisplayName') -like '*LocalStack*' }      # empty
      # Autostart entry (set during TASK 5):
      (Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run').'LocalStack Control Center' -eq $null  # True
      ```

      **Expected:** all False/empty → matrix item 20 PASS. A stale autostart
      entry = RELEASE BLOCKER.

- [ ] Verify the TASK 2 external dev-process witness set is still alive
      (uninstall must not touch unrelated processes).

- [ ] Note honestly whether user data dirs were retained (documented Phase
      10E behavior: retained) — they are restored/normalized in TASK 9.

**Rollback/stop:** if the uninstaller stalls (known memory-pressure risk),
verify actual state before retrying; report retry counts honestly.

**Commit checkpoint:** none.

---

## TASK 9 — Restore user state

**Type:** destructive-to-QA-state, restorative for the user. **Must run even
if prior QA tasks FAILED** (with evidence preserved first).

- [ ] Restore settings/app-data from the TASK 2 backup:

      ```powershell
      Copy-Item -Recurse -Force "$bak\localstack-control-center" "$env:LOCALAPPDATA\localstack-control-center"  # if it existed at baseline
      Copy-Item -Recurse -Force "$bak\com.localstack.controlcenter" "$env:LOCALAPPDATA\com.localstack.controlcenter"  # if it existed at baseline
      ```

- [ ] Restore the autostart value ONLY if the baseline export contained one:

      ```powershell
      $v = Get-Content "$bak\autostart-run-value.txt" -Raw
      if ($v -ne "<ABSENT AT BASELINE>") {
        New-ItemProperty -Path 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' `
          -Name 'LocalStack Control Center' -Value $v.Trim() -PropertyType String -Force | Out-Null
      }
      ```

      (Do not restore stale QA-generated state over the user's backup.)

- [ ] Restore the baseline install state: if LocalStack was installed at
      baseline (recorded in TASK 2), reinstall from the authoritative NSIS
      artifact; if it was not installed, leave it uninstalled.

- [ ] Verify restored state against the TASK 2 record item by item
      (install present/absent, settings file byte-identical to backup,
      autostart value identical or absent, no app running unless baseline
      had it running). Record as matrix item 21.

**Rollback/stop:** restore verification failure = RELEASE BLOCKER (restore
procedure loses user state) → report; the untouched `$bak` directory remains
as the safety net.

**Commit checkpoint:** none.

---

## TASK 10 — QA report

- [ ] Create `docs/qa/external-qa-v1.0.md` recording everything the spec
      requires: Windows environment; baseline commit/tag; workflow run ID
      34872661118; artifact SHA-256 set; the full 22-item matrix with
      PASS/FAIL/N/A and evidence per item; backup location (outside repo);
      restore result; findings classified per severity policy; final Phase
      11A QA verdict.

- [ ] Redact anything secret-shaped from the report (none is expected — the
      app stores no secrets; a scan of report content is still performed).

- [ ] Review the report against the spec's "Documentation output" list —
      every bullet present, no placeholders (no TBD/TODO markers).

**Commit checkpoint:** commit the QA report (single commit).

---

## TASK 11 — Repository hygiene

**Type:** repository hygiene; separate commit from TASK 10.

- [ ] Add `github-release-artifacts/` to `.gitignore` (keep the existing
      entries intact):

      ```bash
      # then verify
      git check-ignore github-release-artifacts/ && echo IGNORED
      git status --short   # the dir must no longer appear as untracked
      ```

- [ ] Inspect the junk file before deleting (read its content; check size,
      timestamps, and whether git ever tracked it via
      `git log --all --oneline -- 'tom protocol verification*'`):

      ```bash
      cat 'tom protocol verification'*
      git log --all --oneline -- 'tom protocol verification*'
      ```

- [ ] Delete it ONLY if clearly confirmed accidental junk (expected: empty
      or meaningless scratch content, never tracked). Delete by the exact
      name including the U+F022 trailing character (glob
      `tom protocol verification*` from bash is sufficient and safe). Record
      the file's content summary in the QA report. If content is meaningful
      → leave untouched and record that decision instead.

- [ ] Verify no release binaries are tracked:

      ```bash
      git ls-files | grep -iE '\.(exe|msi)$' || echo "no binaries tracked"
      ```

      **Expected:** "no binaries tracked". (Note: the `*.exe`/`*.msi`
      gitignore entries already exist from Phase 10E — verify they are still
      effective, not removed.)

- [ ] Non-destructive heuristic secret scan of tracked content (and of the
      current history's blob names — no history rewrite):

      ```bash
      git grep -inIE 'api[_-]?key|secret|password|token\s*=|BEGIN (RSA|EC|OPENSSH) PRIVATE KEY' -- . || echo "no hits"
      # names-only historical pass (no content rewrite):
      git log --all --diff-filter=A --name-only --pretty=format: | sort -u | \
        grep -iE 'secret|credential|\.pem$|\.key$' || echo "no suspicious names"
      ```

      Review hits; expect the existing words only in docs/comments/security
      context (e.g. safety tests). Redact any potential secret VALUE from
      reports/output; never paste candidate secrets into the QA report.

- [ ] Confirm `.gitignore` change + junk-file removal in one hygiene commit
      (does NOT include the QA report — that was TASK 10's commit).

**Rollback/stop:** junk-file deletion is unrecoverable-by-design — hence it
happens only after inspection; if any doubt exists, leave the file and
record the decision.

**Commit checkpoint:** commit `.gitignore` (+ deletion) as the hygiene commit.

---

## TASK 12 — Branch hardening

**Type:** remote/GitHub state change. Executes only after TASKS 1–11 are
complete and green. Never touches v1.0.0.

- [ ] Push the phase branch (NOT main, NOT tags):

      ```bash
      git push -u origin phase-11a-qa-hardening
      git ls-remote --heads origin phase-11a-qa-hardening   # verify
      ```

- [ ] Open a PR into main:

      ```bash
      gh pr create --base main --head phase-11a-qa-hardening \
        --title "Phase 11A: external QA evidence + repository hardening" \
        --body "QA report, gitignore/artifact hygiene, junk-file removal per spec. No application source changes. v1.0.0 untouched."
      ```

- [ ] Wait for CI on the PR and require it to pass:

      ```bash
      gh pr checks phase-11a-qa-hardening   # all required checks success
      ```

      **Stop condition:** CI failure → diagnose; do NOT merge with failing
      checks; report.

- [ ] Merge the PR (merge commit is fine; no second approval is required by
      design). After merge, verify main:

      ```bash
      git fetch origin
      git log -1 --oneline origin/main
      gh run list --branch main --limit 1   # CI green on merged main
      ```

- [ ] Configure main protection (balanced solo-developer policy):

      ```bash
      gh api repos/Zehna/Lokalstack/branches/main/protection -X PUT \
        --input - <<'JSON'
      {
        "required_status_checks": {
          "strict": true,
          "contexts": ["Quality gates (windows-latest)"]
        },
        "enforce_admins": false,
        "required_pull_request_reviews": {
          "required_approving_review_count": 0
        },
        "restrictions": null,
        "allow_force_pushes": false,
        "allow_deletions": false,
        "required_linear_history": false
      }
      JSON
      ```

      Notes: `strict: true` keeps the branch up to date before merge;
      `required_approving_review_count: 0` = zero mandatory second-person
      approval; `enforce_admins: false` preserves a solo-developer escape
      hatch; the status-check context name must exactly match
      ci.yml's job name (`Quality gates (windows-latest)`).

      **Plan/visibility fallback:** if the repository's visibility or GitHub
      plan rejects part of this payload (e.g. branch protection unavailable
      for private repos on free plans), apply every field the API accepts,
      record exactly which protections could NOT be enabled in the QA
      report, and never fabricate success. `required_status_checks` and
      force-push/deletion protection are the priority fields.

- [ ] Verify the applied protection:

      ```bash
      gh api repos/Zehna/Lokalstack/branches/main/protection
      # assert: force-push disabled, deletion disabled, PR reviews required
      # with 0 approvals, required context = the CI job
      ```

- [ ] Confirm v1.0.0 unchanged: `git rev-parse v1.0.0^{commit}` still
      `6e2c03f6ad8b2099f5cf4ba52995916a200d9acd`; `gh release list` shows no
      releases; no tag push ever issued.

**Rollback/stop:** protection is adjustable via the same API (`gh api -X DELETE
repos/Zehna/Lokalstack/branches/main/protection`) if it blocks a legitimate
flow; the branch/PR can
be re-run; v1.0.0 has no rollback need because nothing touches it.

**Commit checkpoint:** none on the branch (PR merge itself integrates
TASKS 10–11 commits into main).

---

## TASK 13 — Final Phase 11A gate

- [ ] Verify final main commit contains the QA report + hygiene changes:

      ```bash
      git fetch origin
      git log -3 --oneline origin/main
      git show --stat origin/main | head -30
      ```

- [ ] Verify CI green on merged main: `gh run list --branch main --limit 1`
      → success.

- [ ] Verify tag immutability one final time:

      ```bash
      git rev-parse v1.0.0^{commit}   # 6e2c03f6ad8b2099f5cf4ba52995916a200d9acd
      git tag --list                   # exactly v1.0.0
      ```

- [ ] Verify clean working tree (no stray QA files, no backup inside the
      repo, no tracked modifications; only intentionally untracked noise if
      any decision says so).

- [ ] Produce the final PASS/BLOCKED verdict per the spec's Phase 11A
      success criteria, appended to `docs/qa/external-qa-v1.0.md` if the
      verdict is PASS (a BLOCKED verdict is reported, not silently fixed).

**Commit checkpoint:** final verdict annotation commit if needed.

---

## Review checkpoint discipline

After each task: review evidence, confirm the task's expected results, and
only then proceed. A failed expectation never rolls forward silently — it
either stops (blocker) or is recorded (major/minor) per the severity policy.
