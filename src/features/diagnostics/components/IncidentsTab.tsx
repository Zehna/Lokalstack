/** Incidents tab: labelled filter over loaded metadata (spec §19/§24). */
import { useMemo, useState } from 'react'
import type { IncidentDto } from '@/types/domain'

export function IncidentsTab({
  incidents,
  onMarkReviewed,
}: {
  incidents: IncidentDto[]
  onMarkReviewed: (incidentId: string) => void
}) {
  const [filter, setFilter] = useState('')
  const filtered = useMemo(() => {
    const q = filter.trim().toLowerCase()
    if (!q) return incidents
    return incidents.filter(
      (i) =>
        i.subsystem.toLowerCase().includes(q) ||
        i.code.toLowerCase().includes(q) ||
        i.summary.toLowerCase().includes(q) ||
        i.severity.toLowerCase().includes(q),
    )
  }, [incidents, filter])

  return (
    <section aria-label="Incident history">
      <div className="mb-3">
        <label htmlFor="incident-filter" className="block text-xs font-medium text-slate-400">
          Filter incidents
        </label>
        <input
          id="incident-filter"
          type="text"
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          className="mt-1 w-72 rounded border border-slate-700 bg-slate-900 px-2 py-1 text-sm text-slate-100"
          placeholder="subsystem, code, severity…"
        />
      </div>
      {filtered.length === 0 ? (
        <p className="text-sm text-slate-500">No incidents match.</p>
      ) : (
        <ul className="divide-y divide-slate-800/60 rounded border border-slate-800">
          {filtered.map((i) => (
            <li key={i.incidentId} className="flex items-start justify-between gap-4 px-4 py-3">
              <div className="min-w-0">
                <p className="text-sm font-medium text-slate-200">
                  {i.subsystem} · {i.code} · {i.severity}
                </p>
                <p className="text-xs text-slate-500">{i.summary}</p>
                <p className="mt-1 text-xs text-slate-500">
                  Occurrences: {i.occurrences} · Review status: {i.reviewState === 'new' ? 'New' : 'Reviewed'}
                </p>
                {i.bundleIds.length > 0 && (
                  <p className="text-xs text-slate-500">Bundles: {i.bundleIds.length}</p>
                )}
              </div>
              {i.reviewState === 'new' && (
                <button
                  type="button"
                  onClick={() => onMarkReviewed(i.incidentId)}
                  className="shrink-0 rounded border border-slate-700 px-2 py-1 text-xs text-slate-200 hover:bg-slate-800"
                >
                  Mark Reviewed
                </button>
              )}
            </li>
          ))}
        </ul>
      )}
    </section>
  )
}
