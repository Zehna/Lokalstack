import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { restoreNativeClient, mockNativeClient } from '@/test/nativeMock'
import { makeReadiness, makeConflictReport } from '@/test/fixtures'
import {
  resetConflictsPolling,
  resetConflictsTransitions,
  useConflictsStore,
} from '@/stores/conflictsStore'
import { useControlStore } from '@/stores/controlStore'

describe('conflictsStore', () => {
  beforeEach(() => {
    mockNativeClient()
    resetConflictsTransitions()
  })
  afterEach(() => {
    resetConflictsPolling()
  })
  afterEach(() => {
    restoreNativeClient()
    useConflictsStore.setState({
      readiness: [],
      loading: true,
      refreshing: false,
      error: null,
      lastUpdated: null,
      lastPortReport: null,
      portSuggestions: [],
    })
    useControlStore.setState({ history: [], pending: null })
  })

  it('loads readiness views', async () => {
    const { getWorkspacesReadiness } = await import('@/services/native/ports')
    vi.mocked(getWorkspacesReadiness).mockResolvedValue([makeReadiness()])
    await useConflictsStore.getState().load()
    expect(useConflictsStore.getState().readiness).toHaveLength(1)
    expect(useConflictsStore.getState().loading).toBe(false)
  })

  it('emits dependency_unavailable once, not per poll (transition dedup)', async () => {
    const { getWorkspacesReadiness } = await import('@/services/native/ports')
    const blocked = makeReadiness({
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
    })
    vi.mocked(getWorkspacesReadiness).mockResolvedValue([blocked])
    await useConflictsStore.getState().load()
    vi.mocked(getWorkspacesReadiness).mockResolvedValue([blocked])
    await useConflictsStore.getState().refresh()
    expect(
      useControlStore.getState().history.filter((e) => e.action === 'dependency_unavailable'),
    ).toHaveLength(1)
  })

  it('emits dependency_recovered exactly once when the dependency returns to available', async () => {
    const { getWorkspacesReadiness } = await import('@/services/native/ports')
    const blocked = makeReadiness({
      dependencies: [
        {
          id: 'dep-1',
          workspaceId: 'ws-1',
          sourceServiceId: 'svc-1',
          target: { type: 'tcp_port', port: 5432 },
          targetLabel: 'PostgreSQL :5432',
          required: true,
          state: 'unavailable',
        },
      ],
      issues: [
        {
          code: 'DEPENDENCY_UNAVAILABLE',
          severity: 'error',
          title: 'PostgreSQL unreachable',
          message: 'down',
          dependencyId: 'dep-1',
        },
      ],
    })
    const healthy = makeReadiness({
      dependencies: [
        {
          id: 'dep-1',
          workspaceId: 'ws-1',
          sourceServiceId: 'svc-1',
          target: { type: 'tcp_port', port: 5432 },
          targetLabel: 'PostgreSQL :5432',
          required: true,
          state: 'available',
        },
      ],
    })
    vi.mocked(getWorkspacesReadiness).mockResolvedValue([blocked])
    await useConflictsStore.getState().load()
    expect(
      useControlStore.getState().history.filter((e) => e.action === 'dependency_unavailable'),
    ).toHaveLength(1)
    // Dependency present and available → one recovery event.
    vi.mocked(getWorkspacesReadiness).mockResolvedValue([healthy])
    await useConflictsStore.getState().refresh()
    expect(
      useControlStore.getState().history.filter((e) => e.action === 'dependency_recovered'),
    ).toHaveLength(1)
    // Still available on the next poll → no repeat.
    vi.mocked(getWorkspacesReadiness).mockResolvedValue([healthy])
    await useConflictsStore.getState().refresh()
    expect(
      useControlStore.getState().history.filter((e) => e.action === 'dependency_recovered'),
    ).toHaveLength(1)
  })

  it('does not fabricate recovery when the workspace vanishes from readiness', async () => {
    const { getWorkspacesReadiness } = await import('@/services/native/ports')
    const blocked = makeReadiness({
      dependencies: [
        {
          id: 'dep-1',
          workspaceId: 'ws-1',
          sourceServiceId: 'svc-1',
          target: { type: 'tcp_port', port: 5432 },
          targetLabel: 'PostgreSQL :5432',
          required: true,
          state: 'unavailable',
        },
      ],
      issues: [
        {
          code: 'DEPENDENCY_UNAVAILABLE',
          severity: 'error',
          title: 'PostgreSQL unreachable',
          message: 'down',
          dependencyId: 'dep-1',
        },
      ],
    })
    vi.mocked(getWorkspacesReadiness).mockResolvedValue([blocked])
    await useConflictsStore.getState().load()
    // Workspace gone entirely: the unavailable set clears silently.
    vi.mocked(getWorkspacesReadiness).mockResolvedValue([])
    await useConflictsStore.getState().refresh()
    expect(
      useControlStore.getState().history.filter((e) => e.action === 'dependency_recovered'),
    ).toHaveLength(0)
  })

  it('evaluateConflict stores the report and suggestions', async () => {
    const { evaluatePort, findFreePorts } = await import('@/services/native/ports')
    vi.mocked(evaluatePort).mockResolvedValue(makeConflictReport())
    vi.mocked(findFreePorts).mockResolvedValue([
      { port: 3001, status: 'available' },
      { port: 3002, status: 'available' },
    ])
    await useConflictsStore.getState().evaluateConflict(3000)
    expect(useConflictsStore.getState().lastPortReport?.kind).toBe('other_project')
    expect(useConflictsStore.getState().portSuggestions).toHaveLength(2)
  })

  it('evaluateConflict records a conflict_detected transition for real conflicts', async () => {
    const { evaluatePort, findFreePorts } = await import('@/services/native/ports')
    vi.mocked(evaluatePort).mockResolvedValue(makeConflictReport())
    vi.mocked(findFreePorts).mockResolvedValue([])
    await useConflictsStore.getState().evaluateConflict(3000)
    expect(
      useControlStore.getState().history.filter((e) => e.action === 'conflict_detected'),
    ).toHaveLength(1)
  })

  it('no conflict_detected event for no_conflict reports', async () => {
    const { evaluatePort, findFreePorts } = await import('@/services/native/ports')
    vi.mocked(evaluatePort).mockResolvedValue(makeConflictReport({ kind: 'no_conflict', severity: 'info' }))
    vi.mocked(findFreePorts).mockResolvedValue([])
    await useConflictsStore.getState().evaluateConflict(4000)
    expect(
      useControlStore.getState().history.filter((e) => e.action === 'conflict_detected'),
    ).toHaveLength(0)
  })

  it('addDependency refreshes readiness after the mutation', async () => {
    const { addWorkspaceDependency, getWorkspacesReadiness } = await import('@/services/native/ports')
    vi.mocked(addWorkspaceDependency).mockResolvedValue(undefined)
    vi.mocked(getWorkspacesReadiness).mockResolvedValue([makeReadiness()])
    await useConflictsStore.getState().addDependency('ws-1', 'svc-1', { type: 'tcp_port', port: 5432 }, true)
    expect(addWorkspaceDependency).toHaveBeenCalledWith('ws-1', 'svc-1', { type: 'tcp_port', port: 5432 }, true)
    expect(useConflictsStore.getState().readiness).toHaveLength(1)
  })

  it('conflict detected → resolved emits exactly one of each; unchanged emits nothing', async () => {
    const { getWorkspacesReadiness } = await import('@/services/native/ports')
    const conflict = makeReadiness({
      status: 'conflict',
      conflicts: [makeConflictReport()],
    })
    vi.mocked(getWorkspacesReadiness).mockResolvedValue([conflict])
    await useConflictsStore.getState().load()
    // Same conflict on the next poll → nothing.
    vi.mocked(getWorkspacesReadiness).mockResolvedValue([conflict])
    await useConflictsStore.getState().refresh()
    const detected = useControlStore
      .getState()
      .history.filter((e) => e.action === 'conflict_detected')
    expect(detected).toHaveLength(1)
    // Resolved.
    vi.mocked(getWorkspacesReadiness).mockResolvedValue([makeReadiness()])
    await useConflictsStore.getState().refresh()
    const resolved = useControlStore
      .getState()
      .history.filter((e) => e.action === 'conflict_resolved')
    expect(resolved).toHaveLength(1)
    // No further events on a clean poll.
    await useConflictsStore.getState().refresh()
    expect(resolved).toHaveLength(1)
    expect(detected).toHaveLength(1)
  })

  it('workspace blocked → recovered emits one edge each; unchanged emits nothing', async () => {
    const { getWorkspacesReadiness } = await import('@/services/native/ports')
    const blocked = makeReadiness({ status: 'blocked' })
    vi.mocked(getWorkspacesReadiness).mockResolvedValue([makeReadiness()])
    await useConflictsStore.getState().load()
    // running → blocked: one event.
    vi.mocked(getWorkspacesReadiness).mockResolvedValue([blocked])
    await useConflictsStore.getState().refresh()
    expect(
      useControlStore.getState().history.filter((e) => e.action === 'workspace_blocked'),
    ).toHaveLength(1)
    // blocked → blocked: nothing.
    vi.mocked(getWorkspacesReadiness).mockResolvedValue([blocked])
    await useConflictsStore.getState().refresh()
    expect(
      useControlStore.getState().history.filter((e) => e.action === 'workspace_blocked'),
    ).toHaveLength(1)
    // blocked → running: one recovery.
    vi.mocked(getWorkspacesReadiness).mockResolvedValue([makeReadiness()])
    await useConflictsStore.getState().refresh()
    expect(
      useControlStore.getState().history.filter((e) => e.action === 'workspace_recovered'),
    ).toHaveLength(1)
    // running → running: nothing more.
    await useConflictsStore.getState().refresh()
    expect(
      useControlStore.getState().history.filter((e) => e.action === 'workspace_recovered'),
    ).toHaveLength(1)
  })

  it('first sighting of a blocked workspace does not retro-emit blocked', async () => {
    const { getWorkspacesReadiness } = await import('@/services/native/ports')
    vi.mocked(getWorkspacesReadiness).mockResolvedValue([makeReadiness({ status: 'blocked' })])
    await useConflictsStore.getState().load()
    expect(
      useControlStore.getState().history.filter((e) => e.action === 'workspace_blocked'),
    ).toHaveLength(0)
  })
})
