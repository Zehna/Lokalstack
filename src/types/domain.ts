/**
 * Domain types for LocalStack Control Center.
 *
 * These are the TypeScript mirror of the serde DTOs that Tauri commands
 * return. They intentionally stay small: fields are added when an engine
 * actually produces the data, not before.
 */

/** Lifecycle state of a local service as reported by the backend. */
export type ServiceStatus = 'running' | 'stopped' | 'unhealthy' | 'unknown'

/**
 * Coarse service category. Phase 3 will refine this into concrete
 * framework/service kinds (Next.js, Flask, PostgreSQL, Ollama, ...).
 */
export type ServiceType =
  | 'frontend'
  | 'backend'
  | 'database'
  | 'ai'
  | 'docker'
  | 'infrastructure'
  | 'unknown'

/**
 * A local service observed on the machine.
 *
 * Phase 0 used this for mock data only. From Phase 1 on it is also the shape
 * used for *real* data, with `name` staying null when nothing but the socket
 * is known — the UI must not invent identities.
 */
export interface Service {
  /** Stable identifier, e.g. `port-3000` or `pid-4212` once discovery exists. */
  id: string
  /**
   * Human-readable display name, or `null` when unknown. Discovery engines
   * must not guess: an unnamed service renders as its address/port.
   */
  name: string | null
  /** Coarse category of the service. */
  type: ServiceType
  /** Local listening port, when known. */
  port: number | null
  /** Owning process ID, when known. */
  pid: number | null
  /** Executable / process name, when known. */
  processName: string | null
  /** Current lifecycle state. */
  status: ServiceStatus
}

/**
 * One TCP listener as reported by the native Windows discovery engine
 * (`GetExtendedTcpTable`). Mirrors the Rust `PortListener` DTO — the JSON
 * contract between `src-tauri/src/discovery/` and this file.
 */
export interface PortListener {
  /** Transport protocol — `"tcp"` in Phase 1. */
  protocol: 'tcp'
  /** IP version of the binding. */
  ipVersion: 4 | 6
  /** Bind address: `127.0.0.1`, `0.0.0.0`, `::1`, `::`, or a specific LAN address. */
  localAddress: string
  /** Local TCP port in host byte order. */
  port: number
  /** PID of the socket owner (0 = system-owned). */
  pid: number
  /** Socket state — `"LISTEN"` in Phase 1. */
  state: 'LISTEN'
}

/**
 * One Windows process, as far as the OS let us inspect it. Mirrors the Rust
 * `ProcessInfo` DTO from `src-tauri/src/process/`.
 *
 * Access-denied is a first-class outcome, not an error: `accessible: false`
 * means the listener exists but Windows refused (or the process died before
 * inspection) — `name`/`executablePath` may still carry a snapshot-derived
 * display name, but the rich metadata is honestly `null`. The UI must render
 * `null` as "unavailable", never as a guess.
 */
export interface ProcessInfo {
  /** Process ID the snapshot was taken for. */
  pid: number
  /** Image basename (e.g. `node.exe`), or `null` when unknown. */
  name: string | null
  /** Full executable path, or `null` when Windows does not reveal it. */
  executablePath: string | null
  /** Process start time as Unix epoch milliseconds, or `null`. */
  startedAt: number | null
  /** Working set in bytes (raw domain value; format only in the UI), or `null`. */
  memoryBytes: number | null
  /**
   * Full command line (read via the PEB walk), or `null` when unreadable —
   * the primary evidence for framework detection.
   */
  commandLine: string | null
  /**
   * CPU percent over the last sampling window (per-core normalized, 0–100),
   * or `null` on the first observation — no fabricated `0%` before a real
   * delta exists.
   */
  cpuPercent: number | null
  /** Whether the process could be inspected this cycle. */
  accessible: boolean
}

/**
 * How sure the native detector is about a service identity. Enum (not a
 * number) on purpose: honest buckets, no fabricated percentages.
 */
export type Confidence = 'low' | 'medium' | 'high' | 'exact'

/** Developer-facing category of a detected service. */
export type ServiceCategory =
  | 'frontend'
  | 'backend'
  | 'database'
  | 'ai'
  | 'infrastructure'
  | 'unknown'

/** One piece of evidence backing a classification (for the details view). */
export interface Evidence {
  /** Where it came from: `process_name`, `executable_path`, `command_line`. */
  source: string
  /** The matched value (substring or argument that fired the rule). */
  value: string
}

