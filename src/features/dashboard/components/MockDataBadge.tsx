import { FlaskConical } from 'lucide-react'

/**
 * Small badge marking UI that renders mock data instead of data discovered
 * by the (future) Tauri engines. Phase 0 requires this label on anything
 * that pretends to show machine state.
 */
export function MockDataBadge({ label = 'mock data' }: { label?: string }) {
  return (
    <span
      title="Placeholder UI data — nothing was discovered on this machine"
      className="inline-flex items-center gap-1 rounded-full border border-amber-500/30 bg-amber-500/10 px-2 py-0.5 text-[11px] font-medium text-amber-400"
    >
      <FlaskConical className="h-3 w-3" strokeWidth={2} />
      {label}
    </span>
  )
}
