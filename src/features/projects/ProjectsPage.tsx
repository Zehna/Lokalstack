import { useMemo, useState } from 'react'
import { ChevronDown, ChevronRight, FolderGit2, FolderOpen } from 'lucide-react'

import { RefreshButton } from '@/app/components/RefreshButton'
import { usePortListeners } from '@/hooks'
import { usePortsStore } from '@/stores/portsStore'
import type { GroupedProcess } from '@/features/services/groupProcesses'
import { groupListenersByProcess } from '@/features/services/groupProcesses'
import { confidenceBadgeClass, confidenceLabel, confidenceTooltip } from '@/features/services/identity'
import type { ProjectIdentity } from '@/types/domain'
import { formatBytes, formatCpuPercent, formatTime } from '@/utils/format'

/** How a project card groups its running processes. */
interface ProjectGroup {
  /** `null` for processes with no credible project association. */
  project: ProjectIdentity | null
  /** All processes belonging to this project (or unlinked, when null). */
  processes: GroupedProcess[]
  /** Distinct ports across the group's processes, ascending. */
  ports: number[]
  /** Summed working-set memory (bytes) across inspectable processes. */
  totalMemoryBytes: number | null
  /** Max of the processes' latest CPU percentages (per-window samples do not sum). */
  peakCpuPercent: number | null
}

/**
 * Group running processes by their resolved project.
 *
 * Pure derived frontend logic over the native snapshot: processes whose
 * `projectByPid` link points at the same `ProjectIdentity.id` share one
 * card. Processes with no credible evidence form the honest "unknown"
 * group — they are never forced into a project.
 */
export function groupByProject(
  groups: GroupedProcess[],
  projectByPid: ReadonlyMap<number, ProjectIdentity>,
): ProjectGroup[] {
  const byId = new Map<string, ProjectGroup>()
  const unknown: ProjectGroup = {
    project: null,
    processes: [],
    ports: [],
    totalMemoryBytes: null,
    peakCpuPercent: null,
  }

  for (const group of groups) {
    const project = projectByPid.get(group.pid) ?? null
    const target =
      project !== null
        ? byId.get(project.id) ?? {
            project,
            processes: [],
            ports: [],
            totalMemoryBytes: null,
            peakCpuPercent: null,
          }
        : unknown

    target.processes.push(group)
    for (const port of group.ports) {
      if (!target.ports.includes(port)) target.ports.push(port)
    }
    if (group.memoryBytes !== null) {
      target.totalMemoryBytes = (target.totalMemoryBytes ?? 0) + group.memoryBytes
    }
    if (group.cpuPercent !== null) {
      target.peakCpuPercent = Math.max(target.peakCpuPercent ?? 0, group.cpuPercent)
    }

    if (project !== null && !byId.has(project.id)) {
      byId.set(project.id, target)
    }
  }

  {
    for (const entry of byId.values()) {
      entry.ports.sort((a, b) => a - b)
    }
    unknown.ports.sort((a, b) => a - b)
  }

  const known = [...byId.values()].sort((a, b) =>
    (a.project?.name ?? '').localeCompare(b.project?.name ?? ''),
  )
  // Unknown processes last — they are the honest leftovers, not a project.
  return unknown.processes.length > 0 ? [...known, unknown] : known
}