/**
 * Developer-facing identity of one process, classified from evidence by the
 * native intelligence layer. Never a guess: `confidence` and `evidence`
 * always accompany the claim, and generic runtimes stay generic ("Node.js",
 * "Python") when framework evidence is missing.
 */
export interface ServiceIdentity {
  /** Concrete identity kind, e.g. `node_js`, `vite`, `postgres_sql`, `unknown`. */
  kind: string
  /** Human-facing display name: `PostgreSQL`, `Vite`, `Node.js`, `node.exe`. */
  displayName: string
  /** Category for filtering/grouping. */
  category: ServiceCategory
  /** How confident the detector is. */
  confidence: Confidence
  /** Evidence that produced this identity, strongest first. */
  evidence: Evidence[]
}

/** A `ServiceIdentity` attached to the PID it belongs to. */
export interface PidServiceIdentity extends ServiceIdentity {
  pid: number
}

/** Ecosystem a resolved project belongs to, from its markers. */
export type ProjectKind = 'node_js' | 'python' | 'rust' | 'go' | 'unknown'

/** Git facts about a project, as far as file parsing revealed them. */
export interface GitInfo {
  /** Whether the project belongs to a Git repository at all. */
  isRepository: boolean
  /** Repository working-tree root, or `null` when not a repository. */
  rootPath: string | null
  /** Current branch, or `null` for detached HEAD / unknown — never guessed. */
  branch: string | null
}

/** An inferred start command, with its own explicit confidence. */
export interface StartCommand {
  /** Display string, e.g. `npm run dev` or the raw `vite …` command line. */
  command: string
  /** How much the inference is trusted. */
  confidence: Confidence
  /** Evidence backing the inference. */
  evidence: Evidence[]
}

/**
 * A resolved local project, associated with running processes by evidence
 * (command-line paths → confirmed project-root markers → Git root). Never
 * assigned from port numbers or "nearest package.json" guesses.
 */
export interface ProjectIdentity {
  /** Stable id — the confirmed project root path. Shared by all its PIDs. */
  id: string
  /** Manifest name when readable, else the directory basename. */
  name: string
  /** Confirmed project root directory. */
  rootPath: string
  /** Ecosystem the markers identify. */
  kind: ProjectKind
  /** Git repository/branch facts. */
  git: GitInfo
  /** JS package manager (`npm`, `pnpm`, `Yarn`, `Bun`, `Ambiguous`), or `null`. */
  packageManager: string | null
  /** Inferred start command, or `null` when nothing honest can be said. */
  startCommand: StartCommand | null
  /** How strongly the process→project association is evidenced. */
  confidence: Confidence
  /** Evidence that produced this identity, strongest first. */
  evidence: Evidence[]
}

/** A PID's link to a project (many PIDs may share one project identity). */
export interface PidProjectLink {
  pid: number
  /** References `ProjectIdentity.id` in the same response. */
  projectId: string
}

/** Response of the `get_port_listeners` Tauri command. */
export interface PortListenersResponse {
  listeners: PortListener[]
  /** Process metadata for every unique PID in the listener list. */
  processes: ProcessInfo[]
  /** Service identity per PID (PID-based; shared across a PID's ports). */
  services: PidServiceIdentity[]
  /** Unique projects resolved from process evidence (Phase 4). */
  projects: ProjectIdentity[]
  /** PID → project id; many PIDs may share one project. */
  projectLinks: PidProjectLink[]
  /** Unix epoch milliseconds at which the snapshot was taken. */
  lastUpdated: number
  /** Wall-clock duration of the native discovery cycle, in milliseconds. */
  durationMs: number
}

/** Aggregated CPU / memory usage snapshot. */
export interface SystemUsage {
  /** Overall CPU utilization, 0–100. */
  cpuPercent: number
  /** Overall memory utilization, 0–100. */
  memoryPercent: number
}

/** A detected conflict: more than one listener competing for the same port. */
export interface PortConflict {
  /** Port that has competing listeners. */
  port: number
  /** Services / processes involved in the conflict. */
  services: Service[]
}

/**
 * A development workspace: a group of related services that typically
 * belong to the same project or repository (Phase 6+).
 */
export interface Workspace {
  id: string
  name: string
  services: Service[]
}

/** Identifier of a view in the desktop shell's left navigation. */
export type ViewId =
  | 'dashboard'
  | 'projects'
  | 'services'
  | 'ports'
  | 'workspaces'
  | 'ai-services'
  | 'history'
  | 'settings'
