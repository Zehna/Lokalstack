import { create } from 'zustand'

import { getPortListeners } from '@/services/native/ports'
import type {
  PidControl,
  PortListener,
  ProcessInfo,
  ProjectIdentity,
  ServiceIdentity,
} from '@/types/domain'

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
  /** Unique resolved projects (Phase 4), by root path. */
  projects: ProjectIdentity[]
  /** PID → project identity. Many PIDs may share one project. */
  projectByPid: ReadonlyMap<number, ProjectIdentity>
  /** Control capability + browser URLs per PID (Phase 5). */
  controlByPid: ReadonlyMap<number, PidControl>
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
   * A manual refresh also re-reads project metadata from the filesystem
   * (`bypassProjectCache`), picking up branch changes and new markers.
   */
  refreshListeners: () => Promise<void>
}

/**
 * Real port + process-discovery state, fed by the `get_port_listeners` Tauri
 * command via the native client. No mock data ever enters this store.
 */
export const usePortsStore = create<PortsState>()((set, get) => {
  async function runCycle(bypassProjectCache = false): Promise<void> {
    try {
      const response = await getPortListeners(bypassProjectCache)
      // Phase 10B (§L): treat a contract-violating response as an empty
      // snapshot — degrading beats crashing a live view.
      const safe = {
        listeners: response.listeners ?? [],
        processes: response.processes ?? [],
        services: response.services ?? [],
        projects: response.projects ?? [],
        projectLinks: response.projectLinks ?? [],
        controls: response.controls ?? [],
        lastUpdated: response.lastUpdated ?? Date.now(),
        durationMs: response.durationMs ?? 0,
      }
      const projectById = new Map(safe.projects.map((p) => [p.id, p]))
      set({
        listeners: safe.listeners,
        processByPid: new Map(safe.processes.map((p) => [p.pid, p])),
        serviceByPid: new Map(safe.services.map((s) => [s.pid, s])),
        projects: safe.projects,
        controlByPid: new Map(safe.controls.map((c) => [c.pid, c])),
        projectByPid: new Map(
          safe.projectLinks
            .map((link) => {
              const project = projectById.get(link.projectId)
              return project === undefined ? null : ([link.pid, project] as const)
            })
            .filter((entry): entry is readonly [number, ProjectIdentity] => entry !== null),
        ),
        durationMs: safe.durationMs,
        lastUpdated: safe.lastUpdated,
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
    projects: [],
    projectByPid: new Map(),
    controlByPid: new Map(),
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
      // Manual refresh re-reads project metadata from the filesystem.
      await runCycle(true)
    },
  }
})
