/**
 * Workspaces page — managed lifecycle UX (Phase 10A, spec §29).
 * Key safety assertions: confirmations gate destructive actions, conflict
 * dialog shows owner + advisory ports and never auto-applies anything.
 */
import { render, screen, cleanup } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { mockNativeClient, restoreNativeClient } from '@/test/nativeMock'
import { WorkspacesPage } from '@/features/workspaces/WorkspacesPage'
import { makeConflictReport, makeReadiness, makeWorkspace } from '@/test/fixtures'
import type { WorkspaceReadinessView } from '@/types/domain'
import {
  resetConflictsPolling,
  resetConflictsTransitions,
  useConflictsStore,
} from '@/stores/conflictsStore'
import { resetWorkspacePolling, useWorkspaceStore } from '@/stores/workspaceStore'
import { usePortsStore } from '@/stores/portsStore'

function resetStores(): void {
  useWorkspaceStore.setState({
    workspaces: [],
    loading: true,
    refreshing: false,
    error: null,
    lastUpdated: null,
    openLogManagedId: null,
    logLines: [],
    logCursor: 0,
  })
  useConflictsStore.setState({
    readiness: [],
    loading: true,
    refreshing: false,
    error: null,
    lastUpdated: null,
    lastPortReport: null,
    portSuggestions: [],
  })
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
}

describe('WorkspacesPage', () => {
  beforeEach(async () => {
    mockNativeClient()
    resetConflictsTransitions()
    // The page's mount effect loads everything — give the mocks real empty
    // results so async resolution cannot clobber the store under test.
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
  })
  afterEach(() => {
    cleanup()
    resetWorkspacePolling()
    resetConflictsPolling()
    restoreNativeClient()
    resetStores()
    vi.restoreAllMocks()
  })

  /** Seed state through the mocked commands — the page's own mount load()
   * resolves and overwrites, so mocks are the single source of truth. */
  async function seed(
    workspace: ReturnType<typeof makeWorkspace> | null,
    readiness: WorkspaceReadinessView[] = [],
  ): Promise<void> {
    const { listWorkspaces, getWorkspacesReadiness } = await import('@/services/native/ports')
    vi.mocked(listWorkspaces).mockResolvedValue(workspace === null ? [] : [workspace])
    vi.mocked(getWorkspacesReadiness).mockResolvedValue(readiness)
  }

  it('shows the loading state before the first snapshot resolves', () => {
    resetStores()
    render(<WorkspacesPage />)
    expect(screen.getByText('Loading workspaces…')).toBeInTheDocument()
  })

  it('shows the empty state when no workspace exists', async () => {
    await seed(null)
    render(<WorkspacesPage />)
    expect(await screen.findByText('No workspaces yet.')).toBeInTheDocument()
  })

  it('renders a running workspace with its managed service and lifecycle buttons', async () => {
    const managedWorkspace = makeWorkspace({
      status: 'running',
      services: [
        {
          id: 'svc-1',
          name: 'Vite Dev Server',
          role: 'frontend',
          expectedPort: 3000,
          launchSpecId: 'spec-1',
          source: 'package.json scripts.dev',
          managed: { managedId: 'managed-1', rootPid: 5000, state: { state: 'running' }, startedAt: 0 },
        },
      ],
    })
    await seed(managedWorkspace, [makeReadiness()])
    render(<WorkspacesPage />)
    expect(await screen.findByText('PID 5000')).toBeInTheDocument()
    expect(screen.getAllByText('running').length).toBeGreaterThan(0)
    expect(screen.getByRole('button', { name: 'Restart' })).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Stop' })).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Logs' })).toBeInTheDocument()
    // Workspace-level actions.
    expect(screen.getByRole('button', { name: 'Start workspace' })).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Stop managed' })).toBeInTheDocument()
  })

  it('renders a partial workspace with the honest status', async () => {
    await seed(makeWorkspace({ status: 'partial' }))
    render(<WorkspacesPage />)
    expect(await screen.findByText('partial')).toBeInTheDocument()
  })

  it('shows dependency issues from readiness as blocking information', async () => {
    await seed(
      makeWorkspace({ status: 'error' }),
      [
        makeReadiness({
          status: 'blocked',
          issues: [
            {
              code: 'DEPENDENCY_UNAVAILABLE',
              severity: 'error',
              title: 'PostgreSQL unreachable',
              message: 'PostgreSQL on port 5432 is not listening.',
              dependencyId: 'dep-1',
            },
          ],
        }),
      ],
    )
    render(<WorkspacesPage />)
    expect(await screen.findByText('PostgreSQL on port 5432 is not listening.')).toBeInTheDocument()
    // Workspace-level status (WorkspaceStatus has no 'blocked'; the readiness
    // view carries it) — an error workspace badge accompanies the issue.
    expect(screen.getByText('error')).toBeInTheDocument()
  })

  it('conflict dialog shows owner lifecycle and advisory alternatives — and no auto-apply', async () => {
    await seed(makeWorkspace())
    render(<WorkspacesPage />)
    // Open the conflict dialog through the store (dialog is driven by lastPortReport).
    useConflictsStore.setState({
      lastPortReport: makeConflictReport(),
      portSuggestions: [
        { port: 3001, status: 'available' },
        { port: 3002, status: 'available' },
      ],
    })
    const dialog = await screen.findByRole('dialog', { name: 'Port conflict' })
    expect(dialog).toBeInTheDocument()
    expect(screen.getByText('Port 3000 is owned by another project.')).toBeInTheDocument()
    expect(screen.getByText(/Lifecycle: external/)).toBeInTheDocument()
    expect(screen.getByText(':3001')).toBeInTheDocument()
    expect(screen.getByText(':3002')).toBeInTheDocument()
    expect(screen.getByText(/advisory only/i)).toBeInTheDocument()
    // No auto-apply: only Close exists — nothing like "Use port 3001".
    expect(screen.queryByRole('button', { name: /use port|apply|switch/i })).not.toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Close' })).toBeInTheDocument()
  })

  it('Start click on a stopped service calls startManagedService with the opaque spec id', async () => {
    const user = userEvent.setup()
    await seed(makeWorkspace({ status: 'stopped' }))
    const { startManagedService } = await import('@/services/native/ports')
    vi.mocked(startManagedService).mockResolvedValue({
      ok: true,
      code: 'OK',
      message: 'Started.',
    })
    render(<WorkspacesPage />)
    await user.click(await screen.findByRole('button', { name: /Start$/ }))
    expect(startManagedService).toHaveBeenCalledWith('spec-1')
  })

  it('Stop requires the two-click confirmation (first click only arms)', async () => {
    const user = userEvent.setup()
    const managedWorkspace = makeWorkspace({
      status: 'running',
      services: [
        {
          id: 'svc-1',
          name: 'Vite Dev Server',
          role: 'frontend',
          expectedPort: 3000,
          launchSpecId: 'spec-1',
          source: 'package.json scripts.dev',
          managed: { managedId: 'managed-1', rootPid: 5000, state: { state: 'running' }, startedAt: 0 },
        },
      ],
    })
    await seed(managedWorkspace)
    const { stopManagedService } = await import('@/services/native/ports')
    render(<WorkspacesPage />)
    await user.click(await screen.findByRole('button', { name: 'Stop' }))
    expect(stopManagedService).not.toHaveBeenCalled()
  })
})
