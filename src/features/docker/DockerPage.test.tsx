/**
 * Docker page — read-only observability UX (Phase 10A, spec §33).
 * Also asserts the Phase 9 safety guarantee at the UI level: no container
 * lifecycle controls of any kind exist on the page.
 */
import { render, screen, cleanup } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { mockNativeClient, restoreNativeClient } from '@/test/nativeMock'
import { DockerPage } from '@/features/docker/DockerPage'
import { makeContainer, makeDockerSnapshot } from '@/test/fixtures'
import { useDockerStore, stopDockerPolling } from '@/stores/dockerStore'

describe('DockerPage', () => {
  beforeEach(() => {
    mockNativeClient()
  })
  afterEach(() => {
    cleanup()
    stopDockerPolling()
    stopDockerPolling()
    restoreNativeClient()
    useDockerStore.setState({ snapshot: null, loading: false, refreshing: false, error: null, lastUpdated: null })
    vi.restoreAllMocks()
  })

  it('shows the checking state on first load', () => {
    useDockerStore.setState({ snapshot: null, loading: true })
    render(<DockerPage />)
    expect(screen.getByText('Checking Docker Engine…')).toBeInTheDocument()
  })

  it('renders Docker-unavailable as a normal state, not an error', () => {
    useDockerStore.setState({
      snapshot: makeDockerSnapshot({
        available: false,
        engine: undefined,
        containers: [],
        projectLinks: [],
        portOwnerships: [],
        error: { kind: 'docker_unavailable', detail: 'pipe missing' },
      }),
      loading: false,
      error: 'Docker Engine is not currently available.',
    })
    render(<DockerPage />)
    expect(screen.getByText('Docker Unavailable')).toBeInTheDocument()
    expect(screen.getByText('Docker Engine is not currently available.')).toBeInTheDocument()
  })

  it('renders access-denied honestly without offering elevation', () => {
    useDockerStore.setState({
      snapshot: makeDockerSnapshot({
        available: false,
        containers: [],
        projectLinks: [],
        portOwnerships: [],
        error: { kind: 'access_denied', detail: 'ACL' },
      }),
      loading: false,
      error: 'Docker Engine detected but access was denied.',
    })
    render(<DockerPage />)
    expect(screen.getByText('Docker Engine detected but access was denied.')).toBeInTheDocument()
    expect(screen.queryByText(/administrator/i)).not.toBeInTheDocument()
    expect(screen.queryByText(/run as admin/i)).not.toBeInTheDocument()
  })

  it('renders a running container with image, state and compose identity', () => {
    useDockerStore.setState({
      snapshot: makeDockerSnapshot({
        containers: [
          makeContainer({
            compose: { projectName: 'historyai', serviceName: 'frontend' },
          }),
        ],
      }),
      loading: false,
    })
    render(<DockerPage />)
    expect(screen.getByText('historyai-frontend-1')).toBeInTheDocument()
    expect(screen.getByText('historyai/frontend:dev')).toBeInTheDocument()
    expect(screen.getByText('running')).toBeInTheDocument()
    expect(screen.getByText('compose: historyai / frontend')).toBeInTheDocument()
  })

  it('shows health only when a healthcheck exists (none stays quiet)', () => {
    useDockerStore.setState({
      snapshot: makeDockerSnapshot({
        containers: [makeContainer({ health: 'none' }), makeContainer({ id: 'h1', name: 'with-health', health: 'unhealthy' })],
      }),
      loading: false,
    })
    render(<DockerPage />)
    expect(screen.getByText('unhealthy')).toBeInTheDocument()
    // No spurious 'none' badge anywhere.
    expect(screen.queryByText('none')).not.toBeInTheDocument()
  })

  it('shows published host→container port mapping distinctly', () => {
    useDockerStore.setState({ snapshot: makeDockerSnapshot(), loading: false })
    render(<DockerPage />)
    expect(screen.getByText('3000 → 3000/tcp')).toBeInTheDocument()
  })

  it('NO lifecycle controls exist anywhere on the page', () => {
    useDockerStore.setState({ snapshot: makeDockerSnapshot(), loading: false })
    render(<DockerPage />)
    const forbidden = [/^start$/i, /^stop$/i, /^restart$/i, /^kill$/i, /^exec$/i, /^delete$/i, /^remove$/i, /^pause$/i]
    for (const pattern of forbidden) {
      expect(screen.queryByRole('button', { name: pattern })).not.toBeInTheDocument()
    }
    // The only button on a container card is the details expander.
    const expanders = screen.getAllByRole('button', { name: /Expand details|Collapse details/ })
    expect(expanders.length).toBeGreaterThan(0)
  })
})
