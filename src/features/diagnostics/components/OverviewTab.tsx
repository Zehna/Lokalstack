/** Overview tab: aggregate state + deep-diagnostics trigger (spec §19). */
import type { DiagnosticsOverviewDto } from '@/types/domain'

export function OverviewTab({
  overview,
  deepCheckRunning,
  deepCheckProgressText,
  onRunDeepChecks,
}: {
  overview: DiagnosticsOverviewDto | null
  deepCheckRunning: boolean
  deepCheckProgressText: string
  onRunDeepChecks: () => void
}) {
  return (
    <section aria-label="Diagnostics overview" className="grid gap-3">
      <div className="grid grid-cols-2 gap-3 md:grid-cols-4">
        <StatCard label="Overall health" value={overview?.overallHealth ?? 'not-run'} />
        <StatCard label="App version" value={overview?.appVersion ?? '…'} />
        <StatCard label="Active incidents" value={String(overview?.activeIncidents ?? 0)} />
        <StatCard label="Support bundles" value={String(overview?.bundleCount ?? 0)} />
        <StatCard label="Storage used" value={`${Math.ceil((overview?.storageBytes ?? 0) / 1024)} KiB`} />
        <StatCard
          label="Pending crash recovery"
          value={overview?.pendingCrashRecovery ? 'Pending' : 'None'}
        />
        <StatCard
          label="Last deep check"
          value={overview?.lastDeepCheckMs ? new Date(overview.lastDeepCheckMs).toLocaleString() : 'never'}
        />
      </div>
      <div className="flex items-center gap-3">
        <button
          type="button"
          onClick={onRunDeepChecks}
          disabled={deepCheckRunning}
          className="rounded bg-sky-700 px-3 py-1.5 text-sm text-white hover:bg-sky-600 disabled:opacity-50"
        >
          Run Deep Diagnostics
        </button>
        {/* Deep-check progress is textual (spec §30). */}
        <p aria-live="polite" className="text-sm text-slate-400" data-testid="deep-check-progress">
          {deepCheckRunning || deepCheckProgressText ? deepCheckProgressText : ''}
        </p>
      </div>
    </section>
  )
}

function StatCard({ label, value }: { label: string; value: string }) {
  return (
    <div className="rounded border border-slate-800 bg-slate-900/40 p-3">
      <p className="text-xs uppercase tracking-wide text-slate-500">{label}</p>
      <p className="mt-1 text-sm font-medium text-slate-100">{value}</p>
    </div>
  )
}
