/**
 * Projects page — evidence-based project association UX (Phase 10A, §28).
 */
import { render, screen, cleanup } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { mockNativeClient, restoreNativeClient } from '@/test/nativeMock'
import { ProjectsPage } from '@/features/projects/ProjectsPage'
import { makeSnapshot, makeDockerSnapshot } from '@/test/fixtures'
import { usePortsStore } from '@/stores/portsStore'
import { useDockerStore } from '@/stores/dockerStore'

describe('ProjectsPage', () => {
  beforeEach(async () => {
    mockNativeClient()
    const { getPortListeners, getDockerSnapshot } = await import('@/services/native/ports')
    const snapshot = makeSnapshot()
    vi.mocked(getPortListeners).mockResolvedValue(snapshot)
    vi.mocked(getDockerSnapshot).mockResolvedValue(makeDockerSnapshot())
    usePortsStore.setState({
      listeners: snapshot.listeners,
      processByPid: new Map(snapshot.processes.map((p) => [p.pid, p])),
      serviceByPid: new Map(snapshot.services.map((s) => [s.pid, s])),
      projects: snapshot.projects,
      projectByPid: new Map([[4212, snapshot.projects[0]]]),
      controlByPid: new Map(),
      loading: false,
      error: null,
      lastUpdated: snapshot.lastUpdated,
    })
  })
  afterEach(() => {
    cleanup()
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
    useDockerStore.setState({ snapshot: null, loading: false })
    vi.restoreAllMocks()
  })

  it('shows the loading state', () => {
    usePortsStore.setState({ loading: true })
    render(<ProjectsPage />)
    expect(screen.getByText('Resolving projects from running processes…')).toBeInTheDocument()
  })

  it('shows the empty state when nothing is running', async () => {
    const { getPortListeners } = await import('@/services/native/ports')
    vi.mocked(getPortListeners).mockResolvedValue(makeSnapshot({
      listeners: [], processes: [], services: [], projects: [], projectLinks: [], controls: [],
    }))
    usePortsStore.setState({
      listeners: [], processByPid: new Map(), serviceByPid: new Map(),
      projects: [], projectByPid: new Map(), loading: false,
    })
    render(<ProjectsPage />)
    expect(await screen.findByText('No running processes to associate yet.')).toBeInTheDocument()
  })

  it('renders a resolved project with its evidence-backed identity', async () => {
    render(<ProjectsPage />)
    expect(await screen.findByText('historyai')).toBeInTheDocument()
  })

  it('shows the discovery error banner', async () => {
    const { getPortListeners } = await import('@/services/native/ports')
    vi.mocked(getPortListeners).mockRejectedValue(new Error('snapshot failed'))
    usePortsStore.setState({ loading: false, error: 'snapshot failed' })
    render(<ProjectsPage />)
    expect(await screen.findByText('snapshot failed')).toBeInTheDocument()
  })
})
