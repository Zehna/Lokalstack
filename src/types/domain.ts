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

/**
 * The backend-issued control target. The frontend receives only the opaque
 * `id` plus display fields — every policy-bearing fact (PID, creation time,
 * service/project classification) lives server-side in the target registry
 * and is recomputed at action time. The frontend cannot construct, forge,
 * or enrich one: the id is a 256-bit hash the backend resolves itself.
 */
export interface ControlTarget {
  /** Opaque, unpredictable registry id. The only value sent to authorize. */
  id: string
  /** Display name for confirmations (advisory; backend re-derives). */
  displayName: string
  /** Executable basename (advisory). */
  processName: string | null
}

/** What the user may do with a process, and why not when they may not. */
export interface ControlCapability {
  /** A browser-friendly localhost URL exists for the process's listeners. */
  canOpen: boolean
  /** The process is a controllable development process (End Process
   * available after explicit confirmation). */
  canStop: boolean
  /** Whether a *targeted* graceful stop exists. Always false in Phase 5:
   * externally discovered processes were not launched into a
   * LocalStack-managed process group, and a console-wide CTRL_BREAK
   * broadcast is never used. */
  gracefulStopSupported: boolean
  /** Why `gracefulStopSupported` is what it is. */
  gracefulStopReason: string
  /** Always false in Phase 5 — restart is deferred (documented decision). */
  canRestart: boolean
  /** When `canStop` is false, the honest reason. */
  reason: string
}

/** A PID's control surface from the discovery snapshot. */
export interface PidControl {
  pid: number
  capability: ControlCapability
  /** Browser-friendly URLs (localhost forms only), deduplicated. */
  urls: string[]
  /** Opaque target to echo back for stop actions (controllable PIDs only). */
  target: ControlTarget | null
}

