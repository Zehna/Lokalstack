/**
 * Phase 10D (spec §U–AH) — accessibility regression tests.
 *
 * Critical, deterministic a11y behaviors: the conflict dialog traps Escape
 * and pulls focus inside; destructive confirmations never autofocus the
 * destructive control; icon-only buttons carry accessible names; status
 * badges are text (never color-only); switches expose role/aria-checked.
 * Native calls always go through the Phase 10A seam mock.
 */
import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { mockNativeClient, restoreNativeClient } from '@/test/nativeMock'
import { makeConflictReport, makeDockerSnapshot } from '@/test/fixtures'
import { WorkspacesPage } from '@/features/workspaces/WorkspacesPage'
import { DockerPage } from '@/features/docker/DockerPage'
import { useConflictsStore } from '@/stores/conflictsStore'
import { useDockerStore } from '@/stores/dockerStore'

describe('accessibility (Phase 10D)', () => {
  let mocks: ReturnType<typeof mockNativeClient>

  beforeEach(() => {
    mocks = mockNativeClient()
  })

  afterEach(() => {
    cleanup()
    restoreNativeClient()
  })

  it('conflict dialog: focus enters the dialog and Escape closes it', () => {
    const report = makeConflictReport({ requestedPort: 3000 })
    useConflictsStore.setState({ lastPortReport: report, portSuggestions: [] })

    render(<WorkspacesPage />)

    const dialog = screen.getByRole('dialog', { name: 'Port conflict' })
    expect(dialog).toBeInTheDocument()
    // Focus must be inside the dialog (the Close button) — not behind it.
    expect(screen.getByRole('button', { name: 'Close' })).toHaveFocus()

    fireEvent.keyDown(window, { key: 'Escape' })
    expect(useConflictsStore.getState().lastPortReport).toBeNull()
  })

  it('conflict dialog exposes owner identity as text (not color-only)', () => {
    const report = makeConflictReport({
      requestedPort: 5432,
      owner: { pid: 4242, processName: 'postgres', projectName: undefined, lifecycle: 'external' },
    } as never)
    useConflictsStore.setState({ lastPortReport: report, portSuggestions: [] })

    render(<WorkspacesPage />)

    const dialog = screen.getByRole('dialog', { name: 'Port conflict' })
    expect(dialog).toHaveTextContent('Owner:')
    expect(dialog).toHaveTextContent('postgres')
    expect(dialog).toHaveTextContent('PID 4242')
    expect(dialog).toHaveTextContent('Lifecycle: external')
  })

  it('Docker container expand/collapse is an icon-only button WITH an accessible name', () => {
    const snapshot = makeDockerSnapshot()
    useDockerStore.setState({ snapshot, loading: false, error: null })

    render(<DockerPage />)

    const expand = screen.getAllByRole('button', { name: 'Expand details' })
    expect(expand.length).toBeGreaterThan(0)
    // Toggling keeps the name in sync with state.
    fireEvent.click(expand[0])
    expect(screen.getAllByRole('button', { name: 'Collapse details' }).length).toBeGreaterThan(0)
  })

  it('settings switches expose role=switch with accurate aria-checked', async () => {
    const { SettingsPage } = await import('@/features/settings/SettingsPage')
    mocks.getAppSettings.mockResolvedValue({
      autoRefresh: true,
      portRefreshIntervalMs: 3000,
      aiPollingEnabled: false,
      dockerPollingEnabled: true,
      launchMinimized: false,
      closeBehavior: 'exit',
      theme: 'system',
      runAtStartup: false,
      startupRegistered: false,
    })

    render(<SettingsPage />)
    await screen.findByText('Discovery & Polling')

    const ai = screen.getByRole('switch', { name: 'AI runtime polling' })
    expect(ai).toHaveAttribute('aria-checked', 'false')
    // The switch is controlled by the store; the click triggers save which
    // applies the returned effective settings.
    mocks.saveAppSettings.mockResolvedValue({
      settings: {
        autoRefresh: true,
        portRefreshIntervalMs: 3000,
        aiPollingEnabled: true,
        dockerPollingEnabled: true,
        launchMinimized: false,
        closeBehavior: 'exit',
        theme: 'system',
        runAtStartup: false,
        startupRegistered: false,
      },
      correctedNote: null,
    })
    fireEvent.click(ai)
    await vi.waitFor(() => expect(ai).toHaveAttribute('aria-checked', 'true'))
  })

  it('destructive confirmation keeps default focus on the safe control', async () => {
    const { SettingsPage } = await import('@/features/settings/SettingsPage')
    mocks.getAppSettings.mockResolvedValue({
      autoRefresh: true,
      portRefreshIntervalMs: 3000,
      aiPollingEnabled: true,
      dockerPollingEnabled: true,
      launchMinimized: false,
      closeBehavior: 'exit',
      theme: 'system',
      runAtStartup: false,
      startupRegistered: false,
    })
    mocks.resetAppSettings.mockResolvedValue({
      autoRefresh: true,
      portRefreshIntervalMs: 3000,
      aiPollingEnabled: true,
      dockerPollingEnabled: true,
      launchMinimized: false,
      closeBehavior: 'exit',
      theme: 'system',
      runAtStartup: false,
      startupRegistered: false,
    })

    render(<SettingsPage />)
    await screen.findByText('Discovery & Polling')

    // Arming the reset must NOT move focus to the destructive button.
    fireEvent.click(screen.getByRole('button', { name: /Reset to Defaults/i }))
    expect(screen.getByRole('button', { name: 'Confirm reset' })).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Confirm reset' })).not.toHaveFocus()
  })
})

