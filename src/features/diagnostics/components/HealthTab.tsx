/** Health tab: grouped check results; status is ALWAYS text (spec §30). */
import type { HealthCheckResultDto } from '@/types/domain'

const GROUPS = ['localstack', 'integrations', 'windows'] as const

export function HealthTab({ results }: { results: HealthCheckResultDto[] }) {
  return (
    <section aria-label="Deep health results">
      {results.length === 0 && (
        <p className="text-sm text-slate-500">Run deep diagnostics to collect health checks.</p>
      )}
      {GROUPS.map((group) => {
        const groupResults = results.filter((r) => r.subsystem === group)
        if (groupResults.length === 0) return null
        return (
          <div key={group} className="mb-4">
            <h3 className="mb-2 text-xs font-semibold uppercase tracking-wide text-slate-500">
              {group}
            </h3>
            <ul className="divide-y divide-slate-800/60 rounded border border-slate-800">
              {groupResults.map((r) => (
                <li key={r.id} className="flex items-start justify-between gap-4 px-4 py-2.5">
                  <div className="min-w-0">
                    <p className="text-sm font-medium text-slate-200">{r.label}</p>
                    <p className="text-xs text-slate-500">
                      {r.summary}
                      {r.detail ? ` — ${r.detail}` : ''}
                    </p>
                  </div>
                  <div className="shrink-0 text-right">
                    {/* Status as text, never color-only (spec §30). */}
                    <span className="text-sm font-medium text-slate-100">{statusText(r.status)}</span>
                    <p className="text-xs text-slate-500">{r.durationMs} ms</p>
                  </div>
                </li>
              ))}
            </ul>
          </div>
        )
      })}
    </section>
  )
}

function statusText(status: string): string {
  switch (status) {
    case 'healthy':
      return 'Healthy'
    case 'degraded':
      return 'Degraded'
    case 'unavailable':
      return 'Unavailable'
    case 'skipped':
      return 'Skipped'
    default:
      return status
  }
}
