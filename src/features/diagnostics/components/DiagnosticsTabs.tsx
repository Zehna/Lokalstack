/** Tab bar with accessible tablist/tab semantics (spec §30). */
import type { DiagnosticsTab } from '../types'

const TABS: Array<{ id: DiagnosticsTab; label: string }> = [
  { id: 'overview', label: 'Overview' },
  { id: 'health', label: 'Health' },
  { id: 'incidents', label: 'Incidents' },
  { id: 'bundles', label: 'Support Bundles' },
]

export function DiagnosticsTabs({
  active,
  onChange,
}: {
  active: DiagnosticsTab
  onChange: (tab: DiagnosticsTab) => void
}) {
  return (
    <div role="tablist" aria-label="Diagnostics sections" className="flex gap-1 border-b border-slate-800">
      {TABS.map((t) => (
        <button
          key={t.id}
          role="tab"
          id={`tab-${t.id}`}
          aria-selected={active === t.id}
          aria-controls={`panel-${t.id}`}
          tabIndex={active === t.id ? 0 : -1}
          onClick={() => onChange(t.id)}
          className={`rounded-t px-3 py-2 text-sm ${
            active === t.id ? 'bg-slate-800 font-medium text-slate-100' : 'text-slate-400 hover:text-slate-200'
          }`}
        >
          {t.label}
        </button>
      ))}
    </div>
  )
}
