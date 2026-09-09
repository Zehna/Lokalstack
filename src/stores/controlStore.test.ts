import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { restoreNativeClient, mockNativeClient } from '@/test/nativeMock'
import { useControlStore, type ControlHistoryEntry } from '@/stores/controlStore'

function targetsOf(entries: ControlHistoryEntry[]): string[] {
  return entries.map((e) => e.outcome)
}

describe('controlStore', () => {
  beforeEach(() => {
    mockNativeClient()
  })
  afterEach(() => {
    restoreNativeClient()
    useControlStore.setState({ pending: null, history: [] })
  })

  const target = { id: 'target-abc', displayName: 'Vite — historyai', processName: 'node.exe' }

  it('records a successful End Process with pid', async () => {
    const { endProcess } = await import('@/services/native/ports')
    vi.mocked(endProcess).mockResolvedValueOnce({
      stopped: true,
      gracefulAttempted: false,
      stillRunning: false,
      message: 'Terminated node.exe (pid 4212).',
    })
    await useControlStore.getState().endProcess(target, 4212)
    const history = useControlStore.getState().history
    expect(history).toHaveLength(1)
    expect(history[0]).toMatchObject({
      action: 'end_process',
      subject: 'Vite — historyai',
      pid: 4212,
      outcome: 'success',
    })
  })

  it('records a failed open with the error message', async () => {
    const { openServiceUrl } = await import('@/services/native/ports')
    vi.mocked(openServiceUrl).mockRejectedValueOnce(new Error('spawn ENOENT'))
    await expect(useControlStore.getState().open('http://localhost:3000', 4212, 'Vite')).rejects.toThrow()
    const history = useControlStore.getState().history
    expect(history[0].outcome).toBe('failure')
    expect(history[0].message).toContain('spawn ENOENT')
  })

  it('classifies STALE_TARGET refusals as stale with the safe copy', async () => {
    const { endProcess } = await import('@/services/native/ports')
    vi.mocked(endProcess).mockRejectedValueOnce(
      new Error('STALE_TARGET: process identity changed since discovery'),
    )
    await expect(useControlStore.getState().endProcess(target, 4212)).rejects.toThrow()
    const entry = useControlStore.getState().history[0]
    expect(entry.outcome).toBe('stale')
    expect(entry.message).toContain('nothing was terminated')
  })

  it('refuses a second action while one is in flight', async () => {
    const { endProcess } = await import('@/services/native/ports')
    let release: (() => void) | undefined
    vi.mocked(endProcess).mockImplementation(
      () => new Promise((resolve) => { release = () => resolve({ stopped: true, gracefulAttempted: false, stillRunning: false, message: 'ok' }) }),
    )
    const first = useControlStore.getState().endProcess(target, 4212)
    await expect(useControlStore.getState().endProcess(target, 4212)).rejects.toThrow(
      'Another control action is already running',
    )
    release?.()
    await first
    expect(useControlStore.getState().pending).toBeNull()
  })

  it('clears pending even when the action throws', async () => {
    const { endProcess } = await import('@/services/native/ports')
    vi.mocked(endProcess).mockRejectedValueOnce(new Error('boom'))
    await expect(useControlStore.getState().endProcess(target, 4212)).rejects.toThrow()
    expect(useControlStore.getState().pending).toBeNull()
  })

  it('bounds the history at 100 entries', async () => {
    const control = useControlStore.getState()
    for (let i = 0; i < 130; i++) {
      await control.open(`http://localhost:${3000 + (i % 50)}`, null, 'x')
    }
    expect(targetsOf(useControlStore.getState().history)).toHaveLength(100)
  })

  it('clearHistory empties the trail', async () => {
    await useControlStore.getState().open('http://localhost:3000', null, 'Vite')
    useControlStore.getState().clearHistory()
    expect(useControlStore.getState().history).toHaveLength(0)
  })
})
