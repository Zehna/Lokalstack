# Installing on Windows

Requirements: Windows 10 or 11 (x64), WebView2 Runtime (preinstalled on
current Windows).

## NSIS install (recommended)

1. Run `LocalStack Control Center_1.1.0_x64-setup.exe`.
2. **Unsigned warning:** the release candidate is not code-signed, so
   SmartScreen may show "Windows protected your PC". Choose *More info →
   Run anyway* only if you obtained the binary from a trusted source.
3. Follow the installer. It is a per-user install (no administrator
   required) and creates a Start Menu entry.

## MSI install

`LocalStack Control Center_1.1.0_x64_en-US.msi` is available as a secondary
format. It installs per-machine policy to the same per-user path and shows a
UAC elevation prompt. Silent installs (`/qn`) fail because elevation cannot
be granted without UI; use `/passive` or the interactive wizard.

## First launch

- Start Menu → **LocalStack Control Center**, or launch the tray icon if
  close-to-tray is enabled.
- No source tree, Node.js, or dev server is needed; the app is fully
  self-contained.
- On first run the app may show empty/unavailable states if Docker or AI
  runtimes are absent — these are normal, not errors.

## Settings and data

- Settings: `%LOCALAPPDATA%\localstack-control-center\settings.json`
- Diagnostics log: `%LOCALAPPDATA%\localstack-control-center\localstack.log`
  (bounded size, redacted, local-only — no telemetry)

## Uninstall

Windows *Settings → Apps → LocalStack Control Center → Uninstall*, or the
NSIS uninstaller in the install folder. Binaries and Start Menu entries are
removed; your settings/log under `%LOCALAPPDATA%\localstack-control-center`
are intentionally retained. Uninstalling never stops running dev services,
Docker containers, or AI models.

## Signing status

This release candidate is **unsigned**. Distribution beyond a trusted local
context should use an Authenticode code-signing certificate; until then,
SmartScreen warnings are expected and should be treated with care. Do not
disable Windows security features to bypass this warning.