/** One project card (or the unknown-processes card). */
function ProjectCard({ group }: { group: ProjectGroup }) {
  const [expanded, setExpanded] = useState(false)
  const project = group.project
  const title = project?.name ?? 'Unknown Project'
  const count = group.processes.length

  return (
    <li className="overflow-hidden rounded-lg border border-slate-800 bg-slate-900/60">
      <div className="px-4 py-3">
        <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
          {project !== null ? (
            project.git.isRepository ? (
              <FolderGit2 className="h-4 w-4 shrink-0 text-violet-400" strokeWidth={1.8} />
            ) : (
              <FolderOpen className="h-4 w-4 shrink-0 text-slate-400" strokeWidth={1.8} />
            )
          ) : (
            <FolderOpen className="h-4 w-4 shrink-0 text-slate-600" strokeWidth={1.8} />
          )}

          <span className="text-sm font-semibold text-slate-100">{title}</span>

          {project !== null && (
            <span
              className={`rounded border px-1.5 py-0.5 text-xs ${confidenceBadgeClass(project.confidence)}`}
              title={confidenceTooltip(project.confidence)}
            >
              {confidenceLabel(project.confidence)}
            </span>
          )}

          {project?.git.branch != null && (
            <span
              className="rounded border border-slate-700 bg-slate-950 px-1.5 py-0.5 font-mono text-xs text-slate-300"
              title={`Git repository: ${project.git.rootPath ?? ''}`}
            >
              ⎇ {project.git.branch}
            </span>
          )}

          {project?.packageManager != null && (
            <span className="rounded border border-slate-700 bg-slate-950 px-1.5 py-0.5 text-xs text-slate-400">
              {project.packageManager}
            </span>
          )}

          <span className="ml-auto text-xs text-slate-500">
            {count} {count === 1 ? 'process' : 'processes'}
            {group.ports.length > 0 ? ` · ports ${group.ports.join(', ')}` : ''}
          </span>
        </div>

        {project !== null && (
          <p className="mt-1 truncate font-mono text-xs text-slate-500" title={project.rootPath}>
            {project.rootPath}
          </p>
        )}

        <div className="mt-2 flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-slate-400">
          {group.processes.map((process) => (
            <span key={process.pid} className="font-mono">
              {process.displayName}
              {process.ports.length > 0 ? ` :${process.ports.join(' :')}` : ''}
              <span className="text-slate-600"> · PID {process.pid}</span>
            </span>
          ))}
          <span className="ml-auto font-mono">
            CPU {formatCpuPercent(group.peakCpuPercent)} · RAM {formatBytes(group.totalMemoryBytes)}
          </span>
        </div>

        {project?.startCommand != null && (
          <p className="mt-1 text-xs text-slate-500">
            Start command:{' '}
            <span className="font-mono text-slate-400">{project.startCommand.command}</span>
            <span className="ml-1 text-slate-600">
              ({project.startCommand.confidence} confidence)
            </span>
          </p>
        )}
      </div>

      {project !== null && (
        <button
          type="button"
          onClick={() => setExpanded((value) => !value)}
          className="flex w-full items-center gap-1.5 border-t border-slate-800/60 bg-slate-950/40 px-4 py-2 text-xs text-slate-500 hover:text-slate-300"
        >
          {expanded ? (
            <ChevronDown className="h-3.5 w-3.5" strokeWidth={1.8} />
          ) : (
            <ChevronRight className="h-3.5 w-3.5" strokeWidth={1.8} />
          )}
          Detection details
        </button>
      )}

      {expanded && project !== null && (
        <div className="border-t border-slate-800/60 bg-slate-950/40 px-4 py-3 text-xs text-slate-400">
          <dl className="grid grid-cols-[8rem_1fr] gap-x-3 gap-y-1.5">
            <dt className="text-slate-500">Root path</dt>
            <dd className="break-all font-mono">{project.rootPath}</dd>
            <dt className="text-slate-500">Kind</dt>
            <dd className="font-mono">{project.kind}</dd>
            <dt className="text-slate-500">Git</dt>
            <dd className="font-mono">
              {project.git.isRepository
                ? `repository · branch ${project.git.branch ?? '(detached)'} · ${project.git.rootPath ?? ''}`
                : 'not a repository'}
            </dd>
            <dt className="text-slate-500">Package manager</dt>
            <dd className="font-mono">{project.packageManager ?? '—'}</dd>
            <dt className="text-slate-500">Confidence</dt>
            <dd>{confidenceLabel(project.confidence)}</dd>
            <dt className="text-slate-500">Evidence</dt>
            <dd className="break-all font-mono">
              {project.evidence.map((e) => `${e.source}: ${e.value}`).join(' · ')}
            </dd>
            <dt className="text-slate-500">Last seen</dt>
            <dd className="font-mono">
              {formatTime(
                group.processes.reduce<number | null>(
                  (latest, process) =>
                    Math.max(latest ?? 0, process.process?.startedAt ?? 0) || null,
                  null,
                ),
              )}
            </dd>
          </dl>
        </div>
      )}
    </li>
  )
}

/**
 * Projects view — resolved projects with their running services, fed by the
 * same ~3 s native cycle. Associations are evidence-based (command-line
 * paths confirmed by project markers); unknown stays unknown.
 */
export function ProjectsPage() {
  usePortListeners()
  const listeners = usePortsStore((state) => state.listeners)
  const processByPid = usePortsStore((state) => state.processByPid)
  const serviceByPid = usePortsStore((state) => state.serviceByPid)
  const projectByPid = usePortsStore((state) => state.projectByPid)
  const loading = usePortsStore((state) => state.loading)
  const refreshing = usePortsStore((state) => state.refreshing)
  const error = usePortsStore((state) => state.error)
  const refreshListeners = usePortsStore((state) => state.refreshListeners)

  const projectGroups = useMemo(
    () =>
      groupByProject(
        groupListenersByProcess(listeners, processByPid, serviceByPid, projectByPid),
        projectByPid,
      ),
    [listeners, processByPid, serviceByPid, projectByPid],
  )

  return (
    <div className="mx-auto w-full max-w-5xl px-6 py-8">
      {/* Header */}
      <div className="mb-6">
        <div className="flex items-center justify-between">
          <h1 className="text-xl font-semibold text-slate-100">Projects</h1>
          <RefreshButton onClick={() => void refreshListeners()} refreshing={refreshing} />
        </div>
        <p className="mt-1 text-sm text-slate-500">
          Local source projects inferred from running processes: command-line paths are
          confirmed by project markers (package.json, Cargo.toml, pyproject.toml, go.mod)
          before any association is claimed.
        </p>
      </div>

      {error !== null ? (
        <div className="mb-4 rounded-lg border border-red-900/60 bg-red-950/30 px-4 py-3 text-sm text-red-300">
          <p className="font-medium">Discovery error</p>
          <p className="mt-1 text-xs text-red-400/80">{error}</p>
        </div>
      ) : null}

      {loading ? (
        <div className="rounded-lg border border-slate-800 bg-slate-900/40 px-4 py-10 text-center text-sm text-slate-500">
          Resolving projects from running processes…
        </div>
      ) : projectGroups.length === 0 ? (
        <div className="rounded-lg border border-dashed border-slate-800 p-10 text-center">
          <p className="text-sm text-slate-400">No running processes to associate yet.</p>
          <p className="mt-1 text-xs text-slate-600">
            Start a dev server — its project will appear here automatically within ~3 seconds.
          </p>
        </div>
      ) : (
        <ul className="space-y-3">
          {projectGroups.map((group, index) => (
            <ProjectCard key={group.project?.id ?? `unknown-${index}`} group={group} />
          ))}
        </ul>
      )}

      <p className="mt-3 text-xs text-slate-600">
        A project appears only when process evidence (executable path or command line)
        leads to a directory with real project markers — never from port numbers.
        Processes without credible evidence stay under “Unknown Project”. Manual refresh
        re-reads project metadata from disk.
      </p>
    </div>
  )
}
