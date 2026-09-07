/**
 * Native client boundary — the only module allowed to call Tauri `invoke`.
 *
 * Display components never invoke Tauri directly; they go through stores and
 * hooks, which go through this client. Keeping the boundary here means the
 * transport (Tauri IPC today, something else tomorrow) is swappable and the
 * rest of the frontend stays pure TypeScript.
 */

import { invoke } from '@tauri-apps/api/core'

import type { PortListenersResponse, StopResult } from '@/types/domain'

/** Shape returned by the Rust `get_port_listeners` command. */
interface RawPortListenersResponse {
  listeners: PortListenersResponse['listeners']
  processes: PortListenersResponse['processes']
  services: PortListenersResponse['services']
  projects: PortListenersResponse['projects']
  projectLinks: PortListenersResponse['projectLinks']
  controls: PortListenersResponse['controls']
  /** Unix epoch milliseconds at which the backend captured the snapshot. */
  capturedAt: number
  /** Wall-clock duration of the native cycle, in milliseconds. */
  durationMs: number
}

/**
 * Read-only Tauri command: every TCP listener currently bound on this
 * machine (IPv4 + IPv6) plus process intelligence for each owning PID,
 * sampled once per cycle.
 *
 * Rejects with a human-readable error string when the native engine fails;
 * callers are expected to surface that in UI error states, not crash.
 */
export async function getPortListeners(bypassProjectCache = false): Promise<PortListenersResponse> {
  if (typeof window === 'undefined' || !('__TAURI_INTERNALS__' in window)) {
    throw new Error(
      'Tauri IPC bridge not available. LocalStack discovery needs the desktop app — a plain browser tab cannot read Windows TCP tables.',
    )
  }
  const raw = await invoke<RawPortListenersResponse>('get_port_listeners', {
    bypassProjectCache,
  })
  return {
    listeners: raw.listeners,
    processes: raw.processes,
    services: raw.services,
    projects: raw.projects,
    projectLinks: raw.projectLinks,
    controls: raw.controls,
    lastUpdated: raw.capturedAt,
    durationMs: raw.durationMs,
  }
}

/**
 * End the process behind an opaque target id (user-confirmed termination).
 * The backend resolves the id in its own registry, re-inspects the process,
 * revalidates identity, and recomputes eligibility before acting — the
 * frontend supplies no PID and no policy fields.
 */
export async function endProcess(targetId: string): Promise<StopResult> {
  return invoke<StopResult>('end_process', { targetId })
}

/**
 * Open a localhost URL (from the snapshot's `controls[].urls`) in the
 * user's default browser. The backend re-checks URL safety and, when a PID
 * is given, that the process still exists.
 */
export async function openServiceUrl(url: string, pid: number | null): Promise<void> {
  return invoke<void>('open_service_url', { url, pid })
}

/* ------------------------------------------------------------------------
 * Workspaces (Phase 6) — managed lifecycle
 * ---------------------------------------------------------------------- */

import type {
  LaunchCandidate,
  LogBatch,
  ManagedActionOutcome,
  WorkspaceView,
} from '@/types/domain'

/** Read-only: launch candidates derived from a project root's manifests. */
export async function getWorkspaceCandidates(projectRoot: string): Promise<LaunchCandidate[]> {
  return invoke<LaunchCandidate[]>('get_workspace_candidates', { projectRoot })
}

/** Create a workspace (explicit user action) for a project root. */
export async function createWorkspace(projectRoot: string): Promise<WorkspaceView> {
  return invoke<WorkspaceView>('create_workspace', { projectRoot })
}

/** Remove a workspace (managed processes keep running). */
export async function removeWorkspace(workspaceId: string): Promise<void> {
  return invoke<void>('remove_workspace', { workspaceId })
}

/** All workspaces with live managed-state views. */
export async function listWorkspaces(): Promise<WorkspaceView[]> {
  return invoke<WorkspaceView[]>('list_workspaces')
}

/** START one workspace service by opaque launch-spec id. */
export async function startManagedService(launchSpecId: string): Promise<ManagedActionOutcome> {
  return invoke<ManagedActionOutcome>('start_managed_service', { launchSpecId })
}

/** STOP one managed service; `force` only after a graceful timeout. */
export async function stopManagedService(
  managedId: string,
  force = false,
): Promise<ManagedActionOutcome> {
  return invoke<ManagedActionOutcome>('stop_managed_service', { managedId, force })
}

/** RESTART one managed service from its trusted launch spec. */
export async function restartManagedService(managedId: string): Promise<ManagedActionOutcome> {
  return invoke<ManagedActionOutcome>('restart_managed_service', { managedId })
}

/** START WORKSPACE: managed services only, in deterministic role order. */
export async function startWorkspaceServices(workspaceId: string): Promise<ManagedActionOutcome[]> {
  return invoke<ManagedActionOutcome[]>('start_workspace_services', { workspaceId })
}

/** STOP MANAGED: only registered managed processes of the workspace. */
export async function stopWorkspaceServices(workspaceId: string): Promise<ManagedActionOutcome[]> {
  return invoke<ManagedActionOutcome[]>('stop_workspace_services', { workspaceId })
}

/** Incremental logs for one managed service. */
export async function getServiceLogs(managedId: string, afterIndex?: number): Promise<LogBatch> {
  return invoke<LogBatch>('get_service_logs', { managedId, afterIndex })
}
