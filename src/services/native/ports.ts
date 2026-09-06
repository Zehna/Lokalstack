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
