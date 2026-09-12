/**
 * Phase 10C §J/§K — settings-driven polling reconfiguration must preserve
 * every Phase 10A polling-ownership invariant:
 *
 * - Auto Refresh OFF stops the shared timer but KEEPS consumer leases.
 * - Re-enabling restores the cycle for the same consumers.
 * - An interval change recreates exactly ONE timer (never two concurrent).
 * - Ownership acquire/release semantics are untouched by `configure`.
 */
import { afterEach, beforeEach, describe, expect, it, vi, type Mock } from 'vitest'

import { mockNativeClient, restoreNativeClient } from '@/test/nativeMock'
import { createPollingOwner } from './pollingOwner'

/** Typed view of a mocked client function's call log. */
function callCount(fn: unknown): number {
  return (fn as unknown as { mock: Mock['mock'] }).mock.calls.length
}

describe('pollingOwner.configure (Phase 10C settings)', () => {
  beforeEach(() => {
    vi.useFakeTimers()
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  it('stops the shared timer on disable while keeping consumer leases', () => {
    const tick = vi.fn()
    const owner = createPollingOwner(1000, tick)
    const release = owner.acquire('dashboard')

    vi.advanceTimersByTime(1000)
    expect(tick).toHaveBeenCalledTimes(1)

    owner.configure({ enabled: false })
    expect(owner.isRunning()).toBe(false)
    expect(owner.consumerCount()).toBe(1) // lease kept

    vi.advanceTimersByTime(10_000)
    expect(tick).toHaveBeenCalledTimes(1) // no more ticks

    release()
  })

  it('re-enabling restores polling for the same consumers', () => {
    const tick = vi.fn()
    const owner = createPollingOwner(1000, tick)
    const release = owner.acquire('dashboard')

    owner.configure({ enabled: false })
    expect(owner.isRunning()).toBe(false)

    owner.configure({ enabled: true })
    expect(owner.isRunning()).toBe(true)

    vi.advanceTimersByTime(1000)
    expect(tick).toHaveBeenCalledTimes(1)

    release()
  })

  it('interval change recreates exactly one timer, not two', () => {
    const tick = vi.fn()
    const owner = createPollingOwner(3000, tick)
    const release = owner.acquire('dashboard')

    // Burn one tick, then retime to 5 s.
    vi.advanceTimersByTime(3000)
    expect(tick).toHaveBeenCalledTimes(1)

    owner.configure({ intervalMs: 5000 })
    expect(owner.isRunning()).toBe(true)
    expect(owner.intervalMs()).toBe(5000)
    expect(owner.consumerCount()).toBe(1) // lease untouched

    // New timer starts at configure time → next tick 5 s from NOW, not on
    // the old 3 s phase. If a second 3 s timer had leaked, tick 2 would
    // fire at +2999 ms instead of +5000 ms.
    vi.advanceTimersByTime(4999)
    expect(tick).toHaveBeenCalledTimes(1)
    vi.advanceTimersByTime(1)
    expect(tick).toHaveBeenCalledTimes(2) // fired exactly at the new cadence

    release()
  })

  it('configure with no consumers never creates a timer', () => {
    const tick = vi.fn()
    const owner = createPollingOwner(1000, tick)

    owner.configure({ enabled: true, intervalMs: 5000 })

    expect(owner.isRunning()).toBe(false)
    expect(owner.intervalMs()).toBe(5000)
    vi.advanceTimersByTime(10_000)
    expect(tick).not.toHaveBeenCalled()
  })

  it('acquire after disable with existing lease still yields one timer', () => {
    const tick = vi.fn()
    const owner = createPollingOwner(1000, tick)
    const releaseA = owner.acquire('a')

    owner.configure({ enabled: false })
    const releaseB = owner.acquire('b')

    expect(owner.isRunning()).toBe(false) // still disabled

    owner.configure({ enabled: true })
    expect(owner.isRunning()).toBe(true)
    expect(owner.consumerCount()).toBe(2)

    releaseA()
    expect(owner.isRunning()).toBe(true) // b still holds
    releaseB()
    expect(owner.isRunning()).toBe(false) // last lease gone, timer stops
  })

  it('disabling then releasing all consumers leaves no stale timer', () => {
    const tick = vi.fn()
    const owner = createPollingOwner(1000, tick)
    const release = owner.acquire('dashboard')

    owner.configure({ enabled: false })
    release()
    expect(owner.consumerCount()).toBe(0)

    owner.configure({ enabled: true })
    expect(owner.isRunning()).toBe(false) // nobody to poll for
    vi.advanceTimersByTime(10_000)
    expect(tick).not.toHaveBeenCalled()
  })

  it('release(id) is symmetric with acquire(id) and cannot underflow', () => {
    const tick = vi.fn()
    const owner = createPollingOwner(1000, tick)
    owner.acquire('docker-page')
    expect(owner.isRunning()).toBe(true)

    owner.release('docker-page')
    expect(owner.isRunning()).toBe(false)
    // Releasing an absent id is a no-op (same semantics as the paired closure).
    owner.release('docker-page')
    expect(owner.consumerCount()).toBe(0)
    expect(owner.isRunning()).toBe(false)
  })
})

/* -------------------------------------------------------------------------
 * Phase 10C correction — LIVE settings re-enable (spec §1–§3 of the final
 * correction): a mounted consumer must be able to disable and re-enable
 * polling WITHOUT remount, with exactly one timer in every state, and
 * manual refresh working while auto polling is off.
 * ---------------------------------------------------------------------- */
import {
  dockerPollingRunning,
  resetDockerPolling,
  startDockerPolling,
  stopDockerPolling,
  useDockerStore,
} from './dockerStore'
import { useSettingsStore } from './settingsStore'
import type { AppSettings } from '@/types/domain'

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

describe('live polling re-enable via settings (correction §1–§2)', () => {
  let mocks: ReturnType<typeof mockNativeClient>

  beforeEach(() => {
    vi.useFakeTimers()
    mocks = mockNativeClient()
    // Default save mock: echo the requested patch back as effective settings.
    mocks.saveAppSettings.mockImplementation(
      async (patch: Partial<AppSettings>) => ({
        settings: makeSettings(patch),
        correctedNote: null,
      }),
    )
    resetDockerPolling()
    // NOTE: do NOT resetPollingAppliers() here — the production appliers
    // (dockerStore, aiRuntimeStore, …) register at module-import time and
    // ARE the system under test. Wiping them would silently disconnect
    // settings from polling.
    useSettingsStore.setState({
      settings: null,
      loading: false,
      saving: false,
      error: null,
      correctedNote: null,
      loaded: false,
    })
    useDockerStore.setState({ snapshot: null, loading: false, refreshing: false, error: null, lastUpdated: null })
  })

  afterEach(() => {
    resetDockerPolling()
    restoreNativeClient()
    vi.useRealTimers()
  })

  it('Docker: mounted consumer + disable => zero timer; enable => exactly one timer, no remount', async () => {
    const { getDockerSnapshot } = await import('@/services/native/ports')
    vi.mocked(getDockerSnapshot).mockResolvedValue(undefined as unknown as Awaited<ReturnType<typeof getDockerSnapshot>>)

    // Page mounts.
    startDockerPolling()
    expect(dockerPollingRunning()).toBe(true)

    // Real settings load (appliers fire with the default, enabled).
    mocks.getAppSettings.mockResolvedValue(makeSettings())
    await useSettingsStore.getState().load()
    expect(dockerPollingRunning()).toBe(true)

    // Consumer REMAINS mounted; settings disable polling via a real save.
    await useSettingsStore.getState().save({ dockerPollingEnabled: false })
    expect(dockerPollingRunning()).toBe(false)

    // Consumer STILL mounted; re-enable resumes immediately (no remount).
    await useSettingsStore.getState().save({ dockerPollingEnabled: true })
    expect(dockerPollingRunning()).toBe(true)

    // Exactly one timer: next tick fires once per 5 s window.
    await vi.advanceTimersByTimeAsync(0)
    const calls = callCount(getDockerSnapshot)
    await vi.advanceTimersByTimeAsync(5_000)
    expect(callCount(getDockerSnapshot)).toBe(calls + 1)
    await vi.advanceTimersByTimeAsync(5_000)
    expect(callCount(getDockerSnapshot)).toBe(calls + 2)
  })

  it('Docker: the settings applier does not create a timer while unmounted', () => {
    // No page mounted; a re-enable must not conjure a polling loop.
    useSettingsStore.setState({ settings: makeSettings({ dockerPollingEnabled: false }), loaded: true })
    expect(dockerPollingRunning()).toBe(false)

    useSettingsStore.setState({ settings: makeSettings({ dockerPollingEnabled: true }), loaded: true })
    expect(dockerPollingRunning()).toBe(false)
    vi.advanceTimersByTime(10_000)
    expect(dockerPollingRunning()).toBe(false)
  })

  it('Docker: manual refresh still works while auto polling is disabled', async () => {
    const { refreshDocker } = await import('@/services/native/ports')
    vi.mocked(refreshDocker).mockResolvedValue(undefined as unknown as Awaited<ReturnType<typeof refreshDocker>>)

    startDockerPolling()
    mocks.getAppSettings.mockResolvedValue(makeSettings())
    await useSettingsStore.getState().load()
    await useSettingsStore.getState().save({ dockerPollingEnabled: false })
    expect(dockerPollingRunning()).toBe(false)

    await useDockerStore.getState().refresh() // manual — never the owner
    expect(refreshDocker).toHaveBeenCalledTimes(1)
    await vi.advanceTimersByTimeAsync(10_000)
    expect(refreshDocker).toHaveBeenCalledTimes(1) // no auto tick ever
  })

  it('Docker: start is idempotent and stop cannot underflow the single lease', () => {
    startDockerPolling()
    startDockerPolling() // same consumer id → still one lease
    expect(dockerPollingRunning()).toBe(true)

    stopDockerPolling()
    stopDockerPolling() // no-op, no underflow
    expect(dockerPollingRunning()).toBe(false)
  })

  it('AI: configured enabled=false stops and true resumes live for a mounted consumer', async () => {
    const {
      aiRuntimePollingRunning,
      resetAiRuntimePolling,
      startAiRuntimePolling,
    } = await import('./aiRuntimeStore')
    resetAiRuntimePolling()
    try {
      const release = startAiRuntimePolling()
      expect(aiRuntimePollingRunning()).toBe(true)

      // A real save fires the AI applier with enabled=false.
      mocks.getAppSettings.mockResolvedValue(makeSettings())
      await useSettingsStore.getState().load()
      await useSettingsStore.getState().save({ aiPollingEnabled: false })
      expect(aiRuntimePollingRunning()).toBe(false)

      // Same consumer still subscribed (lease kept); re-enable resumes.
      await useSettingsStore.getState().save({ aiPollingEnabled: true })
      expect(aiRuntimePollingRunning()).toBe(true)

      release()
      expect(aiRuntimePollingRunning()).toBe(false)
    } finally {
      resetAiRuntimePolling()
    }
  })
})
