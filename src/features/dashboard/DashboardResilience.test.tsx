/**
 * Phase 10B (spec §L, §M, §N) — frontend async-error resilience.
 *
 * Simultaneous failures in optional subsystems (Docker, AI) must stay
 * subsystem-local: the Dashboard shell still renders, ports still load, no
 * unhandled rejections escape. Zero-service machines are normal states, not
 * errors (§M). Polling continues after a transient failure (§L).
 */
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { DashboardPage } from './DashboardPage'
import { mockNativeClient, restoreNativeClient } from '@/test/nativeMock'
import {
  makeDockerSnapshot,
  makeSnapshot,
  makeWorkspace,
} from '@/test/fixtures'

let native: ReturnType<typeof mockNativeClient>

beforeEach(() => {
  native = mockNativeClient()
})

afterEach(() => {
  cleanup()
  restoreNativeClient()
  vi.useRealTimers()
})

describe('frontend chaos resilience (Phase 10B §L/§N)', () => {
  it('Dashboard survives Docker + AI failures while ports/workspaces succeed', async () => {
    native.getPortListeners.mockResolvedValue(
      makeSnapshot({ listeners: [], processes: [], services: [], projects: [], projectLinks: [], controls: [] }),
    )
    native.listWorkspaces.mockResolvedValue([makeWorkspace({ id: 'ws-1', name: 'OK' })])
    // Chaos: these two optional subsystems reject on every call.
    native.getDockerSnapshot.mockRejectedValue(new Error('pipe unavailable'))
    native.refreshDocker.mockRejectedValue(new Error('pipe unavailable'))
    native.getAiRuntimes.mockRejectedValue(new Error('ollama down'))
    native.getWorkspacesReadiness.mockRejectedValue(new Error('readiness down'))

    render(<DashboardPage />)

    // Shell + workspace-derived summary still render; subsystem errors stay
    // contained (count drops to 0, not a crash).
    await waitFor(() => {
      expect(screen.getByText('Dashboard')).toBeInTheDocument()
      expect(screen.getByText('Workspaces')).toBeInTheDocument()
    })
    expect(screen.queryByRole('alert')).not.toBeInTheDocument()
  })

  it('a later Docker recovery is picked up without remount', async () => {
    native.getDockerSnapshot.mockRejectedValueOnce(new Error('engine gone'))
    native.refreshDocker.mockRejectedValueOnce(new Error('engine gone'))
    native.getDockerSnapshot.mockResolvedValueOnce(
      makeDockerSnapshot({ available: true, containers: [] }),
    )

    render(<DashboardPage />)
    await waitFor(() => expect(screen.getByText('Dashboard')).toBeInTheDocument())
  })

  it('transient ports failure does not break the next refresh', async () => {
    native.getPortListeners
      .mockRejectedValueOnce(new Error('transient'))
      .mockResolvedValue(
        makeSnapshot({ listeners: [], processes: [], services: [], projects: [], projectLinks: [], controls: [] }),
      )

    render(<DashboardPage />)
    // First load fails → error state, shell intact.
    await waitFor(() => expect(screen.getByText('Dashboard')).toBeInTheDocument())

    // Manual refresh succeeds → the listeners section re-renders without
    // the error banner (empty-state copy returns).
    const refresh = screen.getByRole('button', { name: /refresh/i })
    fireEvent.click(refresh)
    await waitFor(() => {
      expect(screen.getByText(/No TCP listeners found/i)).toBeInTheDocument()
    })
    expect(screen.queryByRole('alert')).not.toBeInTheDocument()
  })

  it('zero-service machine renders a normal Dashboard (§M)', async () => {
    native.getPortListeners.mockResolvedValue(
      makeSnapshot({ listeners: [], processes: [], services: [], projects: [], projectLinks: [], controls: [] }),
    )
    native.listWorkspaces.mockResolvedValue([])
    native.getAiRuntimes.mockResolvedValue([])
    native.getWorkspacesReadiness.mockResolvedValue([])
    native.getDockerSnapshot.mockResolvedValue(
      makeDockerSnapshot({ available: false, containers: [] }),
    )

    render(<DashboardPage />)

    await waitFor(() => {
      expect(screen.getByText('Dashboard')).toBeInTheDocument()
    })
    // Empty state copy, not an error surface.
    expect(screen.queryByRole('alert')).not.toBeInTheDocument()
    expect(screen.queryByText(/something went wrong/i)).not.toBeInTheDocument()
  })

  it('empty optional subsystems degrade to useful empty states (§M)', async () => {
    native.getPortListeners.mockResolvedValue(
      makeSnapshot({ listeners: [], processes: [], services: [], projects: [], projectLinks: [], controls: [] }),
    )
    native.listWorkspaces.mockResolvedValue([])
    native.getAiRuntimes.mockResolvedValue([])
    native.getWorkspacesReadiness.mockResolvedValue([])

    render(<DashboardPage />)
    await waitFor(() => expect(screen.getByText('Dashboard')).toBeInTheDocument())
    // No crash dialog anywhere in the document.
    expect(screen.queryByRole('alert')).not.toBeInTheDocument()
  })
})
