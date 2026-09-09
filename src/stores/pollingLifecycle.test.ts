/**
 * Polling lifecycle regression matrix (Phase 10A, spec §17–18).
 *
 * For every store-owned poller: start is idempotent, stop is idempotent,
 * restart creates exactly one timer, ticks fire the expected number of
 * refreshes, and no timer survives cleanup. Fake timers — no real sleeping.
 */
import { afterEach, beforeEach, describe, expect, it, vi, type Mock } from 'vitest'

import { mockNativeClient, restoreNativeClient } from '@/test/nativeMock'
import {
  conflictsPollingRunning,
  conflictsPollingSubscribers,
  resetConflictsPolling,
  startConflictsPolling,
  subscribeConflictsPolling,
} from '@/stores/conflictsStore'
import {
  aiRuntimePollingSubscribers,
  resetAiRuntimePolling,
  startAiRuntimePolling,
  subscribeAiRuntimePolling,
} from '@/stores/aiRuntimeStore'
import {
  startDockerPolling,
  stopDockerPolling,
  useDockerStore,
} from '@/stores/dockerStore'
import {
  resetWorkspacePolling,
  startWorkspacePolling,
  subscribeWorkspacePolling,
  workspacePollingRunning,
  workspacePollingSubscribers,
} from '@/stores/workspaceStore'

/** Intervals from the stores (ms). */
const CONFLICTS_MS = 2_000
const WORKSPACE_MS = 2_000
const AI_MS = 10_000
const DOCKER_MS = 5_000

/** Typed view of a mocked client function's call log. */
function callCount(fn: unknown): number {
  return (fn as unknown as { mock: Mock['mock'] }).mock.calls.length
}

