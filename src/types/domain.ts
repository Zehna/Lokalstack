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

/** Response of the `get_port_listeners` Tauri command. */
export interface PortListenersResponse {
  listeners: PortListener[]
  /** Unix epoch milliseconds at which the snapshot was taken. */
  lastUpdated: number
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
