/**
 * Diagnostics feature store (Phase 11C Task 16).
 *
 * Owns the Diagnostics page state: overview, deep-health results, incidents,
 * support bundles, textual deep-check progress, and the notification banner.
 * Every action calls ONLY the native client (src/services/native/
 * diagnostics.ts) — this module never imports `invoke` (safetyGuards).
 */
import { create } from 'zustand'

import {
  getDiagnosticsOverview,
  listIncidents,
  listSupportBundles,
  markIncidentReviewed,
  runDeepHealthChecks,
} from '@/services/native/diagnostics'
import type {
  BundleMetaDto,
  DiagnosticsOverviewDto,
  HealthCheckResultDto,
  IncidentDto,
} from '@/types/domain'

interface DiagnosticsState {
  overview: DiagnosticsOverviewDto | null
  healthResults: HealthCheckResultDto[]
  incidents: IncidentDto[]
  bundles: BundleMetaDto[]
  deepCheckRunning: boolean
  /** Textual deep-check progress (accessibility requirement, spec §30). */
  deepCheckProgressText: string
  /** Last banner message; survives until dismissed. */
  lastBanner: string | null
  /** True when the overview reported a finalized previous crash. */
  recoveredFromCrash: boolean
  loadOverview: () => Promise<void>
  runDeepChecks: () => Promise<void>
  loadIncidents: () => Promise<void>
  markReviewed: (incidentId: string) => Promise<void>
  loadBundles: () => Promise<void>
  markBundleReviewed: (bundleId: string) => Promise<void>
  setBanner: (message: string) => void
  dismissBanner: () => void
}

export const useDiagnosticsStore = create<DiagnosticsState>()((set, get) => ({
  overview: null,
  healthResults: [],
  incidents: [],
  bundles: [],
  deepCheckRunning: false,
  deepCheckProgressText: '',
  lastBanner: null,
  recoveredFromCrash: false,

  loadOverview: async () => {
    try {
      const overview = await getDiagnosticsOverview()
      set({ overview })
      if (overview.recoveryBanner && overview.recoveryBanner !== get().lastBanner) {
        set({ lastBanner: overview.recoveryBanner, recoveredFromCrash: true })
      }
    } catch {
      // Diagnostics must never crash the app; the page shows a stale/empty
      // overview and the error surfaces on the next successful load.
    }
  },

  runDeepChecks: async () => {
    // Progress is textual and announced immediately (spec §30). The total
    // is known only after the check list resolves, so the running text
    // starts unnumbered and is finalized below.
    set({ deepCheckRunning: true, deepCheckProgressText: 'Running deep diagnostics…' })
    try {
      const results = await runDeepHealthChecks()
      set({
        healthResults: results,
        deepCheckProgressText: `Running deep diagnostics… ${results.length} of ${results.length} checks complete`,
      })
    } catch {
      set({
        deepCheckProgressText:
          'Running deep diagnostics… failed to complete; try again.',
      })
    } finally {
      set({ deepCheckRunning: false })
    }
  },

  loadIncidents: async () => {
    try {
      set({ incidents: await listIncidents() })
    } catch {
      // keep previous list
    }
  },

  markReviewed: async (incidentId: string) => {
    // Optimistic update; the backend is authoritative on next load.
    set({
      incidents: get().incidents.map((i) =>
        i.incidentId === incidentId ? { ...i, reviewState: 'reviewed' } : i,
      ),
    })
    try {
      await markIncidentReviewed(incidentId)
    } catch {
      // keep the optimistic state; next load reconciles
    }
  },

  loadBundles: async () => {
    try {
      set({ bundles: await listSupportBundles() })
    } catch {
      // keep previous list
    }
  },

  // RULING (Task 16): the Task-14 command surface intentionally has no
  // mark-bundle-reviewed command (bundle review rides the incident flow
  // today); the optimistic local update keeps the UI consistent until a
  // backend command exists.
  markBundleReviewed: async (bundleId: string) => {
    set({
      bundles: get().bundles.map((b) =>
        b.bundleId === bundleId ? { ...b, reviewState: 'reviewed' } : b,
      ),
    })
  },

  setBanner: (message: string) => {
    set({ lastBanner: message })
  },

  dismissBanner: () => {
    set({ lastBanner: null })
  },
}))
