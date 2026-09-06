import { Activity, Boxes, TriangleAlert } from 'lucide-react'

import { MockDataBadge } from './components/MockDataBadge'
import { SummaryCard } from './components/SummaryCard'
import { usePortListeners } from '@/hooks'
import { RefreshButton } from '@/app/components/RefreshButton'
import { useAppStore } from '@/stores/appStore'
import { usePortsStore } from '@/stores/portsStore'
import type { PortListener } from '@/types/domain'
import { formatPort, formatTime } from '@/utils/format'


/**
 * One real listener row. Deliberately honest: Phase 1 has no process or
 * framework intelligence, so rows show exactly what the OS reports — port,
 * transport, IP version, bind address, PID — and nothing more.
 */
function ListenerRow({ listener }: { listener: PortListener }) {
  const setActiveView = useAppStore((state) => state.setActiveView)
  return (
    <li>
      <button
        type="button"
        onClick={() => setActiveView('ports')}
        className="flex w-full items-center gap-3 rounded-lg border border-slate-800 bg-slate-900/60 px-4 py-3 text-left transition-colors hover:border-slate-700 hover:bg-slate-900"
      >
        <span className="relative flex h-2 w-2 shrink-0">
          <span className="absolute inline-flex h-full w-full animate-ping rounded-full bg-emerald-400 opacity-60" />
          <span className="relative inline-flex h-2 w-2 rounded-full bg-emerald-400" />
        </span>

        <span className="font-mono text-sm font-medium text-slate-200">{listener.port}</span>

        <span className="rounded border border-slate-700 bg-slate-950 px-1.5 py-0.5 text-xs text-slate-400">
          TCP · IPv{listener.ipVersion}
        </span>

        <span className="flex-1 truncate font-mono text-xs text-slate-500">
          {listener.localAddress}
        </span>

        <span className="font-mono text-xs text-slate-500">PID {listener.pid}</span>
      </button>
    </li>
  )
}

/**
 * Dashboard — the summary cards plus a REAL listener section fed by the
 * Windows discovery engine every ~3 seconds. The mock "example services"
 * list from Phase 0 remains, clearly labeled, as a preview of the richer
 * Phase 3 identity view; real and mock surfaces never mix.
 */
export function DashboardPage() {
  usePortListeners()
  const listeners = usePortsStore((state) => state.listeners)
  const loading = usePortsStore((state) => state.loading)
  const refreshing = usePortsStore((state) => state.refreshing)
  const error = usePortsStore((state) => state.error)
  const lastUpdated = usePortsStore((state) => state.lastUpdated)
  const refreshListeners = usePortsStore((state) => state.refreshListeners)

  const listenerCount = listeners.length

  return (
    <div className="mx-auto w-full max-w-5xl px-6 py-8">
      {/* Header */}
      <div className="mb-6">
        <div className="flex items-center justify-between">
          <h1 className="text-xl font-semibold text-slate-100">Dashboard</h1>
          <RefreshButton onClick={() => void refreshListeners()} refreshing={refreshing} />
        </div>
        <p className="mt-1 text-sm text-slate-500">
          Everything currently listening on localhost, discovered live from Windows.
        </p>
      </div>

      {/* Summary cards */}
      <div className="grid grid-cols-2 gap-4 xl:grid-cols-4">
        <SummaryCard
          title="Services"
          icon={<Activity className="h-4 w-4 text-sky-400" strokeWidth={1.8} />}
          value={loading ? '…' : String(listenerCount)}
          detail="live"
          footer="TCP listeners on this machine"
        />
        <SummaryCard
          title="Projects"
          icon={<Boxes className="h-4 w-4 text-violet-400" strokeWidth={1.8} />}
          value="—"
          detail="Phase 4"
          footer="Linked to running services"
        />
        <SummaryCard
          title="Port Conflicts"
          icon={<TriangleAlert className="h-4 w-4 text-amber-400" strokeWidth={1.8} />}
          value={loading ? '…' : '0'}
          detail="Phase 7 engine"
          footer="Ports claimed by multiple listeners"
        />
        <SummaryCard
          title="System Usage"
          icon={<Activity className="h-4 w-4 text-emerald-400" strokeWidth={1.8} />}
          value="—"
          detail="Phase 2"
          footer={<span className="text-xs text-slate-500">CPU / memory arrives in Phase 2.</span>}
        />
      </div>

      {/* Real listeners */}
      <section className="mt-8" aria-labelledby="active-listeners-heading">
        <div className="mb-3 flex items-center justify-between">
          <h2
            id="active-listeners-heading"
            className="text-sm font-semibold uppercase tracking-wider text-slate-400"
          >
            Active Listeners
          </h2>
          <span className="text-xs text-slate-500">
            auto-refresh 3s · updated {formatTime(lastUpdated)}
          </span>
        </div>

        {error !== null ? (
          <div className="rounded-lg border border-red-900/60 bg-red-950/30 px-4 py-3 text-sm text-red-300">
            <p className="font-medium">Discovery error</p>
            <p className="mt-1 text-xs text-red-400/80">{error}</p>
          </div>
        ) : loading ? (
          <div className="rounded-lg border border-slate-800 bg-slate-900/40 px-4 py-6 text-center text-sm text-slate-500">
            Reading TCP listener tables…
          </div>
        ) : listeners.length === 0 ? (
          <p className="rounded-lg border border-dashed border-slate-800 p-6 text-center text-sm text-slate-500">
            No TCP listeners found. Start a dev server and it will appear automatically.
          </p>
        ) : (
          <ul className="space-y-2">
            {listeners.map((listener) => (
              <ListenerRow
                key={`${listener.ipVersion}-${listener.localAddress}-${listener.port}-${listener.pid}`}
                listener={listener}
              />
            ))}
          </ul>
        )}

        <p className="mt-3 text-xs text-slate-600">
          Rows show only what Windows reports (port, transport, IP version, bind address, PID).
          Process names and service identities arrive in Phase 2–3 — LocalStack does not guess.
        </p>
      </section>

      {/* Mock preview of the Phase 3 identity view */}
      <section className="mt-10" aria-labelledby="mock-services-heading">
        <div className="mb-3 flex items-center gap-3">
          <h2
            id="mock-services-heading"
            className="text-sm font-semibold uppercase tracking-wider text-slate-400"
          >
            Services Preview
          </h2>
          <MockDataBadge />
        </div>
        <div className="grid grid-cols-2 gap-2 sm:grid-cols-4">
          {[
            { name: 'Next.js', port: 3000 },
            { name: 'Flask', port: 5000 },
            { name: 'PostgreSQL', port: 5432 },
            { name: 'llama.cpp', port: 8080 },
          ].map((service) => (
            <div
              key={service.name}
              className="rounded-lg border border-dashed border-slate-800 bg-slate-900/40 px-3 py-2.5"
            >
              <p className="truncate text-sm font-medium text-slate-300">{service.name}</p>
              <p className="font-mono text-xs text-slate-500">{formatPort(service.port)}</p>
            </div>
          ))}
        </div>
        <p className="mt-2 text-xs text-slate-600">
          Static example of the named-service view planned for Phase 3 (framework detection) —
          these are NOT detected services.
        </p>
      </section>
    </div>
  )
}
