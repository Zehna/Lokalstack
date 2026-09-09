import { useEffect, useState } from 'react'
import {
  Archive,
  ChevronDown,
  ChevronRight,
  CircleSlash,
  Container,
  HardDrive,
  HeartPulse,
  RefreshCw,
  Server,
} from 'lucide-react'

import { RefreshButton } from '@/app/components/RefreshButton'
import {
  linksByContainer,
  startDockerPolling,
  stopDockerPolling,
  useDockerStore,
} from '@/stores/dockerStore'
import type { ContainerHealth, ContainerState, DockerContainer } from '@/types/domain'
import { formatBytes, formatTime } from '@/utils/format'

/** Tailwind classes per container state (spec §6). */
const STATE_STYLES: Record<ContainerState, string> = {
  created: 'border-slate-700 bg-slate-950 text-slate-400',
  running: 'border-emerald-900/60 bg-emerald-950/40 text-emerald-400',
  paused: 'border-amber-900/60 bg-amber-950/40 text-amber-400',
  restarting: 'border-sky-900/60 bg-sky-950/40 text-sky-400',
  removing: 'border-amber-900/60 bg-amber-950/40 text-amber-400',
  exited: 'border-slate-700 bg-slate-950 text-slate-500',
  dead: 'border-red-900/60 bg-red-950/40 text-red-400',
  unknown: 'border-slate-700 bg-slate-950 text-slate-500',
}

/** Tailwind classes per health state (spec §7) — `none` is quiet, not red. */
const HEALTH_STYLES: Record<ContainerHealth, string> = {
  healthy: 'border-emerald-900/60 bg-emerald-950/40 text-emerald-400',
  unhealthy: 'border-red-900/60 bg-red-950/40 text-red-400',
  starting: 'border-sky-900/60 bg-sky-950/40 text-sky-400',
  none: 'border-slate-700 bg-slate-950 text-slate-500',
  unknown: 'border-slate-700 bg-slate-950 text-slate-500',
}

/** Badge renderer shared by state + health. */
function Badge({ label, styles }: { label: string; styles: string }) {
  return (
    <span className={`rounded border px-1.5 py-0.5 text-xs font-medium uppercase ${styles}`}>
      {label}
    </span>
  )
}