/** Outcome of a stop action, reported honestly. */
export interface StopResult {
  /** The process is gone. */
  stopped: boolean
  /** A *targeted* graceful method existed and was used — always false for
   * externally discovered processes. */
  gracefulAttempted: boolean
  /** True when the process was still alive after the operation. */
  stillRunning: boolean
  /** Human-readable summary for the history log. */
  message: string
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
  /** Control capability + browser URLs per PID (Phase 5). */
  controls: PidControl[]
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

/* ------------------------------------------------------------------------
 * Workspaces (Phase 6) — managed lifecycle
 * ---------------------------------------------------------------------- */

/** Role of a workspace service. Labels, not assumptions. */
export type WorkspaceRole = 'frontend' | 'backend' | 'database' | 'ai' | 'worker' | 'other'

/**
 * One backend-derived launch candidate (shown in the create-workspace
 * flow). The frontend can only accept or decline a candidate — it can
 * never edit the program, args, or cwd.
 */
export interface LaunchCandidate {
  role: WorkspaceRole
  name: string
  /**
   * Opaque-ish display shape; the trusted spec is registered server-side
   * when the workspace is created. Display only.
   */
  spec: {
    program: string
    args: string[]
    cwd: string
    kind: 'exe' | 'batch'
  }
  expectedPort?: number
  /** Where this came from, e.g. `package.json scripts.dev`. */
  source: string
}

/** Lifecycle state of one managed process (tagged union from Rust). */
export type ManagedState =
  | { state: 'starting' }
  | { state: 'running' }
  | { state: 'start_failed'; exit_code: number | null }
  | { state: 'exited'; exit_code: number | null }
  | { state: 'stopping' }
  | { state: 'stopped' }
  | { state: 'stop_timeout' }
  | { state: 'degraded' }

/** Derived workspace status. */
export type WorkspaceStatus =
  | 'stopped'
  | 'starting'
  | 'running'
  | 'partial'
  | 'stopping'
  | 'error'
  | 'conflict'

/** Live managed-process view (opaque id + display state). */
export interface ManagedProcessView {
  managedId: string
  rootPid: number
  state: ManagedState
  startedAt: number
}

/** One service row in a workspace view. */
export interface WorkspaceServiceView {
  id: string
  name: string
  role: WorkspaceRole
  expectedPort: number | null
  /** Opaque launch-spec id — the only handle for starting this service. */
  launchSpecId: string
  source: string
  managed: ManagedProcessView | null
}

/** Full workspace as delivered by the backend. */
export interface WorkspaceView {
  id: string
  projectRoot: string
  name: string
  status: WorkspaceStatus
  services: WorkspaceServiceView[]
}

/** Honest outcome of a managed lifecycle action. */
export interface ManagedActionOutcome {
  ok: boolean
  code: string
  message: string
  managedId?: string
}

/** One captured output line from a managed process. */
export interface LogLine {
  at: number
  stream: 'stdout' | 'stderr'
  line: string
}

/** Incremental log fetch result. */
export interface LogBatch {
  lines: LogLine[]
  lastIndex: number
}

/** Identifier of a view in the desktop shell's left navigation. */
export type ViewId =
  | 'dashboard'
  | 'projects'
  | 'services'
  | 'ports'
  | 'workspaces'
  | 'ai-services'
  | 'docker'
  | 'history'
  | 'settings'

/* ------------------------------------------------------------------------
 * Port conflicts (Phase 7A) — evidence-based ownership + advisory options
 * ---------------------------------------------------------------------- */

/** Classified conflict outcome (mirrors Rust `ConflictKind`). */
export type ConflictKind =
  | 'no_conflict'
  | 'already_running'
  | 'same_project_external'
  | 'same_project_managed'
  | 'other_project'
  | 'unknown_owner'
  | 'dual_stack_equivalent'
  | 'reserved_or_unverifiable'

/** How badly the conflict blocks a launch (mirrors Rust `ConflictSeverity`). */
export type ConflictSeverity = 'info' | 'potential' | 'blocking'

/** Lifecycle of the owning process relative to LocalStack. */
export type OwnerLifecycle = 'managed' | 'external'

/** Normalized owner descriptor — fields stay absent when unresolved. */
export interface PortOwner {
  pid: number
  processName?: string
  serviceDisplayName?: string
  projectId?: string
  projectName?: string
  lifecycle: OwnerLifecycle
  managedWorkspace?: string
  managedService?: string
}

/** One observed listener on the requested port. */
export interface ListenerSummary {
  ipVersion: number
  address: string
  pid: number
}

/** Who is asking for the port (backend-derived identity of the requester). */
export interface RequestedBy {
  workspaceId?: string
  workspaceName?: string
  serviceId?: string
  serviceName?: string
  projectId?: string
}

/** Options the UI may offer — all backed by real capabilities. */
export type ResolutionOption =
  | 'open_existing'
  | 'show_owner'
  | 'stop_managed_service'
  | 'find_free_port'

/** Full conflict report from the `evaluate_port` command. */
export interface PortConflictReport {
  requestedPort: number
  requestedBy: RequestedBy
  kind: ConflictKind
  severity: ConflictSeverity
  /** Human-facing root-cause sentence, evidence-based. */
  message: string
  owner?: PortOwner
  listeners: ListenerSummary[]
  resolutions: ResolutionOption[]
}

/** One free-port suggestion candidate. */
export interface PortCandidate {
  port: number
  status: 'available' | 'used' | 'potential_conflict'
  usedBy?: string
}

/* ------------------------------------------------------------------------
 * Dependencies + readiness (Phase 7B)
 * ---------------------------------------------------------------------- */

/** What a dependency points at (tagged union from Rust). */
export type DependencyTarget =
  | { type: 'service'; service_id: string }
  | { type: 'external_service'; name: string; port: number }
  | { type: 'tcp_port'; port: number }
  | { type: 'http_endpoint'; host: string; port: number }

/** Runtime state of one dependency — listening, never health. */
export type DependencyState =
  | 'available'
  | 'unavailable'
  | 'starting'
  | 'unhealthy'
  | 'unknown'
  | 'conflicted'

/** One declared dependency with its live state. */
export interface DependencyView {
  id: string
  workspaceId: string
  sourceServiceId: string
  target: DependencyTarget
  targetLabel: string
  required: boolean
  state: DependencyState
}

/** Severity of one readiness issue. */
export type IssueSeverity = 'info' | 'warning' | 'error'

/** Structured root-cause issue (stable identity via code + ids). */
export interface ReadinessIssue {
  code: string
  severity: IssueSeverity
  title: string
  message: string
  sourceServiceId?: string
  dependencyId?: string
  port?: number
}

/** Readiness + conflicts + dependencies for one workspace. */
export interface WorkspaceReadinessView {
  workspaceId: string
  workspaceName: string
  status: string
  issues: ReadinessIssue[]
  dependencies: DependencyView[]
  conflicts: PortConflictReport[]
  /** Topological start order (when acyclic) for managed services. */
  startOrder?: string[]
}

/** Target option offered when adding a dependency. */
export type TargetOption =
  | { type: 'service'; serviceId: string; name: string; expectedPort: number | null }
  | { type: 'external_service_hint' }

/* ------------------------------------------------------------------------
 * AI runtime intelligence (Phase 8) — read-only observability
 * ---------------------------------------------------------------------- */

/** Runtime health — adapter-evidence-based, distinct from process lifecycle. */
export type AiHealth =
  | 'ready'
  | 'loading'
  | 'busy'
  | 'degraded'
  | 'unavailable'
  | 'unknown'

/** Structured probe error (mirrors Rust `AiProbeError`). */
export type AiProbeError =
  | { kind: 'connect_failed'; detail: string }
  | { kind: 'timeout' }
  | { kind: 'http_status'; status: number }
  | { kind: 'malformed_json'; detail: string }
  | { kind: 'too_large'; limit: number }
  | { kind: 'policy_rejected'; reason: string }

/** One model as observed from a runtime — optional fields stay absent. */
export interface AiModelInfo {
  id: string
  displayName?: string
  family?: string
  parameterSize?: string
  quantization?: string
  sizeBytes?: number
  modifiedAt?: number
  loaded?: boolean
  vramBytes?: number
  status?: string
  expiresAt?: number
}

/** Device/resource observations — only what the runtime reported. */
export interface AiResourceInfo {
  device?: string
  vramTotalBytes?: number
  vramFreeBytes?: number
  modelVramBytes?: number
  queueRunning?: number
  queuePending?: number
}

/** Explicit capability flags — the UI never renders unsupported features. */
export interface AiRuntimeCapabilities {
  models: boolean
  loadedModels: boolean
  version: boolean
  health: boolean
  metrics: boolean
  gpuStats: boolean
  queue: boolean
}

/** llama.cpp-style runtime properties (conservative subset). */
export interface AiLlamaCppProps {
  modelPath?: string
  contextSize?: number
  slotsTotal?: number
  slotsIdle?: number
}

/** Normalized, provider-agnostic runtime snapshot. */
export interface AiRuntimeSnapshot {
  runtimeId: string
  pid: number
  serviceKind: string
  displayName: string
  endpoint: string
  health: AiHealth
  version?: string
  capabilities: AiRuntimeCapabilities
  props?: AiLlamaCppProps
  models: AiModelInfo[]
  loadedModels: AiModelInfo[]
  resources?: AiResourceInfo
  capturedAt: number
  latencyMs: number
  errorLabel?: string
  error?: AiProbeError
}

/* ------------------------------------------------------------------------
 * Docker & container intelligence (Phase 9) — read-only observability
 * ---------------------------------------------------------------------- */

/** Container lifecycle state (mirrors Rust `ContainerState`). */
export type ContainerState =
  | 'created'
  | 'running'
  | 'paused'
  | 'restarting'
  | 'removing'
  | 'exited'
  | 'dead'
  | 'unknown'

/** Docker Healthcheck evidence — distinct from running state. */
export type ContainerHealth =
  | 'healthy'
  | 'unhealthy'
  | 'starting'
  | 'none'
  | 'unknown'

/** Typed Docker failure (mirrors Rust `DockerFailure`). */
export type DockerFailure =
  | { kind: 'docker_unavailable'; detail: string }
  | { kind: 'access_denied'; detail: string }
  | { kind: 'timeout' }
  | { kind: 'api_unsupported'; detail: string }
  | { kind: 'malformed_response'; detail: string }
  | { kind: 'response_too_large'; limit: number }
  | { kind: 'engine_error'; detail: string }

/** Compose identity from canonical labels only. */
export interface DockerComposeIdentity {
  projectName?: string
  serviceName?: string
  containerNumber?: string
  workingDir?: string
  configFiles?: string
}

/** One published/exposed port mapping — host and container stay distinct. */
export interface ContainerPort {
  protocol: string
  containerPort: number
  hostIp?: string
  hostPort?: number
  published: boolean
}

/** One Docker network the container is attached to (metadata only). */
export interface ContainerNetwork {
  name: string
  ipAddress?: string
  gateway?: string
}

/** One read-only stats observation. */
export interface ContainerStats {
  cpuPercent?: number
  memoryUsedBytes?: number
  memoryLimitBytes?: number
  networkRxBytes?: number
  networkTxBytes?: number
}

/** Normalized container — the only container shape that crosses to React. */
export interface DockerContainer {
  id: string
  shortId: string
  name: string
  image: string
  imageId?: string
  imageDigest?: string
  state: ContainerState
  status: string
  health: ContainerHealth
  createdAt?: number
  ports: ContainerPort[]
  compose?: DockerComposeIdentity
  networks: ContainerNetwork[]
  /** Bind mounts (source, destination) — metadata only, contents never read. */
  mounts: [string, string][]
  stats?: ContainerStats
}

/** Engine identity — useful facts only. */
export interface DockerEngineInfo {
  version?: string
  apiVersion?: string
  os?: string
  arch?: string
}

/** Evidence-based container → LocalStack project association. */
export interface ContainerProjectLink {
  containerId: string
  projectId?: string
  confidence: 'exact' | 'high' | 'medium' | 'low' | 'unknown'
  evidence: string
}

/** Published TCP host port → owning container (overlay metadata). */
export interface ContainerPortOwnership {
  hostPort: number
  hostIp?: string
  containerId: string
  containerName: string
  containerPort: number
  protocol: string
  image?: string
  composeProject?: string
  composeService?: string
  projectId?: string
}

/** Full Docker snapshot from `get_docker_snapshot` / `refresh_docker`. */
export interface DockerEngineSnapshot {
  available: boolean
  engine?: DockerEngineInfo
  containers: DockerContainer[]
  projectLinks: ContainerProjectLink[]
  portOwnerships: ContainerPortOwnership[]
  truncated: boolean
  capturedAt: number
  latencyMs: number
  error?: DockerFailure
}

/** Per-container details from `get_container_details`. */
export interface ContainerDetails {
  container: DockerContainer
  projectEvidence?: string
  projectConfidence?: string
}
