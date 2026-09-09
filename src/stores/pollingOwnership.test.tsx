/**
 * Component-level polling ownership audit (Phase 10A correction, §6–7).
 *
 * Proves the structural pairing at the React boundary: effects return the
 * release closure directly, StrictMode's double mount/unmount cannot leak
 * or double-count a subscription, and unmount always releases.
 */
import { StrictMode } from 'react'
import { render, cleanup } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { mockNativeClient, restoreNativeClient } from '@/test/nativeMock'
import { WorkspacesPage } from '@/features/workspaces/WorkspacesPage'
import {
  conflictsPollingRunning,
  conflictsPollingSubscribers,
  resetConflictsPolling,
  subscribeConflictsPolling,
} from '@/stores/conflictsStore'
import {
  resetWorkspacePolling,
  workspacePollingRunning,
  workspacePollingSubscribers,
} from '@/stores/workspaceStore'

function HarnessComponent({ consumerId }: { consumerId: string }) {
  useSubscribeHarness(consumerId)
  return <div>harness {consumerId}</div>
}

import { useEffect } from 'react'

/** The exact page pattern: the release closure IS the effect cleanup. */
function useSubscribeHarness(consumerId: string): void {
  useEffect(() => subscribeConflictsPolling(consumerId), [consumerId])
}

describe('component-level polling ownership', () => {
  beforeEach(() => {
    mockNativeClient()
  })
  afterEach(() => {
    cleanup()
    resetConflictsPolling()
    resetWorkspacePolling()
    restoreNativeClient()
    vi.restoreAllMocks()
  })

  it('§6 mount acquires exactly once; unmount releases (no leaked subscriber)', () => {
    render(<HarnessComponent consumerId="probe" />)
    expect(conflictsPollingSubscribers()).toBe(1)
    expect(conflictsPollingRunning()).toBe(true)
    cleanup() // unmount
    expect(conflictsPollingSubscribers()).toBe(0)
    expect(conflictsPollingRunning()).toBe(false)
  })

  it('§7 StrictMode double mount/unmount does not leak or double-count', () => {
    render(
      <StrictMode>
        <HarnessComponent consumerId="strict" />
      </StrictMode>,
    )
    // React StrictMode runs effect → cleanup → effect in development.
    // Idempotent acquire per consumer id keeps ONE logical subscription.
    expect(conflictsPollingSubscribers()).toBe(1)
    expect(conflictsPollingRunning()).toBe(true)
    cleanup()
    expect(conflictsPollingSubscribers()).toBe(0)
    expect(conflictsPollingRunning()).toBe(false)
  })

  it('§7 unmount → remount restarts the timer (no permanent leak)', () => {
    const first = render(<HarnessComponent consumerId="cycle" />)
    first.unmount()
    expect(conflictsPollingRunning()).toBe(false)
    const second = render(<HarnessComponent consumerId="cycle" />)
    expect(conflictsPollingSubscribers()).toBe(1)
    expect(conflictsPollingRunning()).toBe(true)
    second.unmount()
    expect(conflictsPollingRunning()).toBe(false)
  })

  it('§2/§3/§4 two pages share one timer; first unmount keeps it for the second', () => {
    const dashboard = render(<HarnessComponent consumerId="dashboard" />)
    render(<HarnessComponent consumerId="services" />)
    expect(conflictsPollingSubscribers()).toBe(2)
    expect(conflictsPollingRunning()).toBe(true)
    dashboard.unmount() // dashboard unmounts first
    expect(conflictsPollingSubscribers()).toBe(1)
    expect(conflictsPollingRunning()).toBe(true) // services still subscribed
    cleanup() // services unmounts
    expect(conflictsPollingRunning()).toBe(false)
  })

  it('a real page (Workspaces) cleans up both of its subscriptions on unmount', async () => {
    const { listWorkspaces, getWorkspacesReadiness, getPortListeners } = await import(
      '@/services/native/ports'
    )
    vi.mocked(listWorkspaces).mockResolvedValue([])
    vi.mocked(getWorkspacesReadiness).mockResolvedValue([])
    vi.mocked(getPortListeners).mockResolvedValue({
      listeners: [],
      processes: [],
      services: [],
      projects: [],
      projectLinks: [],
      controls: [],
      lastUpdated: 0,
      durationMs: 0,
    })
    const view = render(<WorkspacesPage />)
    // Page subscribes to workspace + conflicts polling.
    expect(workspacePollingSubscribers()).toBe(1)
    expect(conflictsPollingSubscribers()).toBe(1)
    expect(workspacePollingRunning()).toBe(true)
    expect(conflictsPollingRunning()).toBe(true)
    view.unmount()
    expect(workspacePollingSubscribers()).toBe(0)
    expect(conflictsPollingSubscribers()).toBe(0)
    expect(workspacePollingRunning()).toBe(false)
    expect(conflictsPollingRunning()).toBe(false)
  })
})