/** One expandable container card. */
function ContainerCard({
  container,
  projectName,
  projectConfidence,
}: {
  container: DockerContainer
  projectName?: string
  projectConfidence?: string
}) {
  const [expanded, setExpanded] = useState(false)

  const published = container.ports.filter((p) => p.published && p.protocol === 'tcp')

  return (
    <section className="rounded-lg border border-slate-800">
      <div className="flex items-center gap-3 border-b border-slate-800 bg-slate-900/60 px-4 py-3">
        <button
          type="button"
          onClick={() => setExpanded((value) => !value)}
          className="text-slate-500 hover:text-slate-300"
          aria-label={expanded ? 'Collapse details' : 'Expand details'}
        >
          {expanded ? <ChevronDown className="h-4 w-4" /> : <ChevronRight className="h-4 w-4" />}
        </button>
        <Container className="h-4 w-4 text-sky-400" strokeWidth={1.8} />
        <h3 className="text-sm font-semibold text-slate-100">{container.name}</h3>
        <Badge label={container.state} styles={STATE_STYLES[container.state]} />
        {container.health !== 'none' && (
          <Badge label={container.health} styles={HEALTH_STYLES[container.health]} />
        )}
        <span className="font-mono text-xs text-slate-500">{container.image}</span>
        <span className="flex-1" />
        {container.compose?.projectName && (
          <span className="rounded border border-slate-700 bg-slate-950 px-1.5 py-0.5 text-xs text-slate-400">
            compose: {container.compose.projectName}
            {container.compose.serviceName ? ` / ${container.compose.serviceName}` : ''}
          </span>
        )}
        {projectName && (
          <span
            className="rounded border border-violet-900/60 bg-violet-950/40 px-1.5 py-0.5 text-xs text-violet-300"
            title={projectConfidence ? `Association confidence: ${projectConfidence}` : undefined}
          >
            {projectName}
          </span>
        )}
      </div>

      <div className="grid grid-cols-2 gap-3 px-4 py-3 text-xs text-slate-400 sm:grid-cols-4">
        <div>
          <div className="text-slate-500">Ports (host → container)</div>
          <div className="font-mono text-slate-300">
            {published.length === 0
              ? '—'
              : published
                  .map((p) => `${p.hostPort} → ${p.containerPort}/${p.protocol}`)
                  .join(', ')}
          </div>
        </div>
        <div>
          <div className="text-slate-500">CPU</div>
          <div className="font-mono text-slate-300">
            {container.stats?.cpuPercent !== undefined
              ? `${container.stats.cpuPercent.toFixed(1)}%`
              : '—'}
          </div>
        </div>
        <div>
          <div className="text-slate-500">Memory</div>
          <div className="font-mono text-slate-300">
            {container.stats?.memoryUsedBytes !== undefined
              ? formatBytes(container.stats.memoryUsedBytes)
              : '—'}
            {container.stats?.memoryLimitBytes !== undefined
              ? ` / ${formatBytes(container.stats.memoryLimitBytes)}`
              : ''}
          </div>
        </div>
        <div>
          <div className="text-slate-500">Status</div>
          <div className="text-slate-300">{container.status || '—'}</div>
        </div>
      </div>

      {expanded && (
        <div className="space-y-2 border-t border-slate-800 px-4 py-3 text-xs text-slate-400">
          <div className="flex items-center gap-2">
            <HardDrive className="h-3.5 w-3.5 text-slate-500" />
            <span className="font-mono text-slate-400">{container.shortId}</span>
            {container.imageId && (
              <span className="font-mono text-slate-600">{container.imageId}</span>
            )}
          </div>
          {container.networks.length > 0 && (
            <div>
              <span className="text-slate-500">Networks: </span>
              {container.networks.map((n) => (
                <span key={n.name} className="mr-2 font-mono text-slate-300">
                  {n.name}
                  {n.ipAddress ? ` (${n.ipAddress})` : ''}
                </span>
              ))}
            </div>
          )}
          {container.mounts.length > 0 && (
            <div>
              <span className="text-slate-500">Bind mounts: </span>
              {container.mounts.map(([source, dest]) => (
                <span key={`${source}${dest}`} className="mr-2 font-mono text-slate-300">
                  {source} → {dest}
                </span>
              ))}
            </div>
          )}
          {container.compose?.workingDir && (
            <div>
              <span className="text-slate-500">Compose working dir: </span>
              <span className="font-mono text-slate-300">{container.compose.workingDir}</span>
            </div>
          )}
          {container.createdAt !== undefined && (
            <div>
              <span className="text-slate-500">Created: </span>
              <span className="text-slate-300">{formatTime(container.createdAt)}</span>
            </div>
          )}
          {container.stats?.networkRxBytes !== undefined && (
            <div>
              <span className="text-slate-500">Network: </span>
              <span className="font-mono text-slate-300">
                ↓ {formatBytes(container.stats.networkRxBytes)} · ↑{' '}
                {formatBytes(container.stats.networkTxBytes ?? 0)}
              </span>
            </div>
          )}
          <p className="pt-1 text-slate-600">
            Observability only — LocalStack does not start, stop, or modify containers.
          </p>
        </div>
      )}
    </section>
  )
}

/**
 * Docker page (Phase 9) — read-only engine + container observability.
 * Docker being absent is an honest state, not an error banner (spec §29).
 */
