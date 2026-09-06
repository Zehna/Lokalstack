# LocalStack Control Center

A Windows-first desktop control center for your local development environment:
understand everything currently running on **localhost** — services, ports,
processes, projects, workspaces and local AI runtimes.

> **Status: Phase 0 — Foundation.** The app is a UI/architecture shell with
> mock data only. It does **not** discover or interact with anything on your
> machine yet. See [docs/roadmap.md](docs/roadmap.md).

## Tech stack

| Layer     | Choice                                        |
|-----------|-----------------------------------------------|
| Desktop   | Tauri v2 (Rust)                               |
| Frontend  | React + TypeScript + Vite                     |
| UI        | Tailwind CSS + Lucide icons                   |
| State     | Zustand                                       |

## Getting started

Prerequisites: Node.js 20+, npm 10+, and the
[Tauri v2 prerequisites for Windows](https://tauri.app/start/prerequisites/)
(MSVC toolchain + WebView2).

```bash
npm install
npm run tauri dev     # run the desktop app in development
npm run tauri build   # build a distributable installer
```

Frontend-only workflow (browser, no Tauri window):

```bash
npm run dev         # Vite dev server on http://localhost:1420
npm run typecheck   # tsc --noEmit
npm run build       # typecheck + production build
```

Rust checks:

```bash
cd src-tauri
cargo check
```

## Project layout

```
src/                  React frontend (presentation + state only)
  app/                Desktop shell: layout, sidebar, navigation
  components/         Cross-feature UI primitives (as needed)
  features/           Feature pages: dashboard, services, ports, projects,
                      workspaces, ai-services, history, settings
  hooks/              Side-effect hooks (mock sim today, Tauri wiring later)
  stores/             Zustand stores
  types/              Domain types (contract with Tauri commands)
  utils/              Pure helpers
src-tauri/src/        Rust backend (owns all OS interaction)
  discovery/          Future: port/process/service discovery (Phases 1–3)
  health/             Future: localhost health checks
  control/            Future: explicit, user-confirmed service control
  conflicts/          Future: port conflict engine
  workspace/          Future: workspace detection
  ai/                 Future: local AI runtime detection
docs/                 architecture.md, roadmap.md
```

## Safety model

LocalStack observes localhost. It never scans remote or external networks,
never kills processes, never modifies environment variables, firewall rules,
databases or Docker, and never performs privileged operations. See
[docs/architecture.md](docs/architecture.md).

## License

TBD (open source — to be decided before first public release).
