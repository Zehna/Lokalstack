import { create } from 'zustand'

import { getPortListeners } from '@/services/native/ports'
import type { PortListener, ProcessInfo, ServiceIdentity } from '@/types/domain'

/** One in-flight or completed refresh generation. */
interface PortsState {
  /** All TCP listeners from the last successful native snapshot. */
  listeners: PortListener[]
  /**
   * Process intelligence for the unique PIDs of the last snapshot, keyed by
   * PID for O(1) lookup when rendering listener rows.
   */
  processByPid: ReadonlyMap<number, ProcessInfo>
  /**
   * Service/framework identity per PID (Phase 3 intelligence layer).
   * PID-based: all listener rows of one process share one identity.
   */
  serviceByPid: ReadonlyMap<number, ServiceIdentity>
  /** Wall-clock duration of the last native cycle, or null. */
  durationMs: number | null
  /** True until the first snapshot (success or failure) arrives. */
  loading: boolean
  /** True while any refresh is in flight (drives the manual-refresh spinner). */
  refreshing: boolean
  /** Last error message, or null. Errors never wipe previously loaded data. */
  error: string | null
  /** Unix epoch milliseconds of the last successful snapshot. */
  lastUpdated: number | null

  /**
   * Initial load: clears data, shows the loading state, fetches once.
   * Skips itself if a refresh is already in flight.
   */
  loadListeners: () => Promise<void>
  /**
   * Manual/background refresh: keeps stale data visible, updates in place.
   * Skips itself if a refresh is already in flight (no overlapping requests).
   */
  refreshListeners: () => Promise<void>
}

/**
 * Real port + process-discovery state, fed by the `get_port_listeners` Tauri
 * command via the native client. No mock data ever enters this store.
 */
export const usePortsStore = create<PortsState>()((set, get) => {
  async function runCycle(): Promise<void> {
    try {
      const response = await getPortListeners()
      set({
        listeners: response.listeners,
        processByPid: new Map(response.processes.map((p) => [p.pid, p])),
        serviceByPid: new Map(response.services.map((s) => [s.pid, s])),
        durationMs: response.durationMs,
        lastUpdated: response.lastUpdated,
        error: null,
        refreshing: false,
        loading: false,
      })
    } catch (cause) {
      set({
        loading: false,
        refreshing: false,
        error: cause instanceof Error ? cause.message : String(cause),
      })
    }
  }

  return {
    listeners: [],
    processByPid: new Map(),
    serviceByPid: new Map(),
    durationMs: null,
    loading: true,
    refreshing: false,
    error: null,
    lastUpdated: null,

    loadListeners: async () => {
      if (get().refreshing) return
      set({ refreshing: true, loading: true, error: null })
      await runCycle()
    },

    refreshListeners: async () => {
      if (get().refreshing) return
      set({ refreshing: true })
      await runCycle()
    },
  }
})
