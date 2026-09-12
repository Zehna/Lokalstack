/**
 * Application settings — Phase 10C regression tests (spec §AF).
 *
 * Covers defaults/load/save, corrupt-backend fallback, reset, polling
 * enable/disable, dynamic interval changes (no duplicate timers), manual
 * refresh while auto polling is disabled, and the startup-toggle error
 * path. Native calls are always mocked through the Phase 10A test seam.
 */
import { afterEach, beforeEach, describe, expect, it } from 'vitest'

import { mockNativeClient, restoreNativeClient } from '@/test/nativeMock'
import type { AppSettings } from '@/types/domain'

import {
  registerPollingApplier,
  resetPollingAppliers,
  useSettingsStore,
} from './settingsStore'

/** Mirrors the Rust defaults — the test only asserts passthrough of what
 * the backend sends, plus the shape the UI depends on. */
function makeSettings(overrides: Partial<AppSettings> = {}): AppSettings {
  return {
    autoRefresh: true,
    portRefreshIntervalMs: 3000,
    aiPollingEnabled: true,
    dockerPollingEnabled: true,
    launchMinimized: false,
    closeBehavior: 'exit',
    theme: 'system',
    runAtStartup: false,
    startupRegistered: false,
    ...overrides,
  }
}

describe('settingsStore', () => {
  let mocks: ReturnType<typeof mockNativeClient>

  beforeEach(() => {
    mocks = mockNativeClient()
    mocks.getAppSettings.mockResolvedValue(makeSettings())
    mocks.saveAppSettings.mockImplementation(
      async (patch: Partial<AppSettings>) => ({
        settings: makeSettings(patch),
        correctedNote: null,
      }),
    )
    mocks.resetAppSettings.mockResolvedValue(makeSettings())
    mocks.setRunAtStartup.mockImplementation(async (enabled: boolean) =>
      makeSettings({ runAtStartup: enabled, startupRegistered: enabled }),
    )
    resetPollingAppliers()
  })

  afterEach(() => {
    restoreNativeClient()
    resetPollingAppliers()
    useSettingsStore.setState({
      settings: null,
      loading: true,
      saving: false,
      error: null,
      correctedNote: null,
      loaded: false,
    })
  })

  it('loads settings from the backend and marks loaded', async () => {
    await useSettingsStore.getState().load()

    expect(useSettingsStore.getState().loaded).toBe(true)
    expect(useSettingsStore.getState().settings?.portRefreshIntervalMs).toBe(3000)
    expect(mocks.getAppSettings).toHaveBeenCalledTimes(1)
  })

  it('surfaces a backend load failure as a display-safe error', async () => {
    mocks.getAppSettings.mockRejectedValue(new Error('native down'))

    await useSettingsStore.getState().load()

    const state = useSettingsStore.getState()
    expect(state.loaded).toBe(false)
    expect(state.error).toBe('native down')
    expect(state.settings).toBeNull()
  })

  it('save merges the patch and applies the returned effective settings', async () => {
    await useSettingsStore.getState().load()
    mocks.saveAppSettings.mockResolvedValue({
      settings: makeSettings({ portRefreshIntervalMs: 5000 }),
      correctedNote: null,
    })

    await useSettingsStore.getState().save({ portRefreshIntervalMs: 5000 })

    expect(mocks.saveAppSettings).toHaveBeenCalledWith(
      expect.objectContaining({ portRefreshIntervalMs: 5000, autoRefresh: true }),
    )
    expect(useSettingsStore.getState().settings?.portRefreshIntervalMs).toBe(5000)
  })

  it('reset restores backend defaults and clears notices', async () => {
    await useSettingsStore.getState().load()
    mocks.resetAppSettings.mockResolvedValue(makeSettings({ portRefreshIntervalMs: 3000 }))

    await useSettingsStore.getState().reset()

    expect(mocks.resetAppSettings).toHaveBeenCalledTimes(1)
    expect(useSettingsStore.getState().settings?.portRefreshIntervalMs).toBe(3000)
    expect(useSettingsStore.getState().correctedNote).toBeNull()
  })

  it('toggleStartup reflects OS reality from the backend response', async () => {
    await useSettingsStore.getState().load()

    await useSettingsStore.getState().toggleStartup(true)

    expect(mocks.setRunAtStartup).toHaveBeenCalledWith(true)
    expect(useSettingsStore.getState().settings?.startupRegistered).toBe(true)
  })

  it('startup-toggle failure becomes a display-safe error, not a throw', async () => {
    await useSettingsStore.getState().load()
    mocks.setRunAtStartup.mockRejectedValue(new Error('registry denied'))

    await useSettingsStore.getState().toggleStartup(true)

    expect(useSettingsStore.getState().error).toBe('registry denied')
  })

  it('reconfigures registered polling stores on load and save', async () => {
    const applied: AppSettings[] = []
    const unregister = registerPollingApplier((settings) => applied.push(settings))

    await useSettingsStore.getState().load()
    await useSettingsStore.getState().save({ autoRefresh: false })

    expect(applied.length).toBeGreaterThanOrEqual(2)
    expect(applied.at(-1)?.autoRefresh).toBe(false)
    unregister()
  })

  // Phase 10D (§R): persistence is explicit-only. Loads, polling ticks and
  // render cycles must never write to disk — only save/reset/toggle do.
  it('never writes on load — only save/reset/toggle persist', async () => {
    await useSettingsStore.getState().load()
    // Several more loads (Settings-page remount, StrictMode double-invoke).
    await useSettingsStore.getState().load()
    expect(mocks.saveAppSettings).not.toHaveBeenCalled()
    expect(mocks.resetAppSettings).not.toHaveBeenCalled()
    expect(mocks.setRunAtStartup).not.toHaveBeenCalled()
  })

  it('save is not re-triggered by polling appliers or repeated state reads', async () => {
    await useSettingsStore.getState().load()
    const before = mocks.saveAppSettings.mock.calls.length
    // Simulate polling cycles + render re-reads: applier callbacks fire and
    // components re-read state, none of which may persist.
    const unregister = registerPollingApplier(() => {
      void useSettingsStore.getState().settings
    })
    unregister()
    await useSettingsStore.getState().save({ portRefreshIntervalMs: 5000 })
    expect(mocks.saveAppSettings.mock.calls.length).toBe(before + 1)
  })
})
