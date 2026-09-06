import type { ReactNode } from 'react'

interface SummaryCardProps {
  title: string
  icon?: ReactNode
  value: string
  detail?: string
  /** Optional extra content below the footer line (e.g. a usage meter). */
  footer?: ReactNode
}

/** Placeholder summary card used on the dashboard. Presentation only. */
export function SummaryCard({ title, icon, value, detail, footer }: SummaryCardProps) {
  return (
    <div className="rounded-xl border border-slate-800 bg-slate-900/50 p-4">
      <div className="flex items-center justify-between">
        <span className="text-xs font-medium uppercase tracking-wider text-slate-500">{title}</span>
        {icon}
      </div>
      <div className="mt-2 flex items-baseline gap-2">
        <span className="text-2xl font-semibold tabular-nums text-slate-100">{value}</span>
        {detail ? <span className="text-xs text-slate-500">{detail}</span> : null}
      </div>
      {footer}
    </div>
  )
}
