import { useMemo, useState } from 'react'
import { ArrowUpDown, Search } from 'lucide-react'

import { RefreshButton } from '@/app/components/RefreshButton'
import { usePortListeners } from '@/hooks'
import { usePortsStore } from '@/stores/portsStore'
import type { PortListener } from '@/types/domain'
import { formatTime } from '@/utils/format'

/** Column keys the table can sort by. Only port sorting is required in Phase 1. */
type SortDirection = 'asc' | 'desc'

function listenerMatches(listener: PortListener, query: string): boolean {
  const q = query.trim().toLowerCase()
  if (q === '') return true
  return (
    String(listener.port).includes(q) ||
    String(listener.pid).includes(q) ||
    listener.localAddress.toLowerCase().includes(q)
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
 * Ports view — the real, live table of every TCP listener Windows reports.
 * Data comes from the ports store (native discovery, auto-refresh ~3 s);
 * this component only presents, filters and sorts it.
 */
export function PortsPage() {
  usePortListeners()
  const listeners = usePortsStore((state) => state.listeners)
  const loading = usePortsStore((state) => state.loading)
  const refreshing = usePortsStore((state) => state.refreshing)
  const error = usePortsStore((state) => state.error)
  const lastUpdated = usePortsStore((state) => state.lastUpdated)
  const refreshListeners = usePortsStore((state) => state.refreshListeners)

  const [query, setQuery] = useState('')
  const [direction, setDirection] = useState<SortDirection>('asc')

  const visible = useMemo(
    () =>
      listeners
        .filter((listener) => listenerMatches(listener, query))
        .sort((a, b) => compareListeners(a, b, direction)),
    [listeners, query, direction],
  )

  return (
    <div className="mx-auto w-full max-w-5xl px-6 py-8">
      {/* Header */}
      <div className="mb-6">
        <div className="flex items-center justify-between">
          <h1 className="text-xl font-semibold text-slate-100">Ports</h1>
          <RefreshButton onClick={() => void refreshListeners()} refreshing={refreshing} />
        </div>
        <p className="mt-1 text-sm text-slate-500">
          Every TCP listener on this machine, discovered live from Windows via
          GetExtendedTcpTable. Refreshes automatically every ~3 seconds.
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
            placeholder="Search port, PID, or address…"
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
            Search matches port numbers, PIDs and bind addresses.
          </p>
        </div>
      ) : (
        <div className="overflow-hidden rounded-lg border border-slate-800">
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
                <th scope="col" className="px-4 py-2.5 font-medium">Protocol</th>
                <th scope="col" className="px-4 py-2.5 font-medium">Bind Address</th>
                <th scope="col" className="px-4 py-2.5 font-medium">IP Version</th>
                <th scope="col" className="px-4 py-2.5 text-right font-medium">PID</th>
                <th scope="col" className="px-4 py-2.5 font-medium">State</th>
              </tr>
            </thead>
            <tbody>
              {visible.map((listener) => (
                <tr
                  key={`${listener.ipVersion}-${listener.localAddress}-${listener.port}-${listener.pid}`}
                  className="border-b border-slate-800/60 last:border-0 hover:bg-slate-900/50"
                >
                  <td className="px-4 py-2.5 font-mono font-medium text-slate-200">
                    {listener.port}
                  </td>
                  <td className="px-4 py-2.5 uppercase text-slate-400">{listener.protocol}</td>
                  <td className="px-4 py-2.5 font-mono text-slate-400">{listener.localAddress}</td>
                  <td className="px-4 py-2.5 text-slate-400">IPv{listener.ipVersion}</td>
                  <td className="px-4 py-2.5 text-right font-mono text-slate-400">
                    {listener.pid}
                  </td>
                  <td className="px-4 py-2.5">
                    <span className="rounded border border-emerald-900/60 bg-emerald-950/40 px-1.5 py-0.5 text-xs text-emerald-400">
                      {listener.state}
                    </span>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      <p className="mt-3 text-xs text-slate-600">
        One socket per row — the same port on IPv4 and IPv6 (or two bind addresses) is two rows,
        not a duplicate. Process names arrive in Phase 2; LocalStack does not guess identities.
      </p>
    </div>
  )
}
