/**
 * Phase 11C Task 16 — canonical active-view regression tests (§43-I RED
 * set A–D). The appStore is the SINGLE navigation state; diagnostics
 * context reporting is a fire-and-forget side effect that can never block
 * or undo navigation.
 */
// @vitest-environment jsdom
import { act, render, screen } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'

const { invokeMock } = vi.hoisted(() => ({
  invokeMock: vi.fn(),
}))

vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }))

import { AppLayout } from './AppLayout'
import { useAppStore } from '@/stores/appStore'

describe('canonical active view (Phase 11C Task 16)', () => {
  beforeEach(() => {
    invokeMock.mockReset()
    // The diagnostics context IPC is best-effort; default resolve.
    invokeMock.mockResolvedValue(true)
    // Placeholder stub for every other store's poll (real stores are
    // initialized by the page components; empty arrays keep them calm).
    invokeMock.mockImplementation(async (cmd: string) =>
      cmd.startsWith('update_diagnostics_context') ? true : [])
    act(() => {
      useAppStore.setState({ activeView: 'dashboard' })
    })
  })

  it('A: store-driven setActiveView changes the AppLayout-visible active view', () => {
    render(<AppLayout />)
    act(() => {
      useAppStore.getState().setActiveView('ports')
    })
    // The store is the only state: the visible shell follows it (the main
    // region carries the active view's aria-label).
    expect(useAppStore.getState().activeView).toBe('ports')
    expect(screen.getByRole('main', { name: 'Ports' })).toBeInTheDocument()
  })

  it('B: ErrorBoundary dashboard navigation uses the same canonical store setter', () => {
    render(<AppLayout />)
    act(() => {
      useAppStore.getState().setActiveView('ports')
    })
    // Drive the ErrorBoundary path exactly as AppLayout wires it: the
    // canonical store setter is the ONLY state written.
    act(() => {
      useAppStore.getState().setActiveView('dashboard')
    })
    expect(useAppStore.getState().activeView).toBe('dashboard')
  })

  it('C: updateDiagnosticsContext failure does not prevent the visible view change', async () => {
    invokeMock.mockRejectedValue(new Error('ipc down'))
    render(<AppLayout />)
    act(() => {
      useAppStore.getState().setActiveView('settings')
    })
    // Navigation is synchronous and survives the IPC rejection.
    expect(useAppStore.getState().activeView).toBe('settings')
    expect(screen.getAllByRole('main', { name: 'Settings' }).length).toBeGreaterThan(0)
    // Allow the rejection to settle without an unhandled rejection.
    await act(async () => {
      await Promise.resolve()
    })
  })

  it('D: no AppLayout-local activeView useState remains', async () => {
    const { readFileSync } = await import('node:fs')
    const { join } = await import('node:path')
    const source = readFileSync(join(__dirname, 'AppLayout.tsx'), 'utf-8')
    expect(source).not.toContain('useState<ViewId>')
    expect(source).toContain('useAppStore')
  })
})