describe('polling lifecycle matrix', () => {
  beforeEach(() => {
    vi.useFakeTimers()
    mockNativeClient()
  })
  afterEach(() => {
    // Test-only teardown: drops every consumer and clears the timer.
    resetConflictsPolling()
    resetWorkspacePolling()
    resetAiRuntimePolling()
    stopDockerPolling()
    restoreNativeClient()
    vi.useRealTimers()
  })

  describe('conflictsStore polling', () => {
    it('subscribe creates one consumer and one timer', () => {
      const release = subscribeConflictsPolling('dashboard')
      expect(conflictsPollingSubscribers()).toBe(1)
      expect(conflictsPollingRunning()).toBe(true)
      release()
      expect(conflictsPollingSubscribers()).toBe(0)
      expect(conflictsPollingRunning()).toBe(false)
    })

    it('duplicate subscribe from the same consumer is ONE subscription (§1)', () => {
      const releaseA = subscribeConflictsPolling('dashboard')
      const releaseB = subscribeConflictsPolling('dashboard') // accidental double-acquire
      expect(conflictsPollingSubscribers()).toBe(1)
      expect(conflictsPollingRunning()).toBe(true)
      // Both closures refer to the same single logical subscription: the
      // first release ends it, the second is a safe no-op (no underflow).
      releaseB()
      expect(conflictsPollingSubscribers()).toBe(0)
      expect(conflictsPollingRunning()).toBe(false)
      releaseA()
      expect(conflictsPollingSubscribers()).toBe(0)
      expect(conflictsPollingRunning()).toBe(false)
    })

    it('release closures are structurally paired and idempotent (§5)', () => {
      const release = subscribeConflictsPolling('dashboard')
      release()
      release()
      release()
      expect(conflictsPollingSubscribers()).toBe(0)
      expect(conflictsPollingRunning()).toBe(false)
    })

    it('ticks fire refreshes at the configured interval', async () => {
      const { getWorkspacesReadiness } = await import('@/services/native/ports')
      vi.mocked(getWorkspacesReadiness).mockResolvedValue([])
      const release = startConflictsPolling()
      // Flush the immediate refresh's microtasks so its in-flight guard
      // releases before we count ticks.
      await vi.advanceTimersByTimeAsync(0)
      const callsAfterStart = callCount(getWorkspacesReadiness)
      await vi.advanceTimersByTimeAsync(CONFLICTS_MS)
      expect(callCount(getWorkspacesReadiness)).toBe(callsAfterStart + 1)
      await vi.advanceTimersByTimeAsync(CONFLICTS_MS * 2)
      expect(callCount(getWorkspacesReadiness)).toBe(callsAfterStart + 3)
      release()
    })

    it('stop halts ticks; duplicate release is a no-op', async () => {
      const { getWorkspacesReadiness } = await import('@/services/native/ports')
      vi.mocked(getWorkspacesReadiness).mockResolvedValue([])
      const release = startConflictsPolling()
      await vi.advanceTimersByTimeAsync(0)
      release()
      release()
      release()
      await vi.advanceTimersByTimeAsync(0)
      const calls = callCount(getWorkspacesReadiness)
      await vi.advanceTimersByTimeAsync(CONFLICTS_MS * 3)
      expect(callCount(getWorkspacesReadiness)).toBe(calls)
      expect(conflictsPollingSubscribers()).toBe(0)
    })

    it('restart after release creates exactly one new timer', async () => {
      const { getWorkspacesReadiness } = await import('@/services/native/ports')
      vi.mocked(getWorkspacesReadiness).mockResolvedValue([])
      const release = startConflictsPolling()
      release()
      const release2 = startConflictsPolling()
      await vi.advanceTimersByTimeAsync(0)
      const calls = callCount(getWorkspacesReadiness)
      await vi.advanceTimersByTimeAsync(CONFLICTS_MS)
      expect(callCount(getWorkspacesReadiness)).toBe(calls + 1)
      expect(conflictsPollingSubscribers()).toBe(1)
      release2()
    })

    it('full cleanup leaves zero subscribers and no timer', () => {
      const release = startConflictsPolling()
      release()
      expect(conflictsPollingSubscribers()).toBe(0)
      expect(conflictsPollingRunning()).toBe(false)
    })
  })

  describe('workspaceStore polling', () => {
    it('subscribe/duplicate-subscribe from distinct consumers', () => {
      const r1 = subscribeWorkspacePolling('dashboard')
      const r2 = subscribeWorkspacePolling('services')
      expect(workspacePollingSubscribers()).toBe(2)
      expect(workspacePollingRunning()).toBe(true)
      r1()
      r2()
      expect(workspacePollingRunning()).toBe(false)
    })

    it('ticks fire refreshes; release halts; duplicate release safe', async () => {
      const { listWorkspaces } = await import('@/services/native/ports')
      const release = startWorkspacePolling()
      await vi.advanceTimersByTimeAsync(0)
      const calls = callCount(listWorkspaces)
      await vi.advanceTimersByTimeAsync(WORKSPACE_MS)
      expect(callCount(listWorkspaces)).toBe(calls + 1)
      release()
      release()
      release()
      await vi.advanceTimersByTimeAsync(WORKSPACE_MS * 2)
      expect(callCount(listWorkspaces)).toBe(calls + 1)
      expect(workspacePollingSubscribers()).toBe(0)
      expect(workspacePollingRunning()).toBe(false)
    })
  })

  describe('aiRuntimeStore polling', () => {
    it('subscribe/duplicate-subscribe from distinct consumers', () => {
      const r1 = subscribeAiRuntimePolling('dashboard')
      const r2 = subscribeAiRuntimePolling('ai-services')
      expect(aiRuntimePollingSubscribers()).toBe(2)
      r1()
      r2()
      expect(aiRuntimePollingSubscribers()).toBe(0)
    })

    it('ticks fire at 10 s cadence; release halts; duplicate release safe', async () => {
      const { getAiRuntimes } = await import('@/services/native/ports')
      const release = startAiRuntimePolling()
      await vi.advanceTimersByTimeAsync(0)
      const calls = callCount(getAiRuntimes)
      await vi.advanceTimersByTimeAsync(AI_MS)
      expect(callCount(getAiRuntimes)).toBe(calls + 1)
      // A 3 s advance must not tick (interval is 10 s, not 3 s).
      await vi.advanceTimersByTimeAsync(3_000)
      expect(callCount(getAiRuntimes)).toBe(calls + 1)
      release()
      release()
      await vi.advanceTimersByTimeAsync(AI_MS)
      expect(callCount(getAiRuntimes)).toBe(calls + 1)
      expect(aiRuntimePollingSubscribers()).toBe(0)
    })
  })

  describe('dockerStore polling', () => {
    it('start/duplicate-start/stop/stop is safe and single', async () => {
      const { getDockerSnapshot } = await import('@/services/native/ports')
      startDockerPolling()
      startDockerPolling()
      await vi.advanceTimersByTimeAsync(0)
      const calls = callCount(getDockerSnapshot)
      await vi.advanceTimersByTimeAsync(DOCKER_MS)
      expect(callCount(getDockerSnapshot)).toBe(calls + 1)
      stopDockerPolling()
      stopDockerPolling()
      stopDockerPolling()
      await vi.advanceTimersByTimeAsync(DOCKER_MS * 2)
      expect(callCount(getDockerSnapshot)).toBe(calls + 1)
      expect(useDockerStore.getState().refreshing).toBe(false)
    })
  })

  describe('shared-store ownership (named subscriptions)', () => {
    it('two real consumers share one timer (§2)', () => {
      const releaseDashboard = subscribeConflictsPolling('dashboard')
      const releaseServices = subscribeConflictsPolling('services')
      expect(conflictsPollingSubscribers()).toBe(2)
      expect(conflictsPollingRunning()).toBe(true)
      releaseDashboard()
      releaseServices()
    })

    it('dashboard unmounting keeps the timer alive for services (§3)', async () => {
      const { getWorkspacesReadiness } = await import('@/services/native/ports')
      vi.mocked(getWorkspacesReadiness).mockResolvedValue([])
      const releaseDashboard = subscribeConflictsPolling('dashboard')
      const releaseServices = subscribeConflictsPolling('services')
      releaseDashboard() // dashboard unmounts first
      await vi.advanceTimersByTimeAsync(0)
      const calls = callCount(getWorkspacesReadiness)
      await vi.advanceTimersByTimeAsync(CONFLICTS_MS)
      // Services still holds a subscription → ticks continue.
      expect(callCount(getWorkspacesReadiness)).toBe(calls + 1)
      expect(conflictsPollingRunning()).toBe(true)
      releaseServices() // last unmount → timer dies (§4)
      await vi.advanceTimersByTimeAsync(CONFLICTS_MS * 2)
      expect(callCount(getWorkspacesReadiness)).toBe(calls + 1)
      expect(conflictsPollingRunning()).toBe(false)
    })

    it('a stale release closure from an earlier generation cannot kill a newer subscription (§5)', () => {
      const staleRelease = subscribeConflictsPolling('dashboard')
      staleRelease()
      const releaseServices = subscribeConflictsPolling('services')
      // The stale closure fires again (late cleanup, double-invoked effect…).
      staleRelease()
      // Services must still own its subscription and the timer must run.
      expect(conflictsPollingSubscribers()).toBe(1)
      expect(conflictsPollingRunning()).toBe(true)
      releaseServices()
      expect(conflictsPollingRunning()).toBe(false)
    })
  })
})
