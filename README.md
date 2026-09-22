# LocalStack Control Center

A Windows-first desktop control center for your local development environment:
understand everything currently running on **localhost** — services, ports,
processes, projects, workspaces, Docker containers, and local AI runtimes —
and manage your own dev services safely.

## What it does

- **See everything listening** — live TCP discovery with process, service and
  project attribution, refreshed automatically.
- **Understand ownership** — which project, workspace, or container owns each
  port; managed vs external lifecycle shown honestly.
- **Control your own services safely** — start/stop/restart managed workspace
  services with two-step confirmation, opaque targets, and PID-reuse
  protection. External processes are never touched silently.
- **Explain blocked launches** — port conflicts, missing dependencies, and
  readiness issues with structured root causes and advisory free ports.
- **Observe local AI runtimes** — read-only health, versions, and model
  inventory for Ollama, llama.cpp, ComfyUI, Gradio, Open WebUI. Loopback
  only; never runs inference, never downloads/deletes/unloads models.
- **Observe Docker** — read-only containers, published ports, compose
  services, and project association. No container lifecycle, no exec.
- **Stay out of the way** — tray icon, close-to-tray, launch-minimized,
  opt-in Windows startup, single instance.
- **Diagnose problems safely** — deep health checks, incident history, and
  DPAPI-encrypted local support bundles with three privacy profiles for
  sharing; local-only, redacted, no automatic upload.

## Windows requirements

- Windows 10 or 11, x64.
- WebView2 Runtime (preinstalled on current Windows 10/11; the installer
  uses the system WebView2).
- No administrator rights required for the per-user NSIS install.

## Installation

1. Download `LocalStack Control Center_1.0.1_x64-setup.exe` (NSIS) or the
   `.msi` from the release artifacts.
2. Run the installer. **The binary is currently unsigned** — SmartScreen may
   show "Windows protected your PC"; choose *More info → Run anyway* if you
   trust the source. Public distribution should use code signing (see below).
3. Launch from the Start Menu ("LocalStack Control Center") — no source
   tree, Node.js, or dev server is required at runtime.

Uninstall via Windows *Settings → Apps* or the NSIS uninstaller; user
settings/logs under `%LOCALAPPDATA%\localstack-control-center` are retained
(documented behavior).

## Development

Prerequisites: Node.js 20+, npm 10+, Rust stable (MSVC toolchain), and the
[Tauri v2 prerequisites](https://tauri.app/start/prerequisites/).

```bash
npm ci                # reproducible install
npm run tauri dev     # desktop app in development (Vite on :1420)
npm run tauri build   # production installers — the ONLY supported packaging path
npm run test:run      # frontend tests
cd src-tauri && cargo test   # Rust tests
```

> Packaging note: `npx tauri build` compiles with the `custom-protocol`
> feature and embeds the frontend assets. A plain `cargo build --release`
> does **not** — it produces a binary that expects the dev server on port
> 1420. Always package through the Tauri CLI.

## Safety model

- Process control uses opaque, expiring targets — never raw PIDs from the UI.
- Workspace lifecycle fails closed when trusted state cannot be validated.
- AI and Docker integrations are strictly read-only with allowlisted
  requests; there is no arbitrary URL, shell, or Docker API surface.
- Closing, hiding, or uninstalling LocalStack never stops your dev services,
  containers, or AI runtimes.
- Diagnostics are local-only and redacted; there is no telemetry.

## Known limitations

- Unsigned release candidate (SmartScreen warning on first run).
- Theme preference stored but visual theming is deferred.
- Auto-update disabled until signed updater infrastructure exists.
- Windows-only today (macOS/Linux are not targeted in 1.0).

See [docs/install-windows.md](docs/install-windows.md) for detailed
installation instructions and [CHANGELOG.md](CHANGELOG.md) for the full
release history.
