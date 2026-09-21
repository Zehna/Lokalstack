/**
 * Phase 11C Task 15 — typed diagnostics native client.
 *
 * Pins the exact Task-14 command names and argument shapes at the invoke
 * boundary. This module is the ONLY new production `invoke` callsite; the
 * safetyGuards static test keeps the boundary intact.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest'

const { invokeMock } = vi.hoisted(() => ({
  invokeMock: vi.fn(async (cmd: string) => ({ cmd })),
}))

vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }))

import {
  deleteSupportBundle,
  exportSupportBundle,
  getDiagnosticsOverview,
  getSupportBundleDetail,
  getSupportSummary,
  listIncidents,
  listSupportBundles,
  markIncidentReviewed,
  openDiagnosticsFolder,
  prepareLocalstackGithubIssue,
  revealExportResult,
  runDeepHealthChecks,
  updateDiagnosticsContext,
} from './diagnostics'

describe('diagnostics native client', () => {
  beforeEach(() => {
    invokeMock.mockClear()
  })

  it('wraps exact backend command names with typed DTOs', async () => {
    await getDiagnosticsOverview()
    expect(invokeMock).toHaveBeenLastCalledWith('get_diagnostics_overview')

    await runDeepHealthChecks()
    expect(invokeMock).toHaveBeenLastCalledWith('run_deep_health_checks')

    await listIncidents()
    expect(invokeMock).toHaveBeenLastCalledWith('list_incidents')

    await markIncidentReviewed('abc123')
    expect(invokeMock).toHaveBeenLastCalledWith('mark_incident_reviewed', {
      incidentId: 'abc123',
    })

    await listSupportBundles()
    expect(invokeMock).toHaveBeenLastCalledWith('list_support_bundles')

    await getSupportBundleDetail('abc123')
    expect(invokeMock).toHaveBeenLastCalledWith('get_support_bundle_detail', {
      bundleId: 'abc123',
    })

    await deleteSupportBundle('abc123')
    expect(invokeMock).toHaveBeenLastCalledWith('delete_support_bundle', {
      bundleId: 'abc123',
    })

    await openDiagnosticsFolder()
    expect(invokeMock).toHaveBeenLastCalledWith('open_diagnostics_folder')

    await getSupportSummary(undefined)
    expect(invokeMock).toHaveBeenLastCalledWith('get_support_summary', {
      bundleId: undefined,
    })

    await getSupportSummary('abc123')
    expect(invokeMock).toHaveBeenLastCalledWith('get_support_summary', {
      bundleId: 'abc123',
    })

    await prepareLocalstackGithubIssue('abc123')
    expect(invokeMock).toHaveBeenLastCalledWith('prepare_localstack_github_issue', {
      bundleId: 'abc123',
    })

    await revealExportResult('cap456')
    expect(invokeMock).toHaveBeenLastCalledWith('reveal_export_result', {
      exportId: 'cap456',
    })
  })

  it('exportSupportBundle sends the profile and confirmation flag', async () => {
    await exportSupportBundle('abc123', 'safe-share', false)
    expect(invokeMock).toHaveBeenLastCalledWith('export_support_bundle', {
      bundleId: 'abc123',
      profile: 'safe-share',
      confirmed: false,
    })
  })

  it('updateDiagnosticsContext sends the validated ViewId position', async () => {
    await updateDiagnosticsContext('diagnostics')
    // ViewId positional discriminant = NAV_ITEMS index (Task 16 adds the
    // 'diagnostics' entry at index 9; matches Rust NAV_LEN = 10).
    expect(invokeMock).toHaveBeenLastCalledWith('update_diagnostics_context', {
      viewId: 9,
    })

    await updateDiagnosticsContext('dashboard')
    expect(invokeMock).toHaveBeenLastCalledWith('update_diagnostics_context', {
      viewId: 0,
    })
  })
})
