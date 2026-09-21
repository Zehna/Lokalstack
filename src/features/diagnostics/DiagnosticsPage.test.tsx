/**
 * Phase 11C Task 17 — Diagnostics page: tabs, accessibility, Full Forensics
 * confirmation UX, banner, and typed error surfaces (spec §24/§30).
 *
 * All backend access is mocked at the native-client seam; the page itself
 * consumes only the Task-16 store.
 */
// @vitest-environment jsdom
import { cleanup, render, screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const { client } = vi.hoisted(() => ({
  client: {
    getDiagnosticsOverview: vi.fn(),
    runDeepHealthChecks: vi.fn(),
    listIncidents: vi.fn(),
    markIncidentReviewed: vi.fn(),
    listSupportBundles: vi.fn(),
    exportSupportBundle: vi.fn(),
    deleteSupportBundle: vi.fn(),
    openDiagnosticsFolder: vi.fn(),
    getSupportSummary: vi.fn(),
    prepareLocalstackGithubIssue: vi.fn(),
    updateDiagnosticsContext: vi.fn(),
    revealExportResult: vi.fn(),
  },
}))

vi.mock('@/services/native/diagnostics', () => ({ ...client }))

import { DiagnosticsPage } from './DiagnosticsPage'
import { useDiagnosticsStore } from '@/stores/diagnosticsStore'
import type {
  BundleMetaDto,
  DiagnosticsOverviewDto,
  HealthCheckResultDto,
  IncidentDto,
} from '@/types/domain'

const overview: DiagnosticsOverviewDto = {
  appVersion: '1.0.1',
  overallHealth: 'healthy',
  lastDeepCheckMs: null,
  activeIncidents: 1,
  bundleCount: 1,
  storageBytes: 2048,
  pendingCrashRecovery: false,
  recoveryBanner: 'A previous crash was recovered into a support bundle.',
}

const checks: HealthCheckResultDto[] = [
  { id: 'c1', subsystem: 'windows', label: 'WebView2', status: 'healthy', code: '', summary: 'ok', detail: null, checkedAtMs: 1, durationMs: 2 },
  { id: 'c2', subsystem: 'integrations', label: 'Docker engine', status: 'degraded', code: 'D1', summary: 'slow', detail: null, checkedAtMs: 1, durationMs: 3 },
  { id: 'c3', subsystem: 'localstack', label: 'Settings file', status: 'skipped', code: '', summary: 'n/a', detail: null, checkedAtMs: 1, durationMs: 0 },
]

const incident: IncidentDto = {
  incidentId: 'inc1',
  subsystem: 'docker',
  code: 'ERR_X',
  severity: 'severe',
  operation: 'probe',
  summary: 'synthetic severe summary',
  firstSeenMs: 100,
  lastSeenMs: 200,
  occurrences: 42,
  reviewState: 'new',
  bundleIds: ['b1'],
}

const bundle: BundleMetaDto = {
  bundleId: 'b1',
  createdAtMs: 1234,
  trigger: 'severe-incident:ERR_X',
  subsystem: 'docker',
  severity: 'severe',
  fingerprint: 'fp123',
  encryptedSizeBytes: 4096,
  integrity: 'valid',
  reviewState: 'new',
  appVersion: '1.0.1',
  schemaVersion: 1,
}

describe('DiagnosticsPage', () => {
  afterEach(() => {
    // No global RTL auto-cleanup in this setup: Settings tests clean up
    // explicitly; page state must not leak between renders.
    cleanup()
  })

  beforeEach(async () => {
    vi.clearAllMocks()
    client.getDiagnosticsOverview.mockResolvedValue(overview)
    client.runDeepHealthChecks.mockResolvedValue(checks)
    client.listIncidents.mockResolvedValue([incident])
    client.listSupportBundles.mockResolvedValue([bundle])
    useDiagnosticsStore.setState({
      overview: null,
      healthResults: [],
      incidents: [],
      bundles: [],
      deepCheckRunning: false,
      deepCheckProgressText: '',
      lastBanner: null,
      recoveredFromCrash: false,
    })
  })

  it('exposes tabbed navigation with accessible names', async () => {
    render(<DiagnosticsPage />)
    await screen.findByRole('tab', { name: 'Overview' })
    expect(screen.getByRole('tab', { name: 'Health' })).toBeInTheDocument()
    expect(screen.getByRole('tab', { name: 'Incidents' })).toBeInTheDocument()
    expect(screen.getByRole('tab', { name: 'Support Bundles' })).toBeInTheDocument()
    expect(screen.getByRole('tablist')).toBeInTheDocument()
  })

  it('health status is text, not color-only', async () => {
    const user = userEvent.setup()
    render(<DiagnosticsPage />)
    // Deep checks run only on explicit action (spec §23 quick-vs-deep).
    await user.click(await screen.findByRole('button', { name: /run deep diagnostics/i }))
    await user.click(await screen.findByRole('tab', { name: 'Health' }))
    const panel = await screen.findByRole('tabpanel', { name: /health/i })
    await within(panel).findByText('Healthy')
    expect(within(panel).getByText('Degraded')).toBeInTheDocument()
    expect(within(panel).getByText('Skipped')).toBeInTheDocument()
  })

  it('full forensics requires fresh confirmation per export', async () => {
    const user = userEvent.setup()
    client.exportSupportBundle.mockResolvedValue({ exportId: 'cap1', fileName: 'x.zip', profile: 'full-forensics' })
    render(<DiagnosticsPage />)
    await user.click(await screen.findByRole('tab', { name: 'Support Bundles' }))
    await user.click(await screen.findByRole('button', { name: /export/i }))
    // Choose Full Forensics in the profile dialog.
    await user.click(await screen.findByRole('button', { name: /full forensics/i }))
    // The confirmation modal appears with the sensitive-category list.
    const dialog = await screen.findByRole('dialog')
    expect(within(dialog).getByText(/username/i)).toBeInTheDocument()
    expect(within(dialog).getByText(/machine identifier/i)).toBeInTheDocument()
    // Cancel aborts: no export call.
    await user.click(within(dialog).getByRole('button', { name: /cancel/i }))
    expect(client.exportSupportBundle).not.toHaveBeenCalled()
    // Re-open, confirm: export is called with confirmed=true.
    await user.click(await screen.findByRole('button', { name: /export/i }))
    await user.click(await screen.findByRole('button', { name: /full forensics/i }))
    await user.click(within(await screen.findByRole('dialog')).getByRole('button', { name: /confirm/i }))
    expect(client.exportSupportBundle).toHaveBeenCalledWith('b1', 'full-forensics', true)
  })

  it('export failure surfaces typed error without crashing', async () => {
    const user = userEvent.setup()
    client.exportSupportBundle.mockRejectedValue(new Error('full-forensics-unconfirmed'))
    render(<DiagnosticsPage />)
    await user.click(await screen.findByRole('tab', { name: 'Support Bundles' }))
    await user.click(await screen.findByRole('button', { name: /export/i }))
    await user.click(await screen.findByRole('button', { name: /safe share/i }))
    const alert = await screen.findByRole('alert')
    expect(alert.textContent).toContain('full-forensics-unconfirmed')
    // Page intact: tabs still present.
    expect(screen.getByRole('tab', { name: 'Health' })).toBeInTheDocument()
  })

  it('banner announces recovery via role=status', async () => {
    render(<DiagnosticsPage />)
    const status = await screen.findByRole('status')
    expect(status.textContent).toContain('recovered into a support bundle')
    const user = userEvent.setup()
    await user.click(screen.getByRole('button', { name: /dismiss/i }))
    expect(screen.queryByRole('status')).toBeNull()
  })

  it('incidents render occurrence counts and review state as text', async () => {
    const user = userEvent.setup()
    render(<DiagnosticsPage />)
    await user.click(await screen.findByRole('tab', { name: 'Incidents' }))
    const panel = await screen.findByRole('tabpanel', { name: /incidents/i })
    expect(await within(panel).findByText(/Occurrences: 42/)).toBeInTheDocument()
    expect(within(panel).getAllByText(/review status: new/i).length).toBeGreaterThan(0)
    expect(within(panel).getByText(/ERR_X/)).toBeInTheDocument()
  })

  it('search filter operates on loaded incident metadata only', async () => {
    const user = userEvent.setup()
    render(<DiagnosticsPage />)
    await user.click(await screen.findByRole('tab', { name: 'Incidents' }))
    const filter = await screen.findByLabelText(/filter incidents/i)
    await user.type(filter, 'nomatch-xyz')
    const panel = screen.getByRole('tabpanel', { name: /incidents/i })
    expect(within(panel).queryByText(/ERR_X/)).toBeNull()
    await user.clear(filter)
    await user.type(filter, 'ERR_X')
    expect(within(panel).getByText(/ERR_X/)).toBeInTheDocument()
  })
})
