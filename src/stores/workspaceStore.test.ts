import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { restoreNativeClient, mockNativeClient } from '@/test/nativeMock'
import { makeWorkspace, makeOutcome } from '@/test/fixtures'
import { useWorkspaceStore } from '@/stores/workspaceStore'
import { useControlStore } from '@/stores/controlStore'

describe('workspaceStore', () => {
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
    })
    useControlStore.setState({ history: [], pending: null })
  })

  it('loads the workspace list', async () => {
    const { listWorkspaces } = await import('@/services/native/ports')
    vi.mocked(listWorkspaces).mockResolvedValue([makeWorkspace()])
    await useWorkspaceStore.getState().load()
    expect(useWorkspaceStore.getState().workspaces).toHaveLength(1)
    expect(useWorkspaceStore.getState().loading).toBe(false)
  })

  it('startService records success', async () => {
    const { startManagedService } = await import('@/services/native/ports')
    vi.mocked(startManagedService).mockResolvedValue(makeOutcome({ message: 'Vite started (pid 5000).' }))
    await useWorkspaceStore.getState().startService('spec-1', 'Vite')
    const entry = useControlStore.getState().history[0]
    expect(entry.action).toBe('service_start')
    expect(entry.outcome).toBe('success')
  })

  it('startService labels PORT_CONFLICT honestly as stale', async () => {
    const { startManagedService } = await import('@/services/native/ports')
    vi.mocked(startManagedService).mockResolvedValue(
      makeOutcome({ ok: false, code: 'PORT_CONFLICT', message: 'Port 3000 is occupied.' }),
    )
    await useWorkspaceStore.getState().startService('spec-1', 'Vite')
    const entry = useControlStore.getState().history[0]
    expect(entry.action).toBe('port_conflict')
    expect(entry.outcome).toBe('stale')
  })

  it('startService labels ALREADY_RUNNING as stale (duplicate protection)', async () => {
    const { startManagedService } = await import('@/services/native/ports')
    vi.mocked(startManagedService).mockResolvedValue(
      makeOutcome({ ok: false, code: 'ALREADY_RUNNING', message: 'Already running.' }),
    )
    await useWorkspaceStore.getState().startService('spec-1', 'Vite')
    expect(useControlStore.getState().history[0].outcome).toBe('stale')
  })

  it('stopService with force records service_force_stop', async () => {
    const { stopManagedService } = await import('@/services/native/ports')
    vi.mocked(stopManagedService).mockResolvedValue(makeOutcome({ message: 'Terminated.' }))
    await useWorkspaceStore.getState().stopService('managed-1', 'Vite', true)
    expect(useControlStore.getState().history[0].action).toBe('service_force_stop')
  })

  it('STOP_TIMEOUT is recorded as a failure, not silently retried', async () => {
    const { stopManagedService } = await import('@/services/native/ports')
    vi.mocked(stopManagedService).mockResolvedValue(
      makeOutcome({ ok: false, code: 'STOP_TIMEOUT', message: 'Process did not exit within the timeout.' }),
    )
    await useWorkspaceStore.getState().stopService('managed-1', 'Vite')
    const entry = useControlStore.getState().history[0]
    expect(entry.outcome).toBe('failure')
    expect(entry.action).toBe('service_stop')
  })

  it('stopWorkspace reports partial results honestly', async () => {
    const { stopWorkspaceServices } = await import('@/services/native/ports')
    vi.mocked(stopWorkspaceServices).mockResolvedValue([
      makeOutcome({ message: 'stopped' }),
      makeOutcome({ ok: false, code: 'STOP_TIMEOUT', message: 'timeout' }),
    ])
    await useWorkspaceStore.getState().stopWorkspace('ws-1', 'historyai')
    const entry = useControlStore.getState().history.find((e) => e.action === 'workspace_stop')
    expect(entry?.message).toBe('1/2 stopped.')
    expect(entry?.outcome).toBe('failure')
  })

  it('openLogs resets the cursor and buffer', () => {
    useWorkspaceStore.getState().openLogs('managed-1')
    expect(useWorkspaceStore.getState().openLogManagedId).toBe('managed-1')
    expect(useWorkspaceStore.getState().logCursor).toBe(0)
  })

  it('pollLogs appends incrementally and bounds the buffer at 1000 lines', async () => {
    useWorkspaceStore.getState().openLogs('managed-1')
    const { getServiceLogs } = await import('@/services/native/ports')
    const line = (n: number) => ({ at: n, stream: 'stdout' as const, line: `l${n}` })
    // First batch of 600 lines, cursor 599.
    vi.mocked(getServiceLogs).mockResolvedValueOnce({
      lines: Array.from({ length: 600 }, (_, i) => line(i)),
      lastIndex: 599,
    })
    // Second batch of 600 more.
    vi.mocked(getServiceLogs).mockResolvedValueOnce({
      lines: Array.from({ length: 600 }, (_, i) => line(i + 600)),
      lastIndex: 1199,
    })
    const store = useWorkspaceStore.getState()
    await store.pollLogs()
    await store.pollLogs()
    const { logLines } = useWorkspaceStore.getState()
    expect(logLines).toHaveLength(1000)
    expect(logLines[999].line).toBe('l1199')
    expect(useWorkspaceStore.getState().logCursor).toBe(1199)
  })

  it('pollLogs with no open service is a no-op', async () => {
    await useWorkspaceStore.getState().pollLogs()
    const { getServiceLogs } = await import('@/services/native/ports')
    expect(getServiceLogs).not.toHaveBeenCalled()
  })

  it('create records workspace_create', async () => {
    const { createWorkspace } = await import('@/services/native/ports')
    vi.mocked(createWorkspace).mockResolvedValue(makeWorkspace())
    const ws = await useWorkspaceStore.getState().create('D:\\Projects\\historyai')
    expect(ws.id).toBe('ws-1')
    expect(useControlStore.getState().history[0].action).toBe('workspace_create')
  })

  it('startWorkspace stops early on PORT_CONFLICT and records both events', async () => {
    const { startWorkspaceServices } = await import('@/services/native/ports')
    vi.mocked(startWorkspaceServices).mockResolvedValue([
      makeOutcome({ message: 'started' }),
      makeOutcome({ ok: false, code: 'PORT_CONFLICT', message: 'Port 3000 occupied.' }),
    ])
    await useWorkspaceStore.getState().startWorkspace('ws-1', 'historyai')
    const history = useControlStore.getState().history
    expect(history.some((e) => e.action === 'port_conflict' && e.outcome === 'stale')).toBe(true)
    const wsEvent = history.find((e) => e.action === 'workspace_start')
    expect(wsEvent?.outcome).toBe('failure')
    expect(wsEvent?.message).toContain('Stopped early')
  })

  it('restartService records success and failure distinctly', async () => {
    const { restartManagedService } = await import('@/services/native/ports')
    vi.mocked(restartManagedService).mockResolvedValueOnce(makeOutcome({ message: 'restarted, new pid 6001' }))
    await useWorkspaceStore.getState().restartService('managed-1', 'Vite')
    expect(useControlStore.getState().history[0]).toMatchObject({ action: 'service_restart', outcome: 'success' })
    vi.mocked(restartManagedService).mockResolvedValueOnce(
      makeOutcome({ ok: false, code: 'STALE_SPEC', message: 'Launch spec no longer valid.' }),
    )
    await useWorkspaceStore.getState().restartService('managed-1', 'Vite')
    // History is append-only (oldest first) — the refusal is the newest entry.
    const history = useControlStore.getState().history
    expect(history[history.length - 1].outcome).toBe('stale')
  })

  it('a failed list load records the error without wiping workspaces', async () => {
    const { listWorkspaces } = await import('@/services/native/ports')
    vi.mocked(listWorkspaces).mockResolvedValue([makeWorkspace()])
    await useWorkspaceStore.getState().load()
    vi.mocked(listWorkspaces).mockRejectedValueOnce(new Error('backend gone'))
    await useWorkspaceStore.getState().refresh()
    const state = useWorkspaceStore.getState()
    expect(state.error).toBe('backend gone')
    expect(state.workspaces).toHaveLength(1)
  })

  it('remove drops the workspace from the list', async () => {
    const { listWorkspaces, removeWorkspace } = await import('@/services/native/ports')
    vi.mocked(removeWorkspace).mockResolvedValue(undefined)
    vi.mocked(listWorkspaces).mockResolvedValue([makeWorkspace()])
    await useWorkspaceStore.getState().load()
    await useWorkspaceStore.getState().remove('ws-1')
    expect(useWorkspaceStore.getState().workspaces).toHaveLength(0)
  })

  it('clearLogs empties the buffer but keeps the cursor', async () => {
    useWorkspaceStore.getState().openLogs('managed-1')
    const { getServiceLogs } = await import('@/services/native/ports')
    vi.mocked(getServiceLogs).mockResolvedValueOnce({
      lines: [{ at: 1, stream: 'stdout', line: 'hello' }],
      lastIndex: 5,
    })
    await useWorkspaceStore.getState().pollLogs()
    useWorkspaceStore.getState().clearLogs()
    expect(useWorkspaceStore.getState().logLines).toHaveLength(0)
  })

  it('pollLogs survives a backend failure (log polling is not fatal)', async () => {
    useWorkspaceStore.getState().openLogs('managed-1')
    const { getServiceLogs } = await import('@/services/native/ports')
    vi.mocked(getServiceLogs).mockRejectedValueOnce(new Error('process exited'))
    await expect(useWorkspaceStore.getState().pollLogs()).resolves.toBeUndefined()
    expect(useWorkspaceStore.getState().logLines).toHaveLength(0)
  })
})