export function DockerPage() {
  const snapshot = useDockerStore((state) => state.snapshot)
  const loading = useDockerStore((state) => state.loading)
  const error = useDockerStore((state) => state.error)
  const lastUpdated = useDockerStore((state) => state.lastUpdated)
  const refresh = useDockerStore((state) => state.refresh)
  const refreshing = useDockerStore((state) => state.refreshing)

  useEffect(() => {
    startDockerPolling()
    return () => stopDockerPolling()
  }, [])

  const [engineExpanded, setEngineExpanded] = useState(false)
  const links = linksByContainer(snapshot?.projectLinks ?? [])
  const running = snapshot?.containers.filter((c) => c.state === 'running').length ?? 0
  const unhealthy = snapshot?.containers.filter((c) => c.health === 'unhealthy').length ?? 0
  const composeProjects = new Set(
    snapshot?.containers
      .map((c) => c.compose?.projectName)
      .filter((name): name is string => Boolean(name)),
  ).size

  return (
    <div className="mx-auto max-w-5xl space-y-6 p-6">
      <header className="flex items-center gap-3">
        <Server className="h-5 w-5 text-sky-400" strokeWidth={1.8} />
        <h1 className="text-lg font-semibold text-slate-100">Docker Engine</h1>
        <RefreshButton onClick={() => void refresh()} refreshing={refreshing} />
      </header>

      {!snapshot && loading && (
        <p className="text-sm text-slate-500">Checking Docker Engine…</p>
      )}

      {snapshot && !snapshot.available && (
        <section className="rounded-lg border border-slate-800 bg-slate-900/40 p-6 text-center">
          <CircleSlash className="mx-auto h-8 w-8 text-slate-600" strokeWidth={1.5} />
          <h2 className="mt-2 text-sm font-semibold text-slate-300">Docker Unavailable</h2>
          <p className="mt-1 text-xs text-slate-500">
            {error ?? 'Docker Engine is not currently available.'}
          </p>
          <p className="mt-3 text-xs text-slate-600">
            This is not a LocalStack error. Docker Desktop may not be installed or running.
          </p>
        </section>
      )}

      {snapshot?.available && (
        <>
          {/* Engine header (spec §31, §49) */}
          <section className="rounded-lg border border-slate-800 bg-slate-900/40">
            <div className="flex items-center gap-3 px-4 py-3">
              <Archive className="h-4 w-4 text-sky-400" />
              <div className="min-w-0 flex-1">
                <div className="text-sm text-slate-200">
                  Docker {snapshot.engine?.version ?? 'unknown'}
                </div>
                <div className="text-xs text-slate-500">
                  API {snapshot.engine?.apiVersion ?? '?'} · {snapshot.engine?.os ?? '?'} /{' '}
                  {snapshot.engine?.arch ?? '?'} · {running}/{snapshot.containers.length} running
                  {unhealthy > 0 ? ` · ${unhealthy} unhealthy` : ''} · {composeProjects} compose
                  project{composeProjects === 1 ? '' : 's'}
                </div>
              </div>
              <Badge label="ready" styles={HEALTH_STYLES.healthy} />
              <span className="text-xs text-slate-600">
                {lastUpdated ? formatTime(lastUpdated) : ''} · {snapshot.latencyMs} ms
              </span>
            </div>
            {engineExpanded && (
              <div className="border-t border-slate-800 px-4 py-2 text-xs text-slate-500">
                <button
                  type="button"
                  className="text-slate-500 hover:text-slate-300"
                  onClick={() => setEngineExpanded(false)}
                >
                  Hide engine details
                </button>
                <div className="mt-1">
                  Snapshot captured at {formatTime(snapshot.capturedAt)}
                  {snapshot.truncated
                    ? ` · truncated at ${snapshot.containers.length} containers`
                    : ''}
                </div>
              </div>
            )}
            {!engineExpanded && (
              <div className="px-4 pb-2">
                <button
                  type="button"
                  className="text-xs text-slate-600 hover:text-slate-400"
                  onClick={() => setEngineExpanded(true)}
                >
                  Engine details…
                </button>
              </div>
            )}
          </section>

          {/* Containers (spec §31) */}
          <div className="space-y-3">
            <h2 className="text-xs font-semibold uppercase tracking-wide text-slate-500">
              Containers
            </h2>
            {snapshot.containers.length === 0 ? (
              <p className="rounded-lg border border-slate-800 bg-slate-900/40 p-6 text-center text-sm text-slate-500">
                No containers found on this engine.
              </p>
            ) : (
              snapshot.containers.map((container) => {
                const link = links.get(container.id)
                const projectName = link?.projectId
                  ? link.projectId.split('\\').pop()
                  : undefined
                return (
                  <ContainerCard
                    key={container.id}
                    container={container}
                    projectName={projectName}
                    projectConfidence={link?.confidence}
                  />
                )
              })
            )}
          </div>

          <div className="flex items-center gap-2 text-xs text-slate-600">
            <HeartPulse className="h-3.5 w-3.5" />
            Health comes from Docker Healthcheck evidence only — running state alone never
            implies healthy.
            {refreshing && <RefreshCw className="ml-2 h-3 w-3 animate-spin" />}
          </div>
        </>
      )}
    </div>
  )
}
