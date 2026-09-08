import { useEffect } from 'react'
import { Activity, Boxes, TriangleAlert } from 'lucide-react'

import { RefreshButton } from '@/app/components/RefreshButton'
import { SummaryCard } from './components/SummaryCard'
import { usePortListeners } from '@/hooks'
import { useAppStore } from '@/stores/appStore'
import { useConflictsStore } from '@/stores/conflictsStore'
import { usePortsStore } from '@/stores/portsStore'
import { useWorkspaceStore } from '@/stores/workspaceStore'
import type { PortListener, ProcessInfo, ProjectIdentity, ServiceIdentity } from '@/types/domain'
import { formatBytes, formatCpuPercent, formatTime } from '@/utils/format'

/**
 * One real listener row. Shows the evidence-based service identity (or the
 * honest runtime/generic name), port, address, and live CPU/RAM. Identities
 * carry confidence from the native detector; nothing is guessed from ports.
 */
function ListenerRow({
  listener,
  process,
  identity,
  project,
}: {
  listener: PortListener
  process: ProcessInfo | undefined
  identity: ServiceIdentity | undefined
  project: ProjectIdentity | undefined
}) {
  const setActiveView = useAppStore((state) => state.setActiveView)
  const displayName = identity?.displayName ?? process?.name ?? `PID ${listener.pid}`
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

        <span
          className="max-w-40 truncate text-sm font-semibold text-slate-100"
          title={
            identity
              ? `${displayName} (${identity.confidence}) — ${identity.evidence
                  .map((e) => `${e.source}: ${e.value}`)
                  .join(', ')}`
              : (process?.executablePath ?? 'Process metadata unavailable')
          }
        >
          {displayName}
        </span>

        <span className="font-mono text-xs text-slate-500">{process?.name ?? ''}</span>

        {project !== undefined && (
          <span
            className="max-w-32 truncate rounded border border-slate-700 bg-slate-950 px-1.5 py-0.5 text-xs text-slate-300"
            title={`${project.name} — ${project.rootPath}`}
          >
            {project.name}
          </span>
        )}

        <span className="rounded border border-slate-700 bg-slate-950 px-1.5 py-0.5 font-mono text-xs text-slate-400">
          :{listener.port}
        </span>

        <span className="rounded border border-slate-700 bg-slate-950 px-1.5 py-0.5 text-xs text-slate-400">
          TCP · IPv{listener.ipVersion}
        </span>

        <span className="flex-1 truncate font-mono text-xs text-slate-500">
          {listener.localAddress}
        </span>

        <span
          className="font-mono text-xs text-slate-400"
          title={
            process?.cpuPercent == null
              ? 'First sample — CPU appears on the next refresh'
              : 'CPU over the last refresh window, normalized per logical core'
          }
        >
          CPU {formatCpuPercent(process?.cpuPercent ?? null)}
        </span>

        <span className="font-mono text-xs text-slate-400">
          RAM {formatBytes(process?.memoryBytes ?? null)}
        </span>

        <span className="font-mono text-xs text-slate-500">PID {listener.pid}</span>
      </button>
    </li>
  )
}

/**
 * Dashboard — summary cards plus the REAL listener section, fed every ~3 s
 * by the Windows discovery + process-intelligence pipeline. No mock service
 * identities anywhere: executable names come from Windows, and nothing more
 * is claimed until Phase 3 detection exists.
 */
export function DashboardPage() {
  usePortListeners()
  const workspaces = useWorkspaceStore((state) => state.workspaces)
  const loadWorkspaces = useWorkspaceStore((state) => state.load)
  useEffect(() => {
    void loadWorkspaces()
  }, [loadWorkspaces])
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
  const loadConflicts = useConflictsStore((state) => state.load)
  const readiness = useConflictsStore((state) => state.readiness)
  useEffect(() => {
    void loadConflicts()
  }, [loadConflicts])

  const listenerCount = listeners.length
  const projects = usePortsStore((state) => state.projects)

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
      <div className="grid grid-cols-2 gap-4 xl:grid-cols-5">
        <SummaryCard
          title="Workspaces"
          icon={<Boxes className="h-4 w-4 text-teal-400" strokeWidth={1.8} />}
          value={loading ? '…' : String(workspaces.length)}
          detail={`${workspaces.filter((w) => w.status === 'running').length} running · ${workspaces.filter((w) => w.status === 'partial').length} partial`}
          footer="Managed service groups"
        />
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
          value={loading ? '…' : String(projects.length)}
          detail="live"
          footer="Linked to running services"
        />
        <SummaryCard
          title="Port Conflicts"
          icon={<TriangleAlert className="h-4 w-4 text-amber-400" strokeWidth={1.8} />}
          value={loading ? '…' : String(
            readiness.reduce((count, view) => count + view.conflicts.length, 0),
          )}
          detail="blocking workspace launches"
          footer="Ports claimed by an owner that is not the requester"
        />
        <SummaryCard
          title="Blocked / Deps Down"
          icon={<TriangleAlert className="h-4 w-4 text-red-400" strokeWidth={1.8} />}
          value={loading ? '…' : String(
            readiness.filter((view) => view.status === 'blocked').length,
          )}
          detail={`${readiness.reduce(
            (count, view) =>
              count + view.dependencies.filter((d) => d.state === 'unavailable').length,
            0,
          )} dependencies unavailable`}
          footer="Required dependencies or ports blocking readiness"
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
            {durationMs !== null ? ` · cycle ${durationMs} ms` : ''}
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
                process={processByPid.get(listener.pid)}
                identity={serviceByPid.get(listener.pid)}
                project={projectByPid.get(listener.pid)}
              />
            ))}
          </ul>
        )}

        <p className="mt-3 text-xs text-slate-600">
          Rows show the evidence-based service identity (or the honest runtime name —
          node.exe stays “Node.js” without framework evidence), port, bind address,
          CPU and working-set memory. “Unavailable” means Windows denied inspection.
        </p>
      </section>
    </div>
  )
}
