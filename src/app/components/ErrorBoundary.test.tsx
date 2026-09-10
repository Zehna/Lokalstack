/**
 * Phase 10B (spec §K) — Error Boundary regression tests.
 *
 * A render exception in one view must produce a recoverable alert surface
 * (no blank WebView, no stack trace), offer Retry / Go to Dashboard, and
 * never auto-remount a deterministically throwing view.
 */
import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { ErrorBoundary } from './ErrorBoundary'

/** Component that throws on every render — deterministically. */
function Bomb({ label }: { label: string }): never {
  throw new Error(`boom: ${label}`)
}

function Healthy(): React.ReactElement {
  return <p>healthy view</p>
}

afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
  vi.clearAllMocks()
})

describe('ErrorBoundary', () => {
  it('renders children when nothing throws', () => {
    render(
      <ErrorBoundary>
        <Healthy />
      </ErrorBoundary>,
    )
    expect(screen.getByText('healthy view')).toBeInTheDocument()
  })

  it('shows a recoverable alert surface when a child throws — not a blank page', () => {
    // Silence the expected console.error noise from React + our boundary.
    const consoleError = vi.spyOn(console, 'error').mockImplementation(() => {})

    render(
      <ErrorBoundary>
        <Bomb label="x" />
      </ErrorBoundary>,
    )

    const alert = screen.getByRole('alert')
    expect(alert).toBeInTheDocument()
    expect(screen.getByText('Something went wrong in this view.')).toBeInTheDocument()
    // No raw stack trace / error message leak to the user.
    expect(alert.textContent).not.toContain('boom')
    expect(alert.textContent).not.toContain('at ')
    expect(consoleError).toHaveBeenCalled() // detail stayed in local dev logs
  })

  it('Retry View remounts a recovered child', () => {
    vi.spyOn(console, 'error').mockImplementation(() => {})
    let shouldThrow = true

    function Flaky(): React.ReactElement {
      if (shouldThrow) throw new Error('flaky')
      return <p>recovered</p>
    }

    render(
      <ErrorBoundary>
        <Flaky />
      </ErrorBoundary>,
    )
    expect(screen.getByRole('alert')).toBeInTheDocument()

    shouldThrow = false
    fireEvent.click(screen.getByRole('button', { name: 'Retry View' }))
    expect(screen.getByText('recovered')).toBeInTheDocument()
    expect(screen.queryByRole('alert')).not.toBeInTheDocument()
  })

  it('Go to Dashboard invokes the shell navigation callback', () => {
    vi.spyOn(console, 'error').mockImplementation(() => {})
    const onNavigateDashboard = vi.fn()

    render(
      <ErrorBoundary onNavigateDashboard={onNavigateDashboard}>
        <Bomb label="y" />
      </ErrorBoundary>,
    )
    fireEvent.click(screen.getByRole('button', { name: 'Go to Dashboard' }))
    expect(onNavigateDashboard).toHaveBeenCalledTimes(1)
  })

  it('navigating away (resetKey change) gives the new view a fresh render', () => {
    vi.spyOn(console, 'error').mockImplementation(() => {})
    const { rerender } = render(
      <ErrorBoundary resetKey="ports">
        <Bomb label="z" />
      </ErrorBoundary>,
    )
    expect(screen.getByRole('alert')).toBeInTheDocument()

    // Navigating to the Dashboard mounts a (healthy) view — it must render.
    rerender(
      <ErrorBoundary resetKey="dashboard">
        <Healthy />
      </ErrorBoundary>,
    )
    expect(screen.getByText('healthy view')).toBeInTheDocument()
    expect(screen.queryByRole('alert')).not.toBeInTheDocument()
  })

  it('does NOT auto-retry a deterministically throwing view (no render loop)', () => {
    const consoleError = vi.spyOn(console, 'error').mockImplementation(() => {})
    render(
      <ErrorBoundary>
        <Bomb label="loop" />
      </ErrorBoundary>,
    )

    expect(screen.getByRole('alert')).toBeInTheDocument()
    // componentDidCatch fired exactly once — no auto-remount loop.
    const boundaryCalls = consoleError.mock.calls.filter((args) =>
      String(args[0]).includes('[ErrorBoundary]'),
    )
    expect(boundaryCalls).toHaveLength(1)
  })
})
