import { RefreshCw } from 'lucide-react'

interface RefreshButtonProps {
  onClick: () => void
  refreshing: boolean
  label?: string
}

/** Manual-refresh button with a spinning state while a request is in flight. */
export function RefreshButton({ onClick, refreshing, label = 'Refresh' }: RefreshButtonProps) {
  return (
    <button
      type="button"
      onClick={onClick}
      disabled={refreshing}
      className="inline-flex items-center gap-1.5 rounded-md border border-slate-700 bg-slate-900 px-2.5 py-1.5 text-xs font-medium text-slate-300 transition-colors hover:border-slate-600 hover:bg-slate-800 disabled:cursor-not-allowed disabled:opacity-50"
    >
      <RefreshCw className={`h-3.5 w-3.5 ${refreshing ? 'animate-spin' : ''}`} strokeWidth={1.8} />
      {label}
    </button>
  )
}
