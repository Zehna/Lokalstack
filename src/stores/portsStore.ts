import { create } from 'zustand'

import { getPortListeners } from '@/services/native/ports'
import type { PortListener } from '@/types/domain'

/** One in-flight or completed refresh generation. */
interface PortsState {
  /** All TCP listeners from the last successful native snapshot. */
  listeners: PortListener[]
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
 * Real port-discovery state, fed by the `get_port_listeners` Tauri command
 * via the native client. No mock data ever enters this store.
 */
export const usePortsStore = create<PortsState>()((set, get) => ({
  listeners: [],
  loading: true,
  refreshing: false,
  error: null,
  lastUpdated: null,

  loadListeners: async () => {
    if (get().refreshing) return
    set({ refreshing: true, loading: true, error: null })
    try {
      const response = await getPortListeners()
      set({
        listeners: response.listeners,
        lastUpdated: response.lastUpdated,
        loading: false,
        refreshing: false,
        error: null,
      })
    } catch (cause) {
      set({
        loading: false,
        refreshing: false,
        error: cause instanceof Error ? cause.message : String(cause),
      })
    }
  },

  refreshListeners: async () => {
    if (get().refreshing) return
    set({ refreshing: true })
    try {
      const response = await getPortListeners()
      set({
        listeners: response.listeners,
        lastUpdated: response.lastUpdated,
        error: null,
        refreshing: false,
        loading: false,
      })
    } catch (cause) {
      set({
        refreshing: false,
        loading: false,
        error: cause instanceof Error ? cause.message : String(cause),
      })
    }
  },
}))
