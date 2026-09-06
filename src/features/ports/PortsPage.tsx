import { useMemo, useState } from 'react'
import { ArrowUpDown, Search } from 'lucide-react'

import { RefreshButton } from '@/app/components/RefreshButton'
import { usePortListeners } from '@/hooks'
import { usePortsStore } from '@/stores/portsStore'
import type { PortListener, ProcessInfo, ProjectIdentity, ServiceIdentity } from '@/types/domain'
import { formatBytes, formatCpuPercent, formatTime } from '@/utils/format'

/** Column keys the table can sort by. Only port sorting is required in Phase 1. */
type SortDirection = 'asc' | 'desc'

function listenerMatches(
  listener: PortListener,
  process: ProcessInfo | undefined,
  identity: ServiceIdentity | undefined,
  project: ProjectIdentity | undefined,
  query: string,
): boolean {
  const q = query.trim().toLowerCase()
  if (q === '') return true
  return (
    String(listener.port).includes(q) ||
    String(listener.pid).includes(q) ||
    listener.localAddress.toLowerCase().includes(q) ||
    (process?.name?.toLowerCase().includes(q) ?? false) ||
    (identity?.displayName.toLowerCase().includes(q) ?? false) ||
    (project?.name.toLowerCase().includes(q) ?? false) ||
    (project?.rootPath.toLowerCase().includes(q) ?? false) ||
    (project?.git.branch?.toLowerCase().includes(q) ?? false)
  )
}

function compareListeners(a: PortListener, b: PortListener, direction: SortDirection): number {
  const byPort = a.port - b.port
  if (byPort !== 0) return direction === 'asc' ? byPort : -byPort
  // Stable tiebreakers so refreshes don't shuffle equal-port rows.
  const byAddress = a.localAddress.localeCompare(b.localAddress)
  if (byAddress !== 0) return byAddress
  return a.pid - b.pid
}

/**
 * Ports view — the real, live table of every TCP listener Windows reports,
 * enriched with process intelligence (name, CPU, memory). Data comes from
 * the ports store (native discovery + process sampling, auto-refresh ~3 s);
 * this component only presents, filters and sorts it.
 */
