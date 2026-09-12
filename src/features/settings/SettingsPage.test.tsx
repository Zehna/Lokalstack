/**
 * Settings page — Phase 10C component tests (spec §AF).
 *
 * Uses the centralized native mock (Phase 10A seam). Covers render of
 * defaults, immediate save on toggle, interval commit, reset confirmation,
 * corrected-note and error presentation, and the startup registration
 * toggle showing OS reality.
 */
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { SettingsPage } from './SettingsPage'
import {
  resetPollingAppliers,
  useSettingsStore,
} from '@/stores/settingsStore'
import { mockNativeClient, restoreNativeClient } from '@/test/nativeMock'
import type { AppSettings } from '@/types/domain'

function makeSettings(overrides: Partial<AppSettings> = {}): AppSettings {
  return {
    autoRefresh: true,
    portRefreshIntervalMs: 3000,
    aiPollingEnabled: true,
    dockerPollingEnabled: true,
    launchMinimized: false,
    closeBehavior: 'exit',
    theme: 'system',
    runAtStartup: false,
    startupRegistered: false,
    ...overrides,
  }
}

describe('SettingsPage', () => {
  beforeEach(() => {
    const mocks = mockNativeClient()
    mocks.getAppSettings.mockResolvedValue(makeSettings())
    mocks.resetAppSettings.mockResolvedValue(makeSettings())
    mocks.saveAppSettings.mockImplementation(
      async (patch: Partial<AppSettings>) => ({
        settings: makeSettings(patch),
        correctedNote: null,
      }),
    )
    mocks.setRunAtStartup.mockImplementation(async (enabled: boolean) =>
      makeSettings({ runAtStartup: enabled, startupRegistered: enabled }),
    )
    resetPollingAppliers()
  })

  afterEach(() => {
    cleanup()
    restoreNativeClient()
    resetPollingAppliers()
    useSettingsStore.setState({
      settings: null,
      loading: true,
      saving: false,
      error: null,
      correctedNote: null,
      loaded: false,
    })
  })

  it('renders loaded settings with accessible controls', async () => {
    render(<SettingsPage />)

    await waitFor(() =>
      expect(useSettingsStore.getState().loaded).toBe(true),
    )

    expect(screen.getByText('Discovery & Polling')).toBeInTheDocument()
    expect(screen.getByRole('switch', { name: 'Auto Refresh' })).toBeChecked()
    expect(
      screen.getByRole('switch', { name: 'AI runtime polling' }),
    ).toBeChecked()
    expect(
      screen.getByRole('switch', { name: 'Docker polling' }),
    ).toBeChecked()
    expect(
      screen.getByRole('switch', { name: 'Run at Windows startup' }),
    ).not.toBeChecked()
    expect(screen.getByLabelText(/Refresh interval/i)).toHaveValue(3000)
  })

  it('saves immediately when Auto Refresh is toggled off', async () => {
    render(<SettingsPage />)
    await waitFor(() => expect(useSettingsStore.getState().loaded).toBe(true))

    fireEvent.click(screen.getByRole('switch', { name: 'Auto Refresh' }))

    await waitFor(() =>
      expect(useSettingsStore.getState().saving).toBe(false),
    )
    expect(useSettingsStore.getState().settings?.autoRefresh).toBe(false)
  })

  it('commits the interval on Enter and shows the corrected note', async () => {
    render(<SettingsPage />)
    await waitFor(() => expect(useSettingsStore.getState().loaded).toBe(true))

    // Backend clamps 999999 to the max; the page shows the correction.
    const saveMocks = vi.mocked(
      (await import('@/services/native/ports')).saveAppSettings,
    )
    saveMocks.mockResolvedValue({
      settings: makeSettings({ portRefreshIntervalMs: 60000 }),
      correctedNote: 'interval clamped to 60000 ms',
    })

    const input = screen.getByLabelText(/Refresh interval/i)
    fireEvent.change(input, { target: { value: '999999' } })
    fireEvent.keyDown(input, { key: 'Enter' })

    await waitFor(() =>
      expect(useSettingsStore.getState().correctedNote).toContain('clamped'),
    )
    expect(screen.getByText(/Invalid value corrected/i)).toBeInTheDocument()
  })

  it('shows a save failure as a dismissible alert', async () => {
    render(<SettingsPage />)
    await waitFor(() => expect(useSettingsStore.getState().loaded).toBe(true))

    const saveMocks = vi.mocked(
      (await import('@/services/native/ports')).saveAppSettings,
    )
    saveMocks.mockRejectedValue(new Error('disk full'))

    fireEvent.click(screen.getByRole('switch', { name: 'AI runtime polling' }))

    await waitFor(() =>
      expect(screen.getByText(/Failed to save: disk full/i)).toBeInTheDocument(),
    )
    expect(screen.getByRole('alert')).toBeInTheDocument()
  })

  it('reset requires confirmation and restores defaults', async () => {
    render(<SettingsPage />)
    await waitFor(() => expect(useSettingsStore.getState().loaded).toBe(true))

    fireEvent.click(screen.getByText('Reset to Defaults'))
    expect(screen.getByText(/Reset all settings to defaults\?/i)).toBeInTheDocument()

    fireEvent.click(screen.getByText('Confirm reset'))
    await waitFor(() =>
      expect(useSettingsStore.getState().saving).toBe(false),
    )
    expect(useSettingsStore.getState().settings?.closeBehavior).toBe('exit')
  })

  it('startup toggle drives the dedicated command and shows OS reality', async () => {
    render(<SettingsPage />)
    await waitFor(() => expect(useSettingsStore.getState().loaded).toBe(true))

    fireEvent.click(
      screen.getByRole('switch', { name: 'Run at Windows startup' }),
    )

    await waitFor(() =>
      expect(useSettingsStore.getState().settings?.startupRegistered).toBe(true),
    )
    expect(screen.getByText(/Registered with Windows/i)).toBeInTheDocument()
  })
})
