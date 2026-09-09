/**
 * Control confirmation UX — the Phase 10A safety-critical component tests
 * (spec §32). No test can trigger termination without the two-click
 * confirmation, and the frontend only ever sends the opaque target id.
 */
import { render, screen, cleanup } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { mockNativeClient, restoreNativeClient } from '@/test/nativeMock'
import { ControlActions, ConfirmButton } from '@/app/components/ControlActions'
import { makeDevControl, makeSystemControl } from '@/test/fixtures'
import { useControlStore } from '@/stores/controlStore'

describe('ControlActions — hardened control UX', () => {
  beforeEach(() => {
    mockNativeClient()
  })
  afterEach(() => {
    cleanup()
    restoreNativeClient()
    useControlStore.setState({ pending: null, history: [] })
    vi.restoreAllMocks()
  })

  it('system/protected process shows a disabled Stop, never an action button', () => {
    render(<ControlActions control={makeSystemControl()} />)
    expect(screen.getByText('Stop unavailable')).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: /End Process/ })).not.toBeInTheDocument()
    expect(screen.queryByRole('button', { name: /End/ })).not.toBeInTheDocument()
  })

  it('eligible process shows the End Process button (honest terminate wording)', () => {
    render(<ControlActions control={makeDevControl()} />)
    expect(screen.getByRole('button', { name: 'End Process' })).toBeInTheDocument()
  })

  it('first click only arms the confirmation — no action fires', async () => {
    const user = userEvent.setup()
    const { endProcess } = await import('@/services/native/ports')
    render(<ControlActions control={makeDevControl()} />)
    await user.click(screen.getByRole('button', { name: 'End Process' }))
    expect(endProcess).not.toHaveBeenCalled()
    expect(screen.getByRole('button', { name: /Terminate Vite — historyai now/ })).toBeInTheDocument()
  })

  it('second click confirms and terminates through the opaque target id', async () => {
    const user = userEvent.setup()
    const { endProcess } = await import('@/services/native/ports')
    vi.mocked(endProcess).mockResolvedValueOnce({
      stopped: true,
      gracefulAttempted: false,
      stillRunning: false,
      message: 'Terminated node.exe (pid 4212).',
    })
    render(<ControlActions control={makeDevControl()} />)
    await user.click(screen.getByRole('button', { name: 'End Process' }))
    await user.click(screen.getByRole('button', { name: /Terminate Vite — historyai now/ }))
    expect(endProcess).toHaveBeenCalledTimes(1)
    expect(endProcess).toHaveBeenCalledWith('target-abc')
  })

  it('leaving the button disarms it (accidental double-click is safe)', async () => {
    const user = userEvent.setup()
    const { endProcess } = await import('@/services/native/ports')
    render(<ControlActions control={makeDevControl()} />)
    await user.click(screen.getByRole('button', { name: 'End Process' }))
    // Move the pointer away — the component disarms on mouseleave.
    fireEventMouseLeave(screen.getByRole('button', { name: /Terminate/ }))
    // Disarmed: the next click re-arms instead of confirming.
    await user.click(screen.getByRole('button', { name: /Terminate|End Process/ }))
    expect(endProcess).not.toHaveBeenCalled()
  })

  it('a stale/unknown target refusal is displayed, not thrown away', async () => {
    const user = userEvent.setup()
    const { endProcess } = await import('@/services/native/ports')
    vi.mocked(endProcess).mockRejectedValueOnce(
      new Error('STALE_TARGET: process identity changed since discovery'),
    )
    render(<ControlActions control={makeDevControl()} />)
    await user.click(screen.getByRole('button', { name: 'End Process' }))
    await user.click(screen.getByRole('button', { name: /Terminate/ }))
    const error = await screen.findByText(/STALE_TARGET/)
    expect(error).toBeInTheDocument()
  })

  it('an in-flight action disables the button (no repeated click)', async () => {
    const { endProcess } = await import('@/services/native/ports')
    let release: (() => void) | undefined
    vi.mocked(endProcess).mockImplementation(
      () =>
        new Promise((resolve) => {
          release = () => resolve({ stopped: true, gracefulAttempted: false, stillRunning: false, message: 'ok' })
        }),
    )
    render(<ControlActions control={makeDevControl()} />)
    const user = userEvent.setup()
    await user.click(screen.getByRole('button', { name: 'End Process' }))
    await user.click(screen.getByRole('button', { name: /Terminate/ }))
    expect(screen.getByRole('button', { name: /Terminate|End Process/ })).toBeDisabled()
    release?.()
  })

  it('Open is offered only when the snapshot provides a URL', () => {
    render(<ControlActions control={makeDevControl()} />)
    expect(screen.getByTitle('Open http://localhost:3000 in your browser')).toBeInTheDocument()
  })

  it('no Open button when the capability says cannot open (system process)', () => {
    render(<ControlActions control={makeSystemControl()} />)
    expect(screen.queryByTitle(/Open http/)).not.toBeInTheDocument()
  })
})

/** React synthesizes onMouseLeave from native bubbling `mouseout` events. */
function fireEventMouseLeave(element: Element): void {
  element.dispatchEvent(
    new window.MouseEvent('mouseout', { bubbles: true, relatedTarget: document.body }),
  )
}

describe('ConfirmButton — two-click destructive confirmation', () => {
  afterEach(() => {
    cleanup()
  })

  it('requires two clicks; one click never fires the action', async () => {
    const user = userEvent.setup()
    const onConfirm = vi.fn()
    render(<ConfirmButton label="Stop managed" confirmLabel="Confirm stop" onConfirm={onConfirm} />)
    await user.click(screen.getByRole('button', { name: 'Stop managed' }))
    expect(onConfirm).not.toHaveBeenCalled()
    await user.click(screen.getByRole('button', { name: 'Confirm stop' }))
    expect(onConfirm).toHaveBeenCalledTimes(1)
  })
})
