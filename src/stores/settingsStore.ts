/**
 * Application settings store (Phase 10C).
 *
 * Owns the loaded settings + save/reset lifecycle. Polling reconfiguration
 * (spec §I/§J/§K) flows through `applyPollingSettings`, which every polling
 * store subscribes to: Auto Refresh OFF stops the shared timers without
 * destroying consumer leases; an interval change recreates exactly one
 * timer (never two). Manual refresh always works — it calls the store
 * actions directly, never the polling owner.
 *
 * Defaults live server-side and arrive via `getAppSettings`; the frontend
 * never hardcodes a default that could drift from Rust (spec §B).
 */
import { create } from 'zustand'

import {
  getAppSettings,
  resetAppSettings,
  saveAppSettings,
  setRunAtStartup,
} from '@/services/native/ports'
import type { AppSettings, SaveSettingsResult } from '@/types/domain'

interface SettingsState {
  /** Loaded settings; null until the first successful load. */
  settings: AppSettings | null
  loading: boolean
  saving: boolean
  /** Last save/reset error (display-safe; details go to diagnostics). */
  error: string | null
  /** "Invalid value corrected" note from the last save (spec §AC). */
  correctedNote: string | null
  /** True after the first successful load (guards polling re-apply). */
  loaded: boolean

  load: () => Promise<void>
  save: (patch: Partial<AppSettings>) => Promise<void>
  reset: () => Promise<void>
  toggleStartup: (enabled: boolean) => Promise<void>
  /** Clear the error/correctedNote without changing settings. */
  dismissNotices: () => void
}

/* ------------------------------------------------------------------------ *
 * Polling reconfiguration plumbing (spec §I)
 * ---------------------------------------------------------------------- */

/** Reconfiguration hooks registered by polling stores. Module-level list —
 * settings apply globally by design, mirroring the Rust single source. */
type PollingApplier = (settings: AppSettings) => void
const pollingAppliers: PollingApplier[] = []

/**
 * Register a polling store for live settings application. Returns the
 * unregister closure (test teardown). Each store's applier calls its own
 * polling owner's `configure()` — which keeps consumer leases intact and
 * can never create a second timer (Phase 10A semantics preserved).
 */
export function registerPollingApplier(applier: PollingApplier): () => void {
  pollingAppliers.push(applier)
  return () => {
    const index = pollingAppliers.indexOf(applier)
    if (index >= 0) pollingAppliers.splice(index, 1)
  }
}

/** Apply current polling-relevant settings to every registered store. */
function applyPollingSettings(settings: AppSettings): void {
  for (const applier of pollingAppliers) {
    applier(settings)
  }
}

/** Test-only: drop every registered applier. */
export function resetPollingAppliers(): void {
  pollingAppliers.length = 0
}

export const useSettingsStore = create<SettingsState>()((set, get) => {
  async function runLoad(): Promise<void> {
    set({ loading: true })
    try {
      const settings = await getAppSettings()
      set({
        settings,
        loading: false,
        error: null,
        loaded: true,
        correctedNote: null,
      })
      applyPollingSettings(settings)
    } catch (cause) {
      set({
        loading: false,
        error: cause instanceof Error ? cause.message : String(cause),
      })
    }
  }

  return {
    settings: null,
    loading: true,
    saving: false,
    error: null,
    correctedNote: null,
    loaded: false,

    load: async () => {
      await runLoad()
    },

    save: async (patch) => {
      const current = get().settings
      if (current === null) {
        set({ error: 'Settings are not loaded yet.' })
        return
      }
      set({ saving: true, error: null })
      try {
        // Full patch, optimistic fields merged from current settings.
        const result: SaveSettingsResult = await saveAppSettings({
          autoRefresh: patch.autoRefresh ?? current.autoRefresh,
          portRefreshIntervalMs:
            patch.portRefreshIntervalMs ?? current.portRefreshIntervalMs,
          aiPollingEnabled: patch.aiPollingEnabled ?? current.aiPollingEnabled,
          dockerPollingEnabled:
            patch.dockerPollingEnabled ?? current.dockerPollingEnabled,
          launchMinimized: patch.launchMinimized ?? current.launchMinimized,
          closeBehavior: patch.closeBehavior ?? current.closeBehavior,
          theme: patch.theme ?? current.theme,
        })
        set({
          settings: result.settings,
          saving: false,
          correctedNote: result.correctedNote,
          error: null,
        })
        applyPollingSettings(result.settings)
      } catch (cause) {
        set({
          saving: false,
          error: cause instanceof Error ? cause.message : String(cause),
        })
      }
    },

    reset: async () => {
      set({ saving: true, error: null })
      try {
        const settings = await resetAppSettings()
        set({ settings, saving: false, correctedNote: null, error: null })
        applyPollingSettings(settings)
      } catch (cause) {
        set({
          saving: false,
          error: cause instanceof Error ? cause.message : String(cause),
        })
      }
    },

    toggleStartup: async (enabled) => {
      set({ saving: true, error: null })
      try {
        // The backend returns the post-toggle view whose `startupRegistered`
        // reflects OS reality — the UI never lies (spec §U, §AH).
        const settings = await setRunAtStartup(enabled)
        set({ settings, saving: false, error: null })
      } catch (cause) {
        set({
          saving: false,
          error: cause instanceof Error ? cause.message : String(cause),
        })
      }
    },

    dismissNotices: () => set({ error: null, correctedNote: null }),
  }
})
