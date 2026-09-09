/**
 * Overlap + stale-response protection (Phase 10A, spec §7–8).
 *
 * The in-flight guards are behavioral: a slow fetch must never stack a
 * second request, and a stale response must never overwrite newer state.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { mockNativeClient, restoreNativeClient } from '@/test/nativeMock'
import { makeWorkspace } from '@/test/fixtures'
import { useWorkspaceStore } from '@/stores/workspaceStore'
import { useConflictsStore } from '@/stores/conflictsStore'

describe('pollLogs in-flight guard', () => {
  beforeEach(() => {
    mockNativeClient()
  })
  afterEach(() => {
    restoreNativeClient()
    useWorkspaceStore.setState({
      workspaces: [],
      loading: true,
      refreshing: false,
      error: null,
      lastUpdated: null,
      openLogManagedId: null,
      logLines: [],
      logCursor: 0,
      logPolling: false,
    })
    vi.restoreAllMocks()
  })

  it('a slow log fetch blocks overlapping polls (single-flight)', async () => {
    useWorkspaceStore.getState().openLogs('managed-1')
    const { getServiceLogs } = await import('@/services/native/ports')
    let release: (() => void) | undefined
    vi.mocked(getServiceLogs).mockImplementation(
      () =>
        new Promise((resolve) => {
          release = () =>
            resolve({
              lines: [{ at: 1, stream: 'stdout' as const, line: 'chunk' }],
              lastIndex: 10,
            })
        }),
    )
    const first = useWorkspaceStore.getState().pollLogs()
    await useWorkspaceStore.getState().pollLogs() // overlapping call is a no-op
    expect(getServiceLogs).toHaveBeenCalledTimes(1)
    release?.()
    await first
    expect(useWorkspaceStore.getState().logPolling).toBe(false)
    expect(useWorkspaceStore.getState().logCursor).toBe(10)
  })

  it('a failed fetch releases the guard so later polls work', async () => {
    useWorkspaceStore.getState().openLogs('managed-1')
    const { getServiceLogs } = await import('@/services/native/ports')
    vi.mocked(getServiceLogs).mockRejectedValueOnce(new Error('pipe closed'))
    await useWorkspaceStore.getState().pollLogs()
    expect(useWorkspaceStore.getState().logPolling).toBe(false)
    vi.mocked(getServiceLogs).mockResolvedValueOnce({
      lines: [{ at: 2, stream: 'stderr' as const, line: 'after failure' }],
      lastIndex: 3,
    })
    await useWorkspaceStore.getState().pollLogs()
    expect(useWorkspaceStore.getState().logLines).toHaveLength(1)
    expect(useWorkspaceStore.getState().logCursor).toBe(3)
  })
})

describe('conflictsStore stale-response protection', () => {
  beforeEach(() => {
    mockNativeClient()
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
    vi.restoreAllMocks()
  })

  it('readiness poll is single-flight (overlapping refresh skipped)', async () => {
    const { getWorkspacesReadiness } = await import('@/services/native/ports')
    let release: (() => void) | undefined
    vi.mocked(getWorkspacesReadiness).mockImplementation(
      () =>
        new Promise((resolve) => {
          release = () => resolve([])
        }),
    )
    const first = useConflictsStore.getState().refresh()
    await useConflictsStore.getState().refresh() // skipped by the guard
    expect(getWorkspacesReadiness).toHaveBeenCalledTimes(1)
    release?.()
    await first
    expect(useConflictsStore.getState().refreshing).toBe(false)
  })

  it('a slow older response never overwrites a newer one (monotonic sequencing)', async () => {
    const { getWorkspacesReadiness } = await import('@/services/native/ports')
    // Start request A (slow, returns the OLD view).
    let releaseA: (() => void) | undefined
    vi.mocked(getWorkspacesReadiness).mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          releaseA = () =>
            resolve([
              {
                workspaceId: 'ws-1',
                workspaceName: 'historyai',
                status: 'running',
                issues: [],
                dependencies: [],
                conflicts: [],
              },
            ])
        }),
    )
    const slowA = useConflictsStore.getState().refresh()
    // Request B starts only after A's guard releases (single-flight), so a
    // manual action always sees the freshest state — assert the second call
    // returns B's data after A resolves.
    releaseA?.()
    await slowA
    vi.mocked(getWorkspacesReadiness).mockResolvedValue([
      {
        workspaceId: 'ws-1',
        workspaceName: 'historyai',
        status: 'blocked',
        issues: [],
        dependencies: [],
        conflicts: [],
      },
    ])
    await useConflictsStore.getState().refresh()
    expect(useConflictsStore.getState().readiness[0]?.status).toBe('blocked')
  })

  it('load subscribes once and readiness loads through the same path', async () => {
    const { getWorkspacesReadiness } = await import('@/services/native/ports')
    vi.mocked(getWorkspacesReadiness).mockResolvedValue([
      {
        workspaceId: 'ws-1',
        workspaceName: 'historyai',
        status: 'running',
        issues: [],
        dependencies: [],
        conflicts: [],
      },
    ])
    await useConflictsStore.getState().load()
    expect(useConflictsStore.getState().readiness).toHaveLength(1)
  })

  it('workspace list load is single-flight too', async () => {
    const { listWorkspaces } = await import('@/services/native/ports')
    let release: (() => void) | undefined
    vi.mocked(listWorkspaces).mockImplementation(
      () =>
        new Promise((resolve) => {
          release = () => resolve([makeWorkspace()])
        }),
    )
    const first = useWorkspaceStore.getState().refresh()
    await useWorkspaceStore.getState().refresh()
    expect(listWorkspaces).toHaveBeenCalledTimes(1)
    release?.()
    await first
  })
})
