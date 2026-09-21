/**
 * Diagnostics page (Phase 11C Task 17) — tabbed UI with text statuses,
 * labelled filters, keyboard-accessible actions, recovery banner, and the
 * Full Forensics fresh-confirmation gate (user-intent/UX gate, spec §9 of
 * the amendment; never claimed as authorization).
 */
import { useEffect, useState } from 'react'

import { useDiagnosticsStore } from '@/stores/diagnosticsStore'
import type { ExportProfileDto } from '@/types/domain'
import { DiagnosticsTabs } from './components/DiagnosticsTabs'
import { OverviewTab } from './components/OverviewTab'
import { HealthTab } from './components/HealthTab'
import { IncidentsTab } from './components/IncidentsTab'
import { BundlesTab } from './components/BundlesTab'
import { NotificationBanner } from './components/NotificationBanner'
import { ExportProfileDialog } from './components/ExportProfileDialog'
import { FullForensicsConfirm } from './components/FullForensicsConfirm'
import type { DiagnosticsTab } from './types'

export function DiagnosticsPage() {
  const [tab, setTab] = useState<DiagnosticsTab>('overview')
  const overview = useDiagnosticsStore((s) => s.overview)
  const healthResults = useDiagnosticsStore((s) => s.healthResults)
  const deepCheckRunning = useDiagnosticsStore((s) => s.deepCheckRunning)
  const deepCheckProgressText = useDiagnosticsStore((s) => s.deepCheckProgressText)
  const incidents = useDiagnosticsStore((s) => s.incidents)
  const bundles = useDiagnosticsStore((s) => s.bundles)
  const lastBanner = useDiagnosticsStore((s) => s.lastBanner)
  const loadOverview = useDiagnosticsStore((s) => s.loadOverview)
  const runDeepChecks = useDiagnosticsStore((s) => s.runDeepChecks)
  const loadIncidents = useDiagnosticsStore((s) => s.loadIncidents)
  const markReviewed = useDiagnosticsStore((s) => s.markReviewed)
  const loadBundles = useDiagnosticsStore((s) => s.loadBundles)
  const markBundleReviewed = useDiagnosticsStore((s) => s.markBundleReviewed)
  const setBanner = useDiagnosticsStore((s) => s.setBanner)
  const dismissBanner = useDiagnosticsStore((s) => s.dismissBanner)

  // Export dialog state (fresh per-export confirmation, spec §9).
  const [exportTarget, setExportTarget] = useState<string | null>(null)
  const [dialogOpen, setDialogOpen] = useState(false)
  const [profile, setProfile] = useState<ExportProfileDto>('safe-share')
  const [confirmOpen, setConfirmOpen] = useState(false)
  const [exportError, setExportError] = useState<string | null>(null)

  useEffect(() => {
    void loadOverview()
    void loadIncidents()
    void loadBundles()
  }, [loadOverview, loadIncidents, loadBundles])

  async function runExport(confirmed: boolean) {
    if (!exportTarget) return
    setConfirmOpen(false)
    try {
      const { exportSupportBundle: doExport } = await import('@/services/native/diagnostics')
      const outcome = await doExport(exportTarget, profile, confirmed)
      setExportError(null)
      setExportTarget(null)
      setBanner(`Export ready: ${outcome.fileName}`)
    } catch (e) {
      setExportError(e instanceof Error ? e.message : String(e))
    }
  }

  function requestExport(_target: string, chosen: ExportProfileDto) {
    setExportError(null)
    setProfile(chosen)
    setDialogOpen(false)
    if (chosen === 'full-forensics') {
      // Fresh per-export confirmation — user-intent gate (spec §9/§12-C).
      setConfirmOpen(true)
      return
    }
    void runExport(false)
  }

  return (
    <div className="flex h-full flex-col gap-4 p-4" data-testid="diagnostics-page">
      <NotificationBanner message={lastBanner} onDismiss={dismissBanner} />
      {exportError !== null && (
        <div role="alert" className="rounded border border-red-700 bg-red-950/60 px-3 py-2 text-sm text-red-200">
          Export failed: {exportError}
        </div>
      )}
      <DiagnosticsTabs active={tab} onChange={setTab} />
      {tab === 'overview' && (
        <div role="tabpanel" id="panel-overview" aria-labelledby="tab-overview">
        <OverviewTab
          overview={overview}
          deepCheckRunning={deepCheckRunning}
          deepCheckProgressText={deepCheckProgressText}
          onRunDeepChecks={() => void runDeepChecks()}
        />
        </div>
      )}
      {tab === 'health' && (
        <div role="tabpanel" id="panel-health" aria-labelledby="tab-health">
          <HealthTab results={healthResults} />
        </div>
      )}
      {tab === 'incidents' && (
        <div role="tabpanel" id="panel-incidents" aria-labelledby="tab-incidents">
          <IncidentsTab incidents={incidents} onMarkReviewed={(id) => void markReviewed(id)} />
        </div>
      )}
      {tab === 'bundles' && (
        <div role="tabpanel" id="panel-bundles" aria-labelledby="tab-bundles">
        <BundlesTab
          bundles={bundles}
          onExport={(id) => {
            setExportTarget(id)
            setExportError(null)
            setDialogOpen(true)
          }}
          onDelete={(id) => {
            void import('@/services/native/diagnostics').then(({ deleteSupportBundle }) =>
              deleteSupportBundle(id).catch(() => {}),
            )
          }}
          onMarkReviewed={markBundleReviewed}
        />
        </div>
      )}
      {confirmOpen && (
        <FullForensicsConfirm
          onCancel={() => {
            setConfirmOpen(false)
            setExportTarget(null)
          }}
          onConfirm={() => void runExport(true)}
        />
      )}
      {dialogOpen && (
        <ExportProfileDialog
          open
          bundleId={exportTarget}
          onSelect={(p) => requestExport(exportTarget ?? '', p)}
          onCancel={() => setExportTarget(null)}
        />
      )}
    </div>
  )
}
