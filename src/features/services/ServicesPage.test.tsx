/**
 * Services page — lifecycle badges + identity presentation (Phase 10A, §31).
 */
import { render, screen, cleanup } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { mockNativeClient, restoreNativeClient } from '@/test/nativeMock'
import { ServicesPage } from '@/features/services/ServicesPage'
import {
  makeSnapshot,
  makeDockerSnapshot,
  makeWorkspace,
} from '@/test/fixtures'
import { usePortsStore } from '@/stores/portsStore'
import { useDockerStore } from '@/stores/dockerStore'
import { resetWorkspacePolling, useWorkspaceStore } from '@/stores/workspaceStore'
import { resetAiRuntimePolling, useAiRuntimeStore } from '@/stores/aiRuntimeStore'

describe('ServicesPage', () => {
  beforeEach(async () => {
    mockNativeClient()
    const { getPortListeners, getDockerSnapshot, listWorkspaces, getAiRuntimes } =
      await import('@/services/native/ports')
    const snapshot = makeSnapshot()
    vi.mocked(getPortListeners).mockResolvedValue(snapshot)
    vi.mocked(getDockerSnapshot).mockResolvedValue(makeDockerSnapshot())
    vi.mocked(listWorkspaces).mockResolvedValue([])
    vi.mocked(getAiRuntimes).mockResolvedValue([])
    // Pre-seed the discovery store; the page's mount loads refresh in place.
    usePortsStore.setState({
      listeners: snapshot.listeners,
      processByPid: new Map(snapshot.processes.map((p) => [p.pid, p])),
      serviceByPid: new Map(snapshot.services.map((s) => [s.pid, s])),
      projects: snapshot.projects,
      projectByPid: new Map([[4212, snapshot.projects[0]]]),
      controlByPid: new Map(snapshot.controls.map((c) => [c.pid, c])),
      loading: false,
      error: null,
      lastUpdated: snapshot.lastUpdated,
    })
  })
  afterEach(() => {
    cleanup()
    resetWorkspacePolling()
    resetAiRuntimePolling()
    restoreNativeClient()
    usePortsStore.setState({
      listeners: [],
      processByPid: new Map(),
      serviceByPid: new Map(),
      projects: [],
      projectByPid: new Map(),
      controlByPid: new Map(),
      loading: false,
      error: null,
      lastUpdated: null,
    })
    useWorkspaceStore.setState({ workspaces: [], loading: false })
    useAiRuntimeStore.setState({ runtimes: [], loading: false })
    useDockerStore.setState({ snapshot: null, loading: false })
    vi.restoreAllMocks()
  })

  it('shows an identified service with confidence and category badges', async () => {
    render(<ServicesPage />)
    expect(await screen.findByText('Vite')).toBeInTheDocument()
    expect(screen.getByText('High')).toBeInTheDocument()
    expect(screen.getByText('frontend')).toBeInTheDocument()
  })

  it('marks an unmanaged discovered process as EXTERNAL', async () => {
    render(<ServicesPage />)
    expect(await screen.findByText('EXTERNAL')).toBeInTheDocument()
    expect(screen.queryByText('MANAGED')).not.toBeInTheDocument()
  })

  it('marks a managed root PID as MANAGED (workspace knowledge wins)', async () => {
    const { listWorkspaces } = await import('@/services/native/ports')
    vi.mocked(listWorkspaces).mockResolvedValue([
      makeWorkspace({
        status: 'running',
        services: [
          {
            id: 'svc-1',
            name: 'Vite Dev Server',
            role: 'frontend',
            expectedPort: 3000,
            launchSpecId: 'spec-1',
            source: 'package.json scripts.dev',
            managed: { managedId: 'managed-1', rootPid: 4212, state: { state: 'running' }, startedAt: 0 },
          },
        ],
      }),
    ])
    render(<ServicesPage />)
    expect(await screen.findByText('MANAGED')).toBeInTheDocument()
    expect(screen.queryByText('EXTERNAL')).not.toBeInTheDocument()
  })

  it('marks a process whose port is Docker-published as CONTAINER', async () => {
    render(<ServicesPage />)
    // Fixture's container publishes host port 3000 — the same port PID 4212 owns.
    expect(await screen.findByText('CONTAINER')).toBeInTheDocument()
  })

  it('shows the empty state when nothing is running', async () => {
    const { getPortListeners } = await import('@/services/native/ports')
    vi.mocked(getPortListeners).mockResolvedValue(makeSnapshot({
      listeners: [], processes: [], services: [], projects: [], projectLinks: [], controls: [],
    }))
    usePortsStore.setState({
      listeners: [], processByPid: new Map(), serviceByPid: new Map(),
      projects: [], projectByPid: new Map(), controlByPid: new Map(), loading: false,
    })
    render(<ServicesPage />)
    expect(await screen.findByText('No active processes with listening ports.')).toBeInTheDocument()
  })

  it('shows the discovery error banner', async () => {
    const { getPortListeners } = await import('@/services/native/ports')
    vi.mocked(getPortListeners).mockRejectedValue(new Error('discovery down'))
    usePortsStore.setState({ loading: false, error: 'discovery down' })
    render(<ServicesPage />)
    expect(await screen.findByText('Discovery error')).toBeInTheDocument()
    expect(screen.getByText('discovery down')).toBeInTheDocument()
  })
})
