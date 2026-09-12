# Release Checklist — v1.0.0 Release Candidate

Status at end of Phase 10E. No tag has been created; no public release exists.

## Source & version

- [x] Clean Git baseline at Phase 10D commit `76e94a8`
- [x] Version 1.0.0 synchronized (package.json, package-lock.json,
      src-tauri/Cargo.toml, Cargo.lock, tauri.conf.json) — pinned by
      `src/releaseVersion.test.ts`
- [x] Product identity consistent (`LocalStack Control Center`,
      `com.localstack.controlcenter`, `localstack-control-center.exe`)

## Quality gates

- [x] `npm ci` (reproducible install)
- [x] `npm run typecheck` — 0 errors
- [x] `npm run test:run` — 212 frontend tests
- [x] `npm run test:coverage` — recorded in Phase 10E report
- [x] `npm run build` — frontend production build
- [x] `cargo check --locked` — 0 warnings
- [x] `cargo test --locked` — 380 deterministic tests
- [x] `cargo test --locked -- --ignored` — 11 live/deterministic tests
- [x] Static safety guards (frontend + Rust) — green
- [x] `npm audit` — 0 vulnerabilities
- [ ] `cargo audit` — not installed on this machine (documented, see report)

## Packaging

- [x] Official build command: `npx tauri build` (NOT `cargo build --release`)
- [x] Custom-protocol embedded assets; binary contains no `localhost:1420`
- [x] Release EXE verified standalone with port 1420 closed, no Vite
- [x] NSIS artifact built (size + SHA-256 recorded in release-checksums.txt)
- [x] MSI artifact built
- [x] NSIS fresh install (per-user, no admin, Start Menu entry)
- [x] Installed app: standalone launch, all views render, settings under
      app-data
- [x] Installed app: empty/unavailable states (no Docker/AI/workspaces) safe
- [x] Installed tray: close-to-tray hide, process alive
- [x] Installed single-instance: second launch exits, first window restored
- [x] Installed autostart: registration points at installed path
      (`C:\Users\<user>\AppData\Local\LocalStack Control Center\...`), disable
      removes it; no `D:\Projects\localstack` path remains
- [x] NSIS uninstall removes binaries + Start Menu; dev services untouched;
      AppData settings/log retained (documented)
- [x] MSI install (`/passive`, elevation) + launch + uninstall verified;
      silent `/qn` fails without UI elevation (documented)
- [x] release-checksums.txt created (EXE + NSIS + MSI SHA-256)

## Documentation

- [x] CHANGELOG.md — 1.0.0 RC section
- [x] README.md — release pass
- [x] docs/install-windows.md — install/uninstall/signing/settings
- [x] CI: .github/workflows/ci.yml (deterministic gates + safety guards)
- [x] Release workflow: .github/workflows/release.yml (workflow_dispatch,
      artifacts only)

## Release policy

- [x] Unsigned status documented (README, install doc)
- [x] Auto-updater disabled/deferred (no unsigned remote update)
- [x] No build artifacts committed (target/, dist/, coverage/, installers
      ignored; release-checksums.txt tracked)
- [ ] **Git tag `v1.0.0`** — intentionally NOT created (Phase 10F decides)
- [ ] **Public release / artifact upload** — intentionally NOT done
