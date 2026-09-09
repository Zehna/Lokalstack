import { useEffect, useMemo, useState } from 'react'

import { RefreshButton } from '@/app/components/RefreshButton'
import { ControlActions } from '@/app/components/ControlActions'
import { usePortListeners } from '@/hooks'
import { useAiRuntimeStore } from '@/stores/aiRuntimeStore'
import { useDockerStore } from '@/stores/dockerStore'
import { usePortsStore } from '@/stores/portsStore'
import { useWorkspaceStore } from '@/stores/workspaceStore'
import type { AiHealth, ContainerPortOwnership, ServiceCategory } from '@/types/domain'
import { formatBytes, formatCpuPercent, formatTime } from '@/utils/format'
import { groupListenersByProcess } from './groupProcesses'
import {
  categoryBadgeClass,
  confidenceBadgeClass,
  confidenceLabel,
  confidenceTooltip,
  evidenceSummary,
} from './identity'

/** Category filter chips shown above the process list. */
const CATEGORY_FILTERS: ReadonlyArray<{ value: ServiceCategory | 'all'; label: string }> = [
  { value: 'all', label: 'All' },
  { value: 'frontend', label: 'Frontend' },
  { value: 'backend', label: 'Backend' },
  { value: 'database', label: 'Database' },
  { value: 'ai', label: 'AI' },
  { value: 'infrastructure', label: 'Infrastructure' },
  { value: 'unknown', label: 'Unknown' },
]

/** Same health styling as the AI Services page (shared semantic). */
const HEALTH_STYLES: Record<AiHealth, string> = {
  ready: 'border-emerald-900/60 bg-emerald-950/40 text-emerald-400',
  loading: 'border-sky-900/60 bg-sky-950/40 text-sky-400',
  busy: 'border-amber-900/60 bg-amber-950/40 text-amber-400',
  degraded: 'border-amber-900/60 bg-amber-950/40 text-amber-400',
  unavailable: 'border-red-900/60 bg-red-950/40 text-red-400',
  unknown: 'border-slate-700 bg-slate-950 text-slate-500',
}

/**
 * Services view — ACTIVE LOCAL PROCESSES with real service identities.
 *
 * Grouped by PID (pure frontend adapter), classified by the native
 * intelligence layer from evidence (executable, path, command line).
 * Confidence is always shown; generic runtimes stay honestly generic
 * ("Node.js", "Python") when framework evidence is missing.
 */