export function PortsPage() {
  usePortListeners()
  const listeners = usePortsStore((state) => state.listeners)
  const processByPid = usePortsStore((state) => state.processByPid)
  const serviceByPid = usePortsStore((state) => state.serviceByPid)
  const projectByPid = usePortsStore((state) => state.projectByPid)
  const loading = usePortsStore((state) => state.loading)
  const refreshing = usePortsStore((state) => state.refreshing)
  const error = usePortsStore((state) => state.error)
  const lastUpdated = usePortsStore((state) => state.lastUpdated)
  const durationMs = usePortsStore((state) => state.durationMs)
  const refreshListeners = usePortsStore((state) => state.refreshListeners)

  const [query, setQuery] = useState('')
  const [direction, setDirection] = useState<SortDirection>('asc')

  const visible = useMemo(
    () =>
      listeners
        .filter((listener) =>
          listenerMatches(
            listener,
            processByPid.get(listener.pid),
            serviceByPid.get(listener.pid),
            projectByPid.get(listener.pid),
            query,
          ),
        )
        .sort((a, b) => compareListeners(a, b, direction)),
    [listeners, processByPid, serviceByPid, projectByPid, query, direction],
  )

  return (
    <div className="mx-auto w-full max-w-6xl px-6 py-8">
      {/* Header */}
      <div className="mb-6">
        <div className="flex items-center justify-between">
          <h1 className="text-xl font-semibold text-slate-100">Ports</h1>
          <RefreshButton onClick={() => void refreshListeners()} refreshing={refreshing} />
        </div>
        <p className="mt-1 text-sm text-slate-500">
          Every TCP listener on this machine with its owning process, discovered live
          from Windows. Refreshes automatically every ~3 seconds
          {durationMs !== null ? ` (native cycle: ${durationMs} ms)` : ''}.
        </p>
        <p className="mt-0.5 text-xs text-slate-600">
          Unavailable process data means Windows refused inspection (normal for
          protected system processes) or the process exited between samples — the
          listener itself is always real.
        </p>
      </div>

      {/* Toolbar */}
      <div className="mb-4 flex flex-wrap items-center gap-3">
        <div className="relative min-w-56 flex-1">
          <Search className="pointer-events-none absolute left-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-slate-500" />
          <input
            type="text"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            placeholder="Search port, service, project, process, PID, branch, or address…"
            className="w-full rounded-md border border-slate-800 bg-slate-900 py-1.5 pl-8 pr-3 text-sm text-slate-200 placeholder:text-slate-600 focus:border-slate-600 focus:outline-none"
          />
        </div>
        <span className="text-xs text-slate-500">
          {loading ? '…' : `${visible.length} of ${listeners.length}`}
        </span>
        <span className="text-xs text-slate-500">updated {formatTime(lastUpdated)}</span>
      </div>

      {/* States */}
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
          Reading TCP listener tables…
        </div>
      ) : listeners.length === 0 ? (
        <div className="rounded-lg border border-dashed border-slate-800 p-10 text-center">
          <p className="text-sm text-slate-400">No TCP listeners found on this machine.</p>
          <p className="mt-1 text-xs text-slate-600">
            Start a dev server — it will appear here automatically within ~3 seconds.
          </p>
        </div>
      ) : visible.length === 0 ? (
        <div className="rounded-lg border border-dashed border-slate-800 p-10 text-center">
          <p className="text-sm text-slate-400">No listeners match “{query}”.</p>
          <p className="mt-1 text-xs text-slate-600">
            Search matches port numbers, service and project names, PIDs, git branches
            and bind addresses.
          </p>
        </div>
      ) : (
        <div className="overflow-x-auto rounded-lg border border-slate-800">
          <table className="w-full text-left text-sm">
            <thead>
              <tr className="border-b border-slate-800 bg-slate-900/80 text-xs uppercase tracking-wider text-slate-500">
                <th scope="col" className="px-4 py-2.5 font-medium">
                  <button
                    type="button"
                    onClick={() => setDirection((d) => (d === 'asc' ? 'desc' : 'asc'))}
                    className="inline-flex items-center gap-1 uppercase tracking-wider hover:text-slate-300"
                    title="Toggle ascending / descending port order"
                  >
                    Port
                    <ArrowUpDown className="h-3 w-3" strokeWidth={1.8} />
                    <span className="lowercase">{direction === 'asc' ? '↑' : '↓'}</span>
                  </button>
                </th>
                <th scope="col" className="px-4 py-2.5 font-medium">Service</th>
                <th scope="col" className="px-4 py-2.5 font-medium">Project</th>
                <th scope="col" className="px-4 py-2.5 font-medium">Process</th>
                <th scope="col" className="px-4 py-2.5 text-right font-medium">PID</th>
                <th scope="col" className="px-4 py-2.5 text-right font-medium">CPU</th>
                <th scope="col" className="px-4 py-2.5 text-right font-medium">Memory</th>
                <th scope="col" className="px-4 py-2.5 font-medium">Bind Address</th>
                <th scope="col" className="px-4 py-2.5 font-medium">IP</th>
                <th scope="col" className="px-4 py-2.5 font-medium">State</th>
              </tr>
            </thead>
            <tbody>
              {visible.map((listener) => {
                const process = processByPid.get(listener.pid)
                const identity = serviceByPid.get(listener.pid)
                const project = projectByPid.get(listener.pid)
                const rowKey = `${listener.ipVersion}-${listener.localAddress}-${listener.port}-${listener.pid}`
                return (
                  <tr
                    key={rowKey}
                    className="border-b border-slate-800/60 last:border-0 hover:bg-slate-900/50"
                  >
                    <td className="px-4 py-2.5 font-mono font-medium text-slate-200">
                      {listener.port}
                    </td>
                    <td
                      className="max-w-40 truncate px-4 py-2.5 font-medium text-slate-200"
                      title={
                        identity
                          ? `${identity.displayName} (${identity.confidence}) — ${identity.evidence
                              .map((e) => `${e.source}: ${e.value}`)
                              .join(', ')}`
                          : 'No service identity available'
                      }
                    >
                      {identity?.displayName ?? 'Unknown'}
                    </td>
                    <td
                      className="max-w-32 truncate px-4 py-2.5 text-slate-300"
                      title={
                        project
                          ? `${project.name} — ${project.rootPath}${project.git.branch != null ? ` (⎇ ${project.git.branch})` : ''}`
                          : 'No project association with credible evidence'
                      }
                    >
                      {project?.name ?? '—'}
                    </td>
                    <td
                      className="max-w-36 truncate px-4 py-2.5 font-mono text-slate-300"
                      title={
                        process?.executablePath ??
                        'Process metadata unavailable (access denied or process gone)'
                      }
                    >
                      {process?.name ?? 'Unavailable'}
                    </td>
                    <td className="px-4 py-2.5 text-right font-mono text-slate-400">
                      {listener.pid}
                    </td>
                    <td
                      className="px-4 py-2.5 text-right font-mono text-slate-400"
                      title={
                        process?.cpuPercent === null || process === undefined
                          ? 'First sample — CPU appears on the next refresh'
                          : undefined
                      }
                    >
                      {formatCpuPercent(process?.cpuPercent ?? null)}
                    </td>
                    <td className="px-4 py-2.5 text-right font-mono text-slate-400">
                      {formatBytes(process?.memoryBytes ?? null)}
                    </td>
                    <td className="px-4 py-2.5 font-mono text-slate-400">{listener.localAddress}</td>
                    <td className="px-4 py-2.5 text-slate-400">IPv{listener.ipVersion}</td>
                    <td className="px-4 py-2.5">
                      <span className="rounded border border-emerald-900/60 bg-emerald-950/40 px-1.5 py-0.5 text-xs text-emerald-400">
                        {listener.state}
                      </span>
                    </td>
                  </tr>
                )
              })}
            </tbody>
          </table>
        </div>
      )}

      <p className="mt-3 text-xs text-slate-600">
        One socket per row — the same port on IPv4 and IPv6 (or two bind addresses) is
        two rows, not a duplicate. Service and project names come from evidence-based
        detection (executable, path, command line, confirmed markers); hover for details.
      </p>
    </div>
  )
}
