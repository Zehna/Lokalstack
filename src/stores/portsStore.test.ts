import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { restoreNativeClient, mockNativeClient } from '@/test/nativeMock'
import { makeSnapshot } from '@/test/fixtures'
import { usePortsStore } from '@/stores/portsStore'

describe('portsStore', () => {
  beforeEach(async () => {
    mockNativeClient()
    const { getPortListeners } = await import('@/services/native/ports')
    vi.mocked(getPortListeners).mockResolvedValue(makeSnapshot())
  })
  afterEach(() => {
    restoreNativeClient()
    usePortsStore.setState({
      listeners: [],
      processByPid: new Map(),
      serviceByPid: new Map(),
      projects: [],
      projectByPid: new Map(),
      controlByPid: new Map(),
      durationMs: null,
      loading: true,
      refreshing: false,
      error: null,
      lastUpdated: null,
    })
  })

  it('starts with loading and empty data', () => {
    const state = usePortsStore.getState()
    expect(state.loading).toBe(true)
    expect(state.listeners).toEqual([])
    expect(state.error).toBeNull()
  })

  it('loadListeners stores the snapshot and clears loading', async () => {
    usePortsStore.setState({ loading: false })
    await usePortsStore.getState().loadListeners()
    const state = usePortsStore.getState()
    expect(state.loading).toBe(false)
    expect(state.error).toBeNull()
    expect(state.listeners).toHaveLength(1)
    expect(state.processByPid.get(4212)?.name).toBe('node.exe')
    expect(state.controlByPid.get(4212)?.capability.canStop).toBe(true)
    expect(state.projectByPid.get(4212)?.name).toBe('historyai')
  })

  it('a failed load records the error without wiping previous data', async () => {
    usePortsStore.setState({ loading: false })
    await usePortsStore.getState().loadListeners()
    const { getPortListeners } = await import('@/services/native/ports')
    vi.mocked(getPortListeners).mockRejectedValueOnce(new Error('backend boom'))
    await usePortsStore.getState().refreshListeners()
    const state = usePortsStore.getState()
    expect(state.error).toBe('backend boom')
    expect(state.listeners).toHaveLength(1)
  })

  it('skips overlapping refreshes (no duplicate polling loops)', async () => {
    usePortsStore.setState({ loading: false })
    const { getPortListeners } = await import('@/services/native/ports')
    let release: (() => void) | undefined
    vi.mocked(getPortListeners).mockImplementation(
      () => new Promise((resolve) => { release = () => resolve(makeSnapshot()) }),
    )
    const first = usePortsStore.getState().refreshListeners()
    const second = usePortsStore.getState().refreshListeners()
    release?.()
    await Promise.all([first, second])
    expect(getPortListeners).toHaveBeenCalledTimes(1)
  })

  it('manual refresh sets and clears the refreshing flag', async () => {
    usePortsStore.setState({ loading: false })
    await usePortsStore.getState().refreshListeners()
    expect(usePortsStore.getState().refreshing).toBe(false)
    expect(usePortsStore.getState().lastUpdated).not.toBeNull()
  })
})
