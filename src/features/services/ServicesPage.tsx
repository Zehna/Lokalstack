import { useMemo } from 'react'

import { RefreshButton } from '@/app/components/RefreshButton'
import { usePortListeners } from '@/hooks'
import { usePortsStore } from '@/stores/portsStore'
import { formatBytes, formatCpuPercent, formatTime } from '@/utils/format'
import { groupListenersByProcess } from './groupProcesses'

/**
 * Services view — ACTIVE LOCAL PROCESSES.
 *
 * Real data: listener rows grouped by owning PID (the domain adapter in
 * `groupProcesses.ts`), merged with the Phase 2 process intelligence from
 * the native snapshot. No framework/service identity is claimed here —
 * a process is called by its executable name or, when Windows refuses
 * access, honestly "Unavailable". Classification arrives in Phase 3.
 */
export function ServicesPage() {
  usePortListeners()
  const listeners = usePortsStore((state) => state.listeners)
  const processByPid = usePortsStore((state) => state.processByPid)
  const loading = usePortsStore((state) => state.loading)
  const refreshing = usePortsStore((state) => state.refreshing)
  const error = usePortsStore((state) => state.error)
  const lastUpdated = usePortsStore((state) => state.lastUpdated)
  const refreshListeners = usePortsStore((state) => state.refreshListeners)

  const groups = useMemo(
    () => groupListenersByProcess(listeners, processByPid),
    [listeners, processByPid],
  )

  return (
    <div className="mx-auto w-full max-w-5xl px-6 py-8">
      {/* Header */}
      <div className="mb-6">
        <div className="flex items-center justify-between">
          <h1 className="text-xl font-semibold text-slate-100">Services</h1>
          <RefreshButton onClick={() => void refreshListeners()} refreshing={refreshing} />
        </div>
        <p className="mt-1 text-sm text-slate-500">
          Active local processes behind the discovered listeners. Names are executable
          basenames from Windows — service identities (Next.js, Flask, PostgreSQL, …)
          arrive with Phase 3 detection, so nothing is guessed here.
        </p>
      </div>

      <div className="mb-3 flex items-center justify-between">
        <h2 className="text-sm font-semibold uppercase tracking-wider text-slate-400">
          Active Local Processes
        </h2>
        <span className="text-xs text-slate-500">
          {loading ? '…' : `${groups.length} process${groups.length === 1 ? '' : 'es'}`} · auto-refresh 3s · updated{' '}
          {formatTime(lastUpdated)}
        </span>
      </div>

      {error !== null ? (
        <div className="rounded-lg border border-red-900/60 bg-red-950/30 px-4 py-3 text-sm text-red-300">
          <p className="font-medium">Discovery error</p>
          <p className="mt-1 text-xs text-red-400/80">{error}</p>
          <p className="mt-2 text-xs text-red-400/60">
            Data may be stale below — the last successful snapshot is kept.
          </p>
        </div>
      ) : null}

      {loading ? (
        <div className="rounded-lg border border-slate-800 bg-slate-900/40 px-4 py-10 text-center text-sm text-slate-500">
          Reading TCP listener tables and process metadata…
        </div>
      ) : groups.length === 0 ? (
        <div className="rounded-lg border border-dashed border-slate-800 p-10 text-center">
          <p className="text-sm text-slate-400">No active processes with listening ports.</p>
          <p className="mt-1 text-xs text-slate-600">
            Start a dev server — it will appear here automatically within ~3 seconds.
          </p>
        </div>
      ) : (
        <ul className="overflow-hidden rounded-lg border border-slate-800">
          {groups.map((group, index) => (
            <li
              key={group.pid}
              className={`bg-slate-900/60 px-4 py-3 ${index > 0 ? 'border-t border-slate-800' : ''}`}
            >
              <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
                <span
                  className={`h-2 w-2 shrink-0 rounded-full ${
                    group.accessible ? 'bg-emerald-400' : 'bg-slate-500'
                  }`}
                  title={
                    group.accessible
                      ? 'Process inspected successfully'
                      : 'Process metadata unavailable (Windows denied access or process is gone)'
                  }
                />

                <span
                  className={`truncate font-mono text-sm font-medium ${
                    group.accessible ? 'text-slate-200' : 'text-slate-400'
                  }`}
                  title={
                    group.process?.executablePath ??
                    'Executable path not available for this process'
                  }
                >
                  {group.displayName}
                </span>

                <span className="font-mono text-xs text-slate-500">PID {group.pid}</span>

                <span className="flex-1 truncate font-mono text-xs text-slate-500">
                  {group.ports.length > 0
                    ? `Ports ${group.ports.join(', ')}`
                    : 'No ports'}
                  {' · '}
                  {group.addresses.length > 0 ? group.addresses.join(', ') : ''}
                </span>

                <span
                  className="font-mono text-xs text-slate-400"
                  title={
                    group.cpuPercent === null
                      ? 'First sample — CPU is measured on the next refresh'
                      : 'CPU over the last refresh window, normalized per logical core'
                  }
                >
                  CPU {formatCpuPercent(group.cpuPercent)}
                </span>

                <span className="font-mono text-xs text-slate-400">
                  RAM {formatBytes(group.memoryBytes)}
                </span>
              </div>

              {group.process?.executablePath != null && (
                <p className="mt-1 truncate pl-5 font-mono text-xs text-slate-600">
                  {group.process.executablePath}
                </p>
              )}
            </li>
          ))}
        </ul>
      )}

      <p className="mt-3 text-xs text-slate-600">
        Multiple listener rows of one process (e.g. the same port on IPv4 and IPv6) are
        grouped into a single entry. Processes marked “Unavailable” keep their port and
        PID — Windows refused full inspection, which is normal for protected system
        processes. LocalStack never terminates or modifies any of these processes.
      </p>
    </div>
  )
}

/** Shared layout for Phase 0 placeholder views. */
export function PlaceholderPage({
  title,
  description,
  children,
}: {
  title: string
  description: string
  children?: React.ReactNode
}) {
  return (
    <div className="mx-auto w-full max-w-5xl px-6 py-8">
      <h1 className="text-xl font-semibold text-slate-100">{title}</h1>
      <p className="mt-1 mb-6 text-sm text-slate-500">{description}</p>
      {children}
      <p className="mt-8 rounded-lg border border-slate-800 bg-slate-900/40 px-4 py-3 text-xs text-slate-500">
        This view arrives in a later phase — the shell reserves the layout today.
      </p>
    </div>
  )
}
