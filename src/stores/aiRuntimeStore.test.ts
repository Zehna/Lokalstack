import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { restoreNativeClient, mockNativeClient } from '@/test/nativeMock'
import { makeAiRuntime } from '@/test/fixtures'
import { useAiRuntimeStore } from '@/stores/aiRuntimeStore'
import { useControlStore } from '@/stores/controlStore'

describe('aiRuntimeStore', () => {
  beforeEach(() => {
    mockNativeClient()
  })
  afterEach(() => {
    restoreNativeClient()
    useAiRuntimeStore.setState({
      runtimes: [],
      loading: true,
      refreshing: false,
      error: null,
      lastUpdated: null,
      errorsByRuntime: {},
    })
    useControlStore.setState({ history: [], pending: null })
  })

  it('loads runtimes and surfaces per-runtime error labels', async () => {
    const { getAiRuntimes } = await import('@/services/native/ports')
    vi.mocked(getAiRuntimes).mockResolvedValue([
      makeAiRuntime(),
      makeAiRuntime({ runtimeId: 'llama-8080', displayName: 'llama.cpp', health: 'unavailable', errorLabel: 'Connection refused' }),
    ])
    await useAiRuntimeStore.getState().load()
    const state = useAiRuntimeStore.getState()
    expect(state.runtimes).toHaveLength(2)
    expect(state.errorsByRuntime['llama-8080']).toBe('Connection refused')
    expect(state.loading).toBe(false)
    expect(state.error).toBeNull()
  })

  it('emits exactly one ready transition per health change (deduped across polls)', async () => {
    const { getAiRuntimes } = await import('@/services/native/ports')
    vi.mocked(getAiRuntimes).mockResolvedValue([makeAiRuntime({ health: 'unavailable' })])
    await useAiRuntimeStore.getState().load()
    vi.mocked(getAiRuntimes).mockResolvedValue([makeAiRuntime({ health: 'ready' })])
    await useAiRuntimeStore.getState().refresh()
    vi.mocked(getAiRuntimes).mockResolvedValue([makeAiRuntime({ health: 'ready' })])
    await useAiRuntimeStore.getState().refresh()

    const readyEvents = useControlStore
      .getState()
      .history.filter((e) => e.action === 'ai_runtime_ready')
    expect(readyEvents).toHaveLength(1)
  })

  it('emits model loaded/unloaded transitions once', async () => {
    const { getAiRuntimes } = await import('@/services/native/ports')
    vi.mocked(getAiRuntimes).mockResolvedValue([makeAiRuntime()])
    await useAiRuntimeStore.getState().load()
    vi.mocked(getAiRuntimes).mockResolvedValue([
      makeAiRuntime({ loadedModels: [{ id: 'llama3.1:8b' }] }),
    ])
    await useAiRuntimeStore.getState().refresh()
    vi.mocked(getAiRuntimes).mockResolvedValue([makeAiRuntime()])
    await useAiRuntimeStore.getState().refresh()

    const history = useControlStore.getState().history
    expect(history.filter((e) => e.action === 'ai_model_loaded')).toHaveLength(1)
    expect(history.filter((e) => e.action === 'ai_model_unloaded')).toHaveLength(1)
  })

  it('a failed refresh records the error but keeps previous runtimes', async () => {
    const { getAiRuntimes } = await import('@/services/native/ports')
    vi.mocked(getAiRuntimes).mockResolvedValue([makeAiRuntime()])
    await useAiRuntimeStore.getState().load()
    vi.mocked(getAiRuntimes).mockRejectedValueOnce(new Error('timeout'))
    await useAiRuntimeStore.getState().refresh()
    const state = useAiRuntimeStore.getState()
    expect(state.error).toBe('timeout')
    expect(state.runtimes).toHaveLength(1)
  })

  it('refreshRuntime bypasses the backend cache', async () => {
    const { getAiRuntimes } = await import('@/services/native/ports')
    vi.mocked(getAiRuntimes).mockResolvedValue([makeAiRuntime()])
    await useAiRuntimeStore.getState().refreshRuntime('ollama-11434')
    expect(getAiRuntimes).toHaveBeenCalledWith(true)
  })
})
