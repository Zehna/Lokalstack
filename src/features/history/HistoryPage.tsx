import { RefreshCw, Square, SquareX, TriangleAlert } from 'lucide-react'

import { RefreshButton } from '@/app/components/RefreshButton'
import { useControlStore } from '@/stores/controlStore'
import { usePortListeners } from '@/hooks'
import { usePortsStore } from '@/stores/portsStore'
import { formatTime } from '@/utils/format'

function actionIcon(action: string) {
  if (action === 'force_stop' || action === 'service_force_stop' || action === 'startup_failure') {
    return <SquareX className="h-3.5 w-3.5 text-red-400" strokeWidth={1.8} />
  }
  if (action === 'stop' || action === 'service_stop' || action === 'workspace_stop') {
    return <Square className="h-3.5 w-3.5 text-amber-400" strokeWidth={1.8} />
  }
  if (action === 'port_conflict' || action === 'conflict_detected' || action === 'dependency_unavailable' || action === 'workspace_blocked' || action === 'ai_runtime_unavailable' || action === 'docker_engine_unavailable' || action === 'container_stopped_observed' || action === 'container_health_unhealthy') {
    return <TriangleAlert className="h-3.5 w-3.5 text-amber-400" strokeWidth={1.8} />
  }
  return <RefreshCw className="h-3.5 w-3.5 text-sky-400" strokeWidth={1.8} />
}

const OUTCOME_STYLES: Record<string, string> = {
  success: 'border-emerald-900/60 bg-emerald-950/40 text-emerald-400',
  failure: 'border-amber-900/60 bg-amber-950/40 text-amber-400',
  stale: 'border-red-900/60 bg-red-950/40 text-red-400',
}

const ACTION_LABELS: Record<string, string> = {
  stop: 'Stop',
  force_stop: 'Force stop',
  open: 'Open',
  // Phase 6 managed-lifecycle events share the audit trail.
  workspace_start: 'Start ws',
  workspace_stop: 'Stop ws',
  service_start: 'Start svc',
  service_stop: 'Stop svc',
  service_force_stop: 'Force svc',
  service_restart: 'Restart',
  workspace_create: 'Create ws',
  startup_failure: 'Start fail',
  port_conflict: 'Conflict',
  // Phase 7 conflict/dependency intelligence (transition events).
  conflict_detected: 'Conflict',
  conflict_resolved: 'Conflict ok',
  dependency_unavailable: 'Dep down',
  dependency_recovered: 'Dep up',
  workspace_blocked: 'Blocked',
  workspace_recovered: 'Unblocked',
  free_port_suggested: 'Free ports',
  // Phase 8 AI runtime transitions (deduped, never per-poll).
  ai_runtime_ready: 'AI ready',
  ai_runtime_unavailable: 'AI down',
  ai_runtime_loading: 'AI loading',
  ai_model_loaded: 'Model in',
  ai_model_unloaded: 'Model out',
  // Phase 9 Docker observations (deduped, observed-not-initiated wording).
  docker_engine_available: 'Docker up',
  docker_engine_unavailable: 'Docker down',
  container_started_observed: 'Cont. up',
  container_stopped_observed: 'Cont. down',
  container_health_unhealthy: 'Unhealthy',
  container_health_recovered: 'Health ok',
}

/**
 * History view — this session's control actions with their outcomes.
 *
 * Observation history (services appearing/disappearing) still belongs to
 * Phase 10; what is real today is the audit trail of every stop / force
 * stop / open the user performed, including stale-target refusals.
 */
export function HistoryPage() {
  usePortListeners()
  const history = useControlStore((state) => state.history)
  const clearHistory = useControlStore((state) => state.clearHistory)
  const refreshing = usePortsStore((state) => state.refreshing)
  const refreshListeners = usePortsStore((state) => state.refreshListeners)

  return (
    <div className="mx-auto w-full max-w-5xl px-6 py-8">
      {/* Header */}
      <div className="mb-6">
        <div className="flex items-center justify-between">
          <h1 className="text-xl font-semibold text-slate-100">History</h1>
          <RefreshButton onClick={() => void refreshListeners()} refreshing={refreshing} />
        </div>
        <p className="mt-1 text-sm text-slate-500">
          Every control action from this session, with its outcome. The audit
          trail is kept in memory only — closing LocalStack clears it.
        </p>
      </div>

      {history.length === 0 ? (
        <div className="rounded-lg border border-dashed border-slate-800 p-10 text-center">
          <p className="text-sm text-slate-400">No control actions this session.</p>
          <p className="mt-1 text-xs text-slate-600">
            Stopping a development service or opening one in the browser will
            record it here.
          </p>
        </div>
      ) : (
        <ul className="overflow-hidden rounded-lg border border-slate-800">
          {[...history].reverse().map((entry) => (
            <li
              key={entry.id}
              className="flex items-center gap-3 border-b border-slate-800/60 bg-slate-900/60 px-4 py-2.5 last:border-0"
            >
              {actionIcon(entry.action)}
              <span className="w-20 shrink-0 rounded border border-slate-700 bg-slate-950 px-1.5 py-0.5 text-center text-xs text-slate-400">
                {ACTION_LABELS[entry.action] ?? entry.action}
              </span>
              <span className="text-sm font-medium text-slate-200">{entry.subject}</span>
              {entry.pid !== null && (
                <span className="font-mono text-xs text-slate-500">PID {entry.pid}</span>
              )}
              <span
                className={`rounded border px-1.5 py-0.5 text-xs ${OUTCOME_STYLES[entry.outcome] ?? ''}`}
              >
                {entry.outcome === 'stale' ? 'stale target' : entry.outcome}
              </span>
              <span className="flex-1 truncate text-xs text-slate-500" title={entry.message}>
                {entry.message}
              </span>
              <span className="shrink-0 font-mono text-xs text-slate-500">
                {formatTime(entry.at)}
              </span>
            </li>
          ))}
        </ul>
      )}

      {history.length > 0 && (
        <div className="mt-4">
          <button
            type="button"
            onClick={clearHistory}
            className="rounded border border-slate-700 bg-slate-900 px-3 py-1.5 text-xs text-slate-400 transition-colors hover:border-slate-600 hover:text-slate-200"
          >
            Clear history
          </button>
        </div>
      )}

      <p className="mt-3 text-xs text-slate-600">
        “stale target” entries are safety refusals: the process changed between
        discovery and the action, so LocalStack refused to act and no process
        was touched.
      </p>
    </div>
  )
}
