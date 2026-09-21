/**
 * Phase 11C Task 16 — diagnostics Zustand store.
 *
 * All actions call only the Task-15 native client (no invoke import —
 * safetyGuards). Deep-check progress is textual (spec §30); the recovery
 * banner survives until dismissed.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest'

const { client } = vi.hoisted(() => ({
  client: {
    getDiagnosticsOverview: vi.fn(),
    runDeepHealthChecks: vi.fn(),
    listIncidents: vi.fn(),
    markIncidentReviewed: vi.fn(),
    listSupportBundles: vi.fn(),
    markBundleReviewed: vi.fn(),
  },
}))

vi.mock('@/services/native/diagnostics', () => ({ ...client }))

import { useDiagnosticsStore } from './diagnosticsStore'
import type { HealthCheckResultDto, IncidentDto } from '@/types/domain'

function health(n: number): HealthCheckResultDto[] {
  return Array.from({ length: n }, (_, i) => ({
    id: `check-${i}`,
    subsystem: 'windows',
    label: `Probe ${i}`,
    status: 'healthy',
    code: '',
    summary: '',
    detail: null,
    checkedAtMs: 1,
    durationMs: 1,
  }))
}

function incident(over: Partial<IncidentDto> = {}): IncidentDto {
  return {
    incidentId: 'inc1',
    subsystem: 'docker',
    code: 'ERR_X',
    severity: 'severe',
    operation: 'probe',
    summary: 'synthetic',
    firstSeenMs: 1,
    lastSeenMs: 2,
    occurrences: 1,
    reviewState: 'new',
    bundleIds: [],
    ...over,
  }
}

describe('diagnosticsStore', () => {
  beforeEach(() => {
    vi.clearAllMocks()
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

  it('runDeepChecks exposes textual progress', async () => {
    client.runDeepHealthChecks.mockImplementation(
      async () =>
        new Promise((resolve) =>
          setTimeout(() => resolve(health(12)), 20),
        ),
    )
    const p = useDiagnosticsStore.getState().runDeepChecks()
    expect(useDiagnosticsStore.getState().deepCheckRunning).toBe(true)
    expect(useDiagnosticsStore.getState().deepCheckProgressText).toBe(
      'Running deep diagnostics…',
    )
    await p
    expect(useDiagnosticsStore.getState().deepCheckProgressText).toBe(
      'Running deep diagnostics… 12 of 12 checks complete',
    )
    expect(useDiagnosticsStore.getState().deepCheckRunning).toBe(false)
    expect(useDiagnosticsStore.getState().healthResults).toHaveLength(12)
  })

  it('banner state survives until dismissed', () => {
    useDiagnosticsStore.getState().setBanner('A previous crash was recovered into a support bundle.')
    expect(useDiagnosticsStore.getState().lastBanner).toBe(
      'A previous crash was recovered into a support bundle.',
    )
    useDiagnosticsStore.getState().dismissBanner()
    expect(useDiagnosticsStore.getState().lastBanner).toBeNull()
  })

  it('markReviewed updates incident review state locally', async () => {
    client.listIncidents.mockResolvedValue([incident()])
    client.markIncidentReviewed.mockResolvedValue(undefined)
    await useDiagnosticsStore.getState().loadIncidents()
    expect(useDiagnosticsStore.getState().incidents[0].reviewState).toBe('new')
    await useDiagnosticsStore.getState().markReviewed('inc1')
    expect(client.markIncidentReviewed).toHaveBeenCalledWith('inc1')
    expect(useDiagnosticsStore.getState().incidents[0].reviewState).toBe('reviewed')
  })

  it('overview load captures the recovery banner and pending-crash state', async () => {
    client.getDiagnosticsOverview.mockResolvedValue({
      appVersion: '1.0.1',
      overallHealth: 'healthy',
      lastDeepCheckMs: null,
      activeIncidents: 0,
      bundleCount: 0,
      storageBytes: 0,
      pendingCrashRecovery: false,
      recoveryBanner: 'A previous crash was recovered into a support bundle.',
    })
    await useDiagnosticsStore.getState().loadOverview()
    expect(useDiagnosticsStore.getState().overview).not.toBeNull()
    expect(useDiagnosticsStore.getState().recoveredFromCrash).toBe(true)
    expect(useDiagnosticsStore.getState().lastBanner).toBe(
      'A previous crash was recovered into a support bundle.',
    )
  })
})
