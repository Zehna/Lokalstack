import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { restoreNativeClient, mockNativeClient } from '@/test/nativeMock'
import { makeDockerSnapshot, makeContainer } from '@/test/fixtures'
import {
  startDockerPolling,
  stopDockerPolling,
  useDockerStore,
} from '@/stores/dockerStore'
import { useControlStore } from '@/stores/controlStore'

describe('dockerStore', () => {
  beforeEach(() => {
    mockNativeClient()
    stopDockerPolling()
  })
  afterEach(() => {
    restoreNativeClient()
    stopDockerPolling()
    useDockerStore.setState({ snapshot: null, loading: false, refreshing: false, error: null, lastUpdated: null })
    useControlStore.setState({ history: [], pending: null })
  })

  it('stores an available snapshot with engine info', async () => {
    const { getDockerSnapshot } = await import('@/services/native/ports')
    vi.mocked(getDockerSnapshot).mockResolvedValue(makeDockerSnapshot())
    await useDockerStore.getState().load()
    const state = useDockerStore.getState()
    expect(state.snapshot?.available).toBe(true)
    expect(state.snapshot?.engine?.version).toBe('27.0.0')
    expect(state.error).toBeNull()
  })

  it('renders Docker-unavailable as a normal state, not a scary error', async () => {
    const { getDockerSnapshot } = await import('@/services/native/ports')
    vi.mocked(getDockerSnapshot).mockResolvedValue(
      makeDockerSnapshot({
        available: false,
        engine: undefined,
        containers: [],
        projectLinks: [],
        portOwnerships: [],
        error: { kind: 'docker_unavailable', detail: 'pipe not found' },
      }),
    )
    await useDockerStore.getState().load()
    const state = useDockerStore.getState()
    expect(state.snapshot?.available).toBe(false)
    expect(state.error).toBe('Docker Engine is not currently available.')
  })

  it('maps access-denied to the honest non-elevating message', async () => {
    const { getDockerSnapshot } = await import('@/services/native/ports')
    vi.mocked(getDockerSnapshot).mockResolvedValue(
      makeDockerSnapshot({
        available: false,
        containers: [],
        projectLinks: [],
        portOwnerships: [],
        error: { kind: 'access_denied', detail: 'ACL' },
      }),
    )
    await useDockerStore.getState().load()
    expect(useDockerStore.getState().error).toBe('Docker Engine detected but access was denied.')
  })

  it('emits one container-started observation per state change', async () => {
    const { getDockerSnapshot, refreshDocker } = await import('@/services/native/ports')
    vi.mocked(getDockerSnapshot).mockResolvedValue(makeDockerSnapshot())
    await useDockerStore.getState().load()
    const withRedis = makeDockerSnapshot({
      containers: [
        makeContainer({ id: 'abc123def456', name: 'historyai-frontend-1', state: 'running' }),
        makeContainer({ id: 'c2', name: 'redis', state: 'running' }),
      ],
    })
    vi.mocked(refreshDocker).mockResolvedValue(withRedis)
    await useDockerStore.getState().refresh()
    const started = useControlStore.getState().history.filter((e) => e.action === 'container_started_observed')
    expect(started).toHaveLength(1)
    expect(started[0].subject).toBe('redis')
    // Same state on the next poll → no duplicate observation.
    await useDockerStore.getState().refresh()
    expect(useControlStore.getState().history.filter((e) => e.action === 'container_started_observed')).toHaveLength(1)
  })

  it('records unhealthy and recovered health transitions', async () => {
    const { getDockerSnapshot, refreshDocker } = await import('@/services/native/ports')
    vi.mocked(getDockerSnapshot).mockResolvedValue(makeDockerSnapshot())
    await useDockerStore.getState().load()
    vi.mocked(refreshDocker).mockResolvedValue(
      makeDockerSnapshot({
        containers: [makeContainer({ health: 'unhealthy' })],
      }),
    )
    await useDockerStore.getState().refresh()
    expect(useControlStore.getState().history.filter((e) => e.action === 'container_health_unhealthy')).toHaveLength(1)
    // Still unhealthy → no repeat.
    await useDockerStore.getState().refresh()
    expect(useControlStore.getState().history.filter((e) => e.action === 'container_health_unhealthy')).toHaveLength(1)
    // Recovery.
    vi.mocked(refreshDocker).mockResolvedValue(
      makeDockerSnapshot({
        containers: [makeContainer({ health: 'healthy' })],
      }),
    )
    await useDockerStore.getState().refresh()
    expect(useControlStore.getState().history.filter((e) => e.action === 'container_health_recovered')).toHaveLength(1)
  })

  it('polling helpers are idempotent for start and stop', () => {
    expect(() => {
      startDockerPolling()
      startDockerPolling()
      stopDockerPolling()
      stopDockerPolling()
    }).not.toThrow()
    // Double start must not throw or duplicate; double stop must not throw.
  })

  it('manual refresh bypasses the backend cache command', async () => {
    const { refreshDocker } = await import('@/services/native/ports')
    vi.mocked(refreshDocker).mockResolvedValue(makeDockerSnapshot())
    await useDockerStore.getState().refresh()
    expect(refreshDocker).toHaveBeenCalledTimes(1)
  })
})
