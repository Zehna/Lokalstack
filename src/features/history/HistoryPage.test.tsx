/**
 * History page — session audit-trail rendering (Phase 10A, spec §35).
 */
import { render, screen, cleanup } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { mockNativeClient, restoreNativeClient } from '@/test/nativeMock'
import { HistoryPage } from '@/features/history/HistoryPage'
import { useControlStore, type ControlHistoryEntry } from '@/stores/controlStore'

function entry(overrides: Partial<ControlHistoryEntry> = {}): ControlHistoryEntry {
  return {
    id: 1,
    at: new Date('2024-06-01T10:00:00').getTime(),
    action: 'end_process',
    subject: 'Vite — historyai',
    pid: 4212,
    outcome: 'success',
    message: 'Terminated node.exe (pid 4212).',
    ...overrides,
  }
}

describe('HistoryPage', () => {
  beforeEach(() => {
    mockNativeClient()
  })
  afterEach(() => {
    cleanup()
    restoreNativeClient()
    useControlStore.setState({ pending: null, history: [] })
    vi.restoreAllMocks()
  })

  it('shows the empty state before any action', () => {
    render(<HistoryPage />)
    expect(screen.getByText('No control actions this session.')).toBeInTheDocument()
  })

  it('renders a control action with its outcome', () => {
    useControlStore.setState({ history: [entry()] })
    render(<HistoryPage />)
    expect(screen.getByText('Vite — historyai')).toBeInTheDocument()
    expect(screen.getByText('PID 4212')).toBeInTheDocument()
    expect(screen.getByText('success')).toBeInTheDocument()
  })

  it('renders a stale-target refusal with the stale label', () => {
    useControlStore.setState({
      history: [entry({ action: 'end_process', outcome: 'stale', message: 'STALE_TARGET: identity changed' })],
    })
    render(<HistoryPage />)
    expect(screen.getByText('stale target')).toBeInTheDocument()
  })

  it('renders Docker observations with observed-not-initiated wording', () => {
    useControlStore.setState({
      history: [
        entry({
          action: 'container_started_observed',
          subject: 'redis',
          pid: null,
          outcome: 'success',
          message: 'Observed container redis running (not initiated by LocalStack)',
        }),
      ],
    })
    render(<HistoryPage />)
    expect(screen.getByText('redis')).toBeInTheDocument()
    expect(screen.getByText(/not initiated by LocalStack/)).toBeInTheDocument()
  })

  it('renders newest entry first (reverse chronological)', () => {
    useControlStore.setState({
      history: [entry({ id: 1, subject: 'first' }), entry({ id: 2, subject: 'second' })],
    })
    render(<HistoryPage />)
    const items = screen.getAllByText(/first|second/)
    expect(items[0].textContent).toBe('second')
    expect(items[1].textContent).toBe('first')
  })

  it('clear history empties the trail', async () => {
    const user = userEvent.setup()
    useControlStore.setState({ history: [entry()] })
    render(<HistoryPage />)
    await user.click(screen.getByRole('button', { name: 'Clear history' }))
    expect(useControlStore.getState().history).toHaveLength(0)
  })
})
