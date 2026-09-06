/**
 * Native client boundary — the only module allowed to call Tauri `invoke`.
 *
 * Display components never invoke Tauri directly; they go through stores and
 * hooks, which go through this client. Keeping the boundary here means the
 * transport (Tauri IPC today, something else tomorrow) is swappable and the
 * rest of the frontend stays pure TypeScript.
 */

import { invoke } from '@tauri-apps/api/core'

import type { PortListenersResponse } from '@/types/domain'

/** Shape returned by the Rust `get_port_listeners` command. */
interface RawPortListenersResponse {
  listeners: PortListenersResponse['listeners']
  processes: PortListenersResponse['processes']
  services: PortListenersResponse['services']
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
export async function getPortListeners(): Promise<PortListenersResponse> {
  if (typeof window === 'undefined' || !('__TAURI_INTERNALS__' in window)) {
    throw new Error(
      'Tauri IPC bridge not available. LocalStack discovery needs the desktop app — a plain browser tab cannot read Windows TCP tables.',
    )
  }
  const raw = await invoke<RawPortListenersResponse>('get_port_listeners')
  return {
    listeners: raw.listeners,
    processes: raw.processes,
    services: raw.services,
    lastUpdated: raw.capturedAt,
    durationMs: raw.durationMs,
  }
}
