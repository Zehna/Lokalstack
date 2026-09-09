/**
 * Ports page — discovery table UX (Phase 10A, spec §30).
 */
import { render, screen, cleanup } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { mockNativeClient, restoreNativeClient } from '@/test/nativeMock'
import { PortsPage } from '@/features/ports/PortsPage'
import {
  makeSnapshot,
  makeDockerSnapshot,
  makeProcess,
  makeService,
  makeProject,
} from '@/test/fixtures'
import { usePortsStore } from '@/stores/portsStore'
import { useDockerStore } from '@/stores/dockerStore'

async function seedStore(overrides = {}): Promise<void> {
  const { getPortListeners, getDockerSnapshot } = await import('@/services/native/ports')
  vi.mocked(getPortListeners).mockResolvedValue(makeSnapshot(overrides))
  vi.mocked(getDockerSnapshot).mockResolvedValue(makeDockerSnapshot())
}

describe('PortsPage', () => {
  beforeEach(() => {
    mockNativeClient()
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
      durationMs: null,
      loading: false,
      refreshing: false,
      error: null,
      lastUpdated: null,
    })
    useDockerStore.setState({ snapshot: null, loading: false, refreshing: false, error: null, lastUpdated: null })
    vi.restoreAllMocks()
  })

  it('shows the loading state', () => {
    usePortsStore.setState({ loading: true })
    render(<PortsPage />)
    expect(screen.getByText('Reading TCP listener tables…')).toBeInTheDocument()
  })

  it('shows the empty state when nothing is listening (normal, not an error)', async () => {
    const { getPortListeners } = await import('@/services/native/ports')
    vi.mocked(getPortListeners).mockResolvedValue(makeSnapshot({
      listeners: [], processes: [], services: [], projects: [], projectLinks: [], controls: [],
    }))
    render(<PortsPage />)
    expect(await screen.findByText('No TCP listeners found on this machine.')).toBeInTheDocument()
  })

  it('renders a listener row with process, service and project metadata', async () => {
    await seedStore()
    render(<PortsPage />)
    expect(await screen.findByText('Vite')).toBeInTheDocument()
    expect(screen.getByText('node.exe')).toBeInTheDocument()
    expect(screen.getByText('historyai')).toBeInTheDocument()
    // PID appears inside the merged PID label cell.
    expect(screen.getByText(/4212/)).toBeInTheDocument()
  })

  it('renders the Docker container ownership overlay beside the PID', async () => {
    await seedStore()
    render(<PortsPage />)
    // The overlay shows the container name for the published host port 3000.
    expect(await screen.findByTitle(/published by Docker container historyai-frontend-1/)).toBeInTheDocument()
  })

  it('search filters listener rows', async () => {
    await seedStore()
    render(<PortsPage />)
    const input = await screen.findByPlaceholderText(/Search port, service, project/)
    await userEvent.setup().type(input, '9999')
    expect(screen.getByText('No listeners match “9999”.')).toBeInTheDocument()
  })

  it('shows the discovery error banner on a failed cycle', async () => {
    const { getPortListeners, getDockerSnapshot } = await import('@/services/native/ports')
    vi.mocked(getPortListeners).mockRejectedValue(new Error('backend unavailable'))
    vi.mocked(getDockerSnapshot).mockResolvedValue(makeDockerSnapshot())
    render(<PortsPage />)
    expect(await screen.findByText('Discovery error')).toBeInTheDocument()
    expect(screen.getByText('backend unavailable')).toBeInTheDocument()
  })

  it('End Process is offered for an eligible dev process on its row (compact label)', async () => {
    await seedStore()
    render(<PortsPage />)
    // Compact row layout uses the short "End" label — still behind the
    // two-click confirmation in ControlActions.
    expect(await screen.findByRole('button', { name: 'End' })).toBeInTheDocument()
  })

  it('import sanity: fixtures build', () => {
    expect(makeProcess).toBeDefined()
    expect(makeService).toBeDefined()
    expect(makeProject).toBeDefined()
  })
})