export function ServicesPage() {
  const workspaces = useWorkspaceStore((state) => state.workspaces)
  const loadWorkspaces = useWorkspaceStore((state) => state.load)
  const aiRuntimes = useAiRuntimeStore((state) => state.runtimes)
  const loadAiRuntimes = useAiRuntimeStore((state) => state.load)
  const dockerSnapshot = useDockerStore((state) => state.snapshot)
  const loadDocker = useDockerStore((state) => state.load)
  useEffect(() => {
    void loadWorkspaces()
    void loadAiRuntimes()
    void loadDocker()
  }, [loadWorkspaces, loadAiRuntimes, loadDocker])
  // Managed root PIDs — any discovered PID in this set is LocalStack-managed.
  const managedPids = new Set(
    workspaces.flatMap((w) =>
      w.services.filter((s) => s.managed !== null).map((s) => s.managed?.rootPid ?? 0),
    ),
  )
  usePortListeners()
  const listeners = usePortsStore((state) => state.listeners)
  const processByPid = usePortsStore((state) => state.processByPid)
  const serviceByPid = usePortsStore((state) => state.serviceByPid)
  const loading = usePortsStore((state) => state.loading)
  const refreshing = usePortsStore((state) => state.refreshing)
  const error = usePortsStore((state) => state.error)
  const lastUpdated = usePortsStore((state) => state.lastUpdated)
  const refreshListeners = usePortsStore((state) => state.refreshListeners)

  const [filter, setFilter] = useState<ServiceCategory | 'all'>('all')
  const [expandedPid, setExpandedPid] = useState<number | null>(null)

  const projectByPid = usePortsStore((state) => state.projectByPid)
  const controlByPid = usePortsStore((state) => state.controlByPid)

  // Phase 9: published host port → container ownership overlay.
  const dockerOwnershipByPort = useMemo(() => {
    const map = new Map<number, ContainerPortOwnership>()
    for (const ownership of dockerSnapshot?.portOwnerships ?? []) {
      if (!map.has(ownership.hostPort)) map.set(ownership.hostPort, ownership)
    }
    return map
  }, [dockerSnapshot])
  const groups = useMemo(
    () =>
      groupListenersByProcess(listeners, processByPid, serviceByPid, projectByPid).filter(
        (group) => {
          if (filter === 'all') return true
          return group.identity?.category === filter
        },
      ),
    [listeners, processByPid, serviceByPid, projectByPid, filter],
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
          Active local processes with evidence-based service identities. Confidence is
          always explicit — generic runtimes are never dressed up as frameworks.
        </p>
      </div>

      {/* Category filter */}
      <div className="mb-4 flex flex-wrap items-center gap-2">
        {CATEGORY_FILTERS.map((entry) => (
          <button
            key={entry.value}
            type="button"
            onClick={() => setFilter(entry.value)}
            className={`rounded-full border px-3 py-1 text-xs font-medium transition-colors ${
              filter === entry.value
                ? 'border-slate-600 bg-slate-800 text-slate-100'
                : 'border-slate-800 bg-slate-900 text-slate-500 hover:border-slate-700 hover:text-slate-300'
            }`}
          >
            {entry.label}
          </button>
        ))}
        <span className="ml-auto text-xs text-slate-500">
          {loading ? '…' : `${groups.length} shown`} · auto-refresh 3s · updated{' '}
          {formatTime(lastUpdated)}
        </span>
      </div>

      {error !== null ? (
        <div className="rounded-lg border border-red-900/60 bg-red-950/30 px-4 py-3 text-sm text-red-300">
          <p className="font-medium">Discovery error</p>
          <p className="mt-1 text-xs text-red-400/80">{error}</p>
        </div>
      ) : null}

      {loading ? (
        <div className="rounded-lg border border-slate-800 bg-slate-900/40 px-4 py-10 text-center text-sm text-slate-500">
          Reading TCP listener tables and process metadata…
        </div>
      ) : groups.length === 0 ? (
        <div className="rounded-lg border border-dashed border-slate-800 p-10 text-center">
          <p className="text-sm text-slate-400">
            {filter === 'all'
              ? 'No active processes with listening ports.'
              : `No ${filter} services right now.`}
          </p>
          <p className="mt-1 text-xs text-slate-600">
            Start a dev server — it will appear here automatically within ~3 seconds.
          </p>
        </div>
      ) : (
        <ul className="overflow-hidden rounded-lg border border-slate-800">
          {groups.map((group, index) => {
            const expanded = expandedPid === group.pid
            return (
              <li
                key={group.pid}
                className={`bg-slate-900/60 ${index > 0 ? 'border-t border-slate-800' : ''}`}
              >
                <div className="flex flex-wrap items-center gap-x-3 gap-y-1 px-4 py-3">
                  <span
                    className={`h-2 w-2 shrink-0 rounded-full ${
                      group.accessible ? 'bg-emerald-400' : 'bg-slate-500'
                    }`}
                    title={
                      group.accessible
                        ? 'Process inspected successfully'
                        : 'Process metadata unavailable (access denied or process gone)'
                    }
                  />

                  <button
                    type="button"
                    onClick={() => setExpandedPid(expanded ? null : group.pid)}
                    className="max-w-56 truncate text-left text-sm font-semibold text-slate-100 hover:text-white"
                    title="Toggle detection details"
                  >
                    {group.displayName}
                  </button>

                  {/* Managed vs external — LocalStack lifecycle knowledge. */}
                  {managedPids.has(group.pid) ? (
                    <span
                      className="rounded border border-sky-900/60 bg-sky-950/40 px-1.5 py-0.5 text-xs text-sky-400"
                      title="This process was launched by LocalStack — it supports graceful stop and restart"
                    >
                      MANAGED
                    </span>
                  ) : (
                    <span
                      className="rounded border border-slate-700 bg-slate-950 px-1.5 py-0.5 text-xs text-slate-500"
                      title="Discovered on the machine — not launched by LocalStack"
                    >
                      EXTERNAL
                    </span>
                  )}

                  {/* Phase 9: containerized service — the owning PID is the
                      Docker proxy; the real workload is a container. */}
                  {group.ports.some((p) => dockerOwnershipByPort.has(p)) && (() => {
                    const ownership = group.ports
                      .map((p) => dockerOwnershipByPort.get(p))
                      .find((o) => o !== undefined)!
                    return (
                      <span
                        className="rounded border border-sky-900/60 bg-sky-950/40 px-1.5 py-0.5 text-xs text-sky-400"
                        title={`Docker container ${ownership.containerName} (${ownership.image ?? 'unknown image'}) publishes host port ${ownership.hostPort} → container port ${ownership.containerPort}${ownership.composeProject ? ` · compose: ${ownership.composeProject}${ownership.composeService ? `/${ownership.composeService}` : ''}` : ''}`}
                      >
                        CONTAINER
                      </span>
                    )
                  })()}

                  {group.identity !== null && (
                    <>
                      <span
                        className={`rounded border px-1.5 py-0.5 text-xs ${confidenceBadgeClass(
                          group.identity.confidence,
                        )}`}
                        title={confidenceTooltip(group.identity.confidence)}
                      >
                        {confidenceLabel(group.identity.confidence)}
                      </span>
                      {group.identity.category !== 'unknown' && (
                        <span
                          className={`rounded border px-1.5 py-0.5 text-xs ${categoryBadgeClass(
                            group.identity.category,
                          )}`}
                        >
                          {group.identity.category}
                        </span>
                      )}
                      {/* Phase 8: runtime health for AI services — distinct
                          from the process lifecycle label shown elsewhere. */}
                      {group.identity.category === 'ai' &&
                        aiRuntimes.find((r) => r.pid === group.pid) !== undefined && (
                          <span
                            className={`rounded border px-1.5 py-0.5 text-xs uppercase ${
                              HEALTH_STYLES[
                                aiRuntimes.find((r) => r.pid === group.pid)!.health
                              ]
                            }`}
                            title="AI runtime health (adapter evidence — not the process lifecycle)"
                          >
                            {aiRuntimes.find((r) => r.pid === group.pid)!.health}
                          </span>
                        )}
                    </>
                  )}

                  {group.project !== null && (
                    <span
                      className="max-w-40 truncate rounded border border-slate-700 bg-slate-950 px-1.5 py-0.5 text-xs text-slate-300"
                      title={`${group.project.name} — ${group.project.rootPath}`}
                    >
                      {group.project.name}
                    </span>
                  )}

                  <span className="font-mono text-xs text-slate-500">
                    {group.process?.name ?? `PID ${group.pid}`}
                  </span>
                  <span className="font-mono text-xs text-slate-500">PID {group.pid}</span>

                  <span className="flex-1 truncate font-mono text-xs text-slate-500">
                    {group.ports.length > 0 ? `Ports ${group.ports.join(', ')}` : 'No ports'}
                  </span>

                  <ControlActions
                    control={
                      controlByPid.get(group.pid) ?? {
                        pid: group.pid,
                        capability: {
                          canOpen: false,
                          canStop: false,
                          gracefulStopSupported: false,
                          gracefulStopReason:
                            'Process was not launched in a LocalStack-managed process group.',
                          canRestart: false,
                          reason: 'Control data not in the current snapshot — refresh.',
                        },
                        urls: [],
                        target: null,
                      }
                    }
                    onStopped={() => void refreshListeners()}
                  />

                  <span
                    className="font-mono text-xs text-slate-400"
                    title={
                      group.cpuPercent === null
                        ? 'First sample — CPU is measured on the next refresh'
                        : undefined
                    }
                  >
                    CPU {formatCpuPercent(group.cpuPercent)}
                  </span>
                  <span className="font-mono text-xs text-slate-400">
                    RAM {formatBytes(group.memoryBytes)}
                  </span>
                </div>

                {expanded && (
                  <div className="border-t border-slate-800/60 bg-slate-950/40 px-4 py-3 text-xs text-slate-400">
                    <dl className="grid grid-cols-[8rem_1fr] gap-x-3 gap-y-1.5">
                      <dt className="text-slate-500">Process</dt>
                      <dd className="font-mono">{group.process?.name ?? '—'}</dd>
                      <dt className="text-slate-500">Executable</dt>
                      <dd className="truncate font-mono" title={group.process?.executablePath ?? undefined}>
                        {group.process?.executablePath ?? '—'}
                      </dd>
                      <dt className="text-slate-500">Command line</dt>
                      <dd className="break-all font-mono" title={group.process?.commandLine ?? undefined}>
                        {group.process?.commandLine ?? 'not readable (access denied)'}
                      </dd>
                      <dt className="text-slate-500">Started</dt>
                      <dd className="font-mono">{formatTime(group.process?.startedAt ?? null)}</dd>
                      <dt className="text-slate-500">Project</dt>
                      <dd className="break-all font-mono">
                        {group.project !== null
                          ? `${group.project.name} (${confidenceLabel(group.project.confidence)} association) · ${group.project.rootPath}`
                          : 'no credible project evidence'}
                      </dd>
                      <dt className="text-slate-500">Confidence</dt>
                      <dd>
                        {group.identity ? (
                          <span title={confidenceTooltip(group.identity.confidence)}>
                            {confidenceLabel(group.identity.confidence)}
                          </span>
                        ) : (
                          '—'
                        )}
                      </dd>
                      <dt className="text-slate-500">Evidence</dt>
                      <dd className="font-mono">
                        {group.identity && group.identity.evidence.length > 0
                          ? evidenceSummary(group.identity)
                          : 'none'}
                      </dd>
                    </dl>
                  </div>
                )}
              </li>
            )
          })}
        </ul>
      )}

      <p className="mt-3 text-xs text-slate-600">
        Identities are classified from evidence (executable name, path, command line) —
        port numbers are never strong evidence, so node.exe on :3000 stays “Node.js”
        unless Next.js evidence exists. Click a name for the detection details.
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
