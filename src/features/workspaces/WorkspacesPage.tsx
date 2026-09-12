import { useEffect, useRef, useState } from 'react'
import {
  ChevronDown,
  ChevronRight,
  FolderOpen,
  Network,
  Play,
  RotateCw,
  ShieldAlert,
  Terminal,
  Trash2,
  TriangleAlert,
} from 'lucide-react'

import { ConfirmButton } from '@/app/components/ControlActions'
import { RefreshButton } from '@/app/components/RefreshButton'
import { usePortListeners } from '@/hooks'
import { subscribeConflictsPolling, useConflictsStore } from '@/stores/conflictsStore'
import { usePortsStore } from '@/stores/portsStore'
import { subscribeWorkspacePolling, useWorkspaceStore } from '@/stores/workspaceStore'
import type {
  DependencyView,
  PortCandidate,
  PortConflictReport,
  ReadinessIssue,
  WorkspaceServiceView,
  WorkspaceStatus,
} from '@/types/domain'
import { formatTime } from '@/utils/format'

/** Tailwind classes per derived workspace status. */
const STATUS_STYLES: Record<WorkspaceStatus, string> = {
  stopped: 'border-slate-700 bg-slate-950 text-slate-400',
  starting: 'border-sky-900/60 bg-sky-950/40 text-sky-400',
  running: 'border-emerald-900/60 bg-emerald-950/40 text-emerald-400',
  partial: 'border-amber-900/60 bg-amber-950/40 text-amber-400',
  stopping: 'border-sky-900/60 bg-sky-950/40 text-sky-400',
  error: 'border-red-900/60 bg-red-950/40 text-red-400',
  conflict: 'border-red-900/60 bg-red-950/40 text-red-400',
}

/** Managed-state badge for one service row. */
function ManagedStateBadge({ managed }: { managed: WorkspaceServiceView['managed'] }) {
  if (managed === null) {
    return (
      <span className="rounded border border-slate-700 bg-slate-950 px-1.5 py-0.5 text-xs text-slate-500">
        stopped
      </span>
    )
  }
  const state = managed.state.state
  const styles: Record<string, string> = {
    starting: 'border-sky-900/60 bg-sky-950/40 text-sky-400',
    running: 'border-emerald-900/60 bg-emerald-950/40 text-emerald-400',
    degraded: 'border-amber-900/60 bg-amber-950/40 text-amber-400',
    stopping: 'border-sky-900/60 bg-sky-950/40 text-sky-400',
    stop_timeout: 'border-red-900/60 bg-red-950/40 text-red-400',
    start_failed: 'border-red-900/60 bg-red-950/40 text-red-400',
    exited: 'border-slate-700 bg-slate-950 text-slate-400',
    stopped: 'border-slate-700 bg-slate-950 text-slate-400',
  }
  const exit = managed.state
  const exitCode =
    (exit.state === 'start_failed' || exit.state === 'exited') && exit.exit_code !== null
      ? ` (code ${exit.exit_code})`
      : ''
  return (
    <span className={`rounded border px-1.5 py-0.5 text-xs ${styles[state] ?? ''}`}>
      {state}
      {exitCode}
    </span>
  )
}

/** Badge for one dependency's runtime state (listening — never health). */
function DependencyBadge({ dependency }: { dependency: DependencyView }) {
  const styles: Record<DependencyView['state'], string> = {
    available: 'border-emerald-900/60 bg-emerald-950/40 text-emerald-400',
    unavailable: 'border-red-900/60 bg-red-950/40 text-red-400',
    starting: 'border-sky-900/60 bg-sky-950/40 text-sky-400',
    unhealthy: 'border-amber-900/60 bg-amber-950/40 text-amber-400',
    unknown: 'border-slate-700 bg-slate-950 text-slate-500',
    conflicted: 'border-red-900/60 bg-red-950/40 text-red-400',
  }
  return (
    <div className="flex items-center gap-2 rounded-lg border border-slate-800 bg-slate-900/60 px-3 py-2">
      <span className="text-xs font-medium text-slate-200">{dependency.targetLabel}</span>
      <span className="text-xs text-slate-500">
        {dependency.required ? 'required' : 'optional'}
      </span>
      <span className={`rounded border px-1.5 py-0.5 text-xs ${styles[dependency.state]}`}>
        {dependency.state}
      </span>
    </div>
  )
}

/** Root-cause issues for one workspace (structured, evidence-based). */
function IssuesList({ issues }: { issues: ReadinessIssue[] }) {
  if (issues.length === 0) return null
  return (
    <div className="space-y-1.5">
      {issues.map((issue, index) => (
        <div
          key={`${issue.code}-${issue.dependencyId ?? ''}-${issue.port ?? ''}-${index}`}
          className="flex items-start gap-2 rounded-lg border border-amber-900/40 bg-amber-950/20 px-3 py-2"
        >
          <TriangleAlert
            className={`mt-0.5 h-3.5 w-3.5 shrink-0 ${
              issue.severity === 'error' ? 'text-red-400' : 'text-amber-400'
            }`}
            strokeWidth={1.8}
          />
          <div>
            <p className="text-xs font-medium text-slate-200">{issue.title}</p>
            <p className="text-xs text-slate-500">{issue.message}</p>
          </div>
        </div>
      ))}
    </div>
  )
}

/** The conflict dialog: honest ownership + advisory free-port suggestions.
 * Phase 10D (§W): Escape closes (non-destructive) and focus is pulled inside
 * on open so keyboard users are never tabbing "behind" the modal. */
function ConflictDialog({ report, suggestions }: { report: PortConflictReport; suggestions: PortCandidate[] }) {
  const clearPortReport = useConflictsStore((state) => state.clearPortReport)
  const closeRef = useRef<HTMLButtonElement>(null)
  const available = suggestions.filter((s) => s.status === 'available')
  const used = suggestions.filter((s) => s.status === 'used')

  useEffect(() => {
    closeRef.current?.focus()
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') clearPortReport()
    }
    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [clearPortReport])

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-6"
      role="dialog"
      aria-modal="true"
      aria-label="Port conflict"
      onClick={clearPortReport}
    >
      <div
        className="w-full max-w-lg rounded-xl border border-slate-700 bg-slate-900 shadow-2xl"
        onClick={(event) => event.stopPropagation()}
      >
        <div className="flex items-center gap-2 border-b border-slate-800 px-4 py-3">
          <ShieldAlert className="h-4 w-4 text-red-400" strokeWidth={1.8} />
          <h3 className="text-sm font-semibold text-slate-100">Port conflict — :{report.requestedPort}</h3>
        </div>
        <div className="space-y-3 px-4 py-3 text-sm">
          <p className="text-slate-300">{report.message}</p>
          {report.owner !== undefined && (
            <div className="rounded-lg border border-slate-800 bg-slate-950/60 px-3 py-2 text-xs text-slate-400">
              <p>
                <span className="text-slate-300">Owner:</span>{' '}
                {report.owner.serviceDisplayName ?? report.owner.processName ?? `PID ${report.owner.pid}`}
                {report.owner.processName !== undefined && ` (${report.owner.processName})`}
              </p>
              {report.owner.projectName !== undefined && (
                <p>Project: {report.owner.projectName}</p>
              )}
              <p>Lifecycle: {report.owner.lifecycle} · PID {report.owner.pid}</p>
            </div>
          )}
          {available.length > 0 && (
            <div>
              <p className="text-xs font-medium uppercase tracking-wider text-slate-500">
                Available alternatives (advisory only)
              </p>
              <div className="mt-1.5 flex flex-wrap gap-1.5">
                {available.map((s) => (
                  <span
                    key={s.port}
                    className="rounded border border-emerald-900/60 bg-emerald-950/40 px-2 py-0.5 font-mono text-xs text-emerald-400"
                  >
                    :{s.port}
                  </span>
                ))}
              </div>
            </div>
          )}
          {used.length > 0 && (
            <div>
              <p className="text-xs font-medium uppercase tracking-wider text-slate-500">Used</p>
              <div className="mt-1.5 space-y-1">
                {used.map((s) => (
                  <p key={s.port} className="font-mono text-xs text-slate-500">
                    :{s.port} — {s.usedBy ?? 'unknown'}
                  </p>
                ))}
              </div>
            </div>
          )}
          <p className="text-xs text-slate-600">
            LocalStack never edits project files or stops the owner automatically — decide what to
            change yourself.
          </p>
        </div>
        <div className="flex justify-end border-t border-slate-800 px-4 py-3">
          <button
            ref={closeRef}
            type="button"
            onClick={clearPortReport}
            className="rounded border border-slate-700 bg-slate-900 px-3 py-1.5 text-xs text-slate-300 hover:border-slate-600"
          >
            Close
          </button>
        </div>
      </div>
    </div>
  )
}

/** The log panel: incremental polling, auto-scroll, stream distinction. */
function LogPanel({ name }: { name: string }) {
  const logLines = useWorkspaceStore((state) => state.logLines)
  const pollLogs = useWorkspaceStore((state) => state.pollLogs)
  const openLogs = useWorkspaceStore((state) => state.openLogs)
  const clearLogs = useWorkspaceStore((state) => state.clearLogs)
  const scrollRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    const timer = setInterval(() => void pollLogs(), 500)
    return () => clearInterval(timer)
  }, [pollLogs])

  useEffect(() => {
    const el = scrollRef.current
    if (el !== null) {
      el.scrollTop = el.scrollHeight
    }
  }, [logLines.length])

  return (
    <div className="mt-3 overflow-hidden rounded-lg border border-slate-800 bg-slate-950">
      <div className="flex items-center justify-between border-b border-slate-800 px-3 py-2">
        <span className="flex items-center gap-2 text-xs font-semibold text-slate-300">
          <Terminal className="h-3.5 w-3.5 text-slate-400" strokeWidth={1.8} />
          {name} — output
        </span>
        <div className="flex items-center gap-2">
          <button
            type="button"
            onClick={clearLogs}
            className="text-xs text-slate-500 hover:text-slate-300"
          >
            Clear view
          </button>
          <button
            type="button"
            onClick={() => openLogs(null)}
            className="text-xs text-slate-500 hover:text-slate-300"
          >
            Close
          </button>
        </div>
      </div>
      <div ref={scrollRef} className="max-h-64 overflow-y-auto px-3 py-2 font-mono text-xs leading-5">
        {logLines.length === 0 ? (
          <p className="text-slate-600">Waiting for output…</p>
        ) : (
          logLines.map((entry, index) => (
            <div key={index} className="flex gap-3">
              <span className="shrink-0 text-slate-600">{formatTime(entry.at)}</span>
              <span
                className={`shrink-0 ${entry.stream === 'stderr' ? 'text-red-400' : 'text-slate-600'}`}
              >
                {entry.stream}
              </span>
              <span className="whitespace-pre-wrap text-slate-300">{entry.line}</span>
            </div>
          ))
        )}
      </div>
    </div>
  )
}

/** One managed service row with its lifecycle actions. */
function ServiceRow({ service }: { service: WorkspaceServiceView }) {
  const startService = useWorkspaceStore((state) => state.startService)
  const stopService = useWorkspaceStore((state) => state.stopService)
  const restartService = useWorkspaceStore((state) => state.restartService)
  const openLogs = useWorkspaceStore((state) => state.openLogs)
  const openLogManagedId = useWorkspaceStore((state) => state.openLogManagedId)
  const [expanded, setExpanded] = useState(false)
  const [busy, setBusy] = useState(false)

  const managed = service.managed
  const alive = managed !== null && ['starting', 'running', 'stopping', 'degraded'].includes(managed.state.state)
  const stopTimedOut = managed !== null && managed.state.state === 'stop_timeout'
  const canStart = managed === null || !alive
  const canStop = alive
  const canRestart = alive

  async function run(action: () => Promise<unknown>): Promise<void> {
    setBusy(true)
    try {
      await action()
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="rounded-lg border border-slate-800 bg-slate-900/60">
      <div className="flex items-center gap-3 px-4 py-3">
        <button
          type="button"
          onClick={() => setExpanded((value) => !value)}
          className="text-slate-500 hover:text-slate-300"
          aria-label={expanded ? 'Collapse details' : 'Expand details'}
        >
          {expanded ? <ChevronDown className="h-4 w-4" /> : <ChevronRight className="h-4 w-4" />}
        </button>
        <span className="text-sm font-semibold text-slate-100">{service.name}</span>
        <span className="rounded border border-slate-700 bg-slate-950 px-1.5 py-0.5 text-xs capitalize text-slate-400">
          {service.role}
        </span>
        {service.expectedPort !== null && (
          <span className="rounded border border-slate-700 bg-slate-950 px-1.5 py-0.5 font-mono text-xs text-slate-400">
            :{service.expectedPort}
          </span>
        )}
        <ManagedStateBadge managed={managed} />
        {managed !== null && (
          <span className="font-mono text-xs text-slate-500">PID {managed.rootPid}</span>
        )}
        <span className="flex-1" />
        {canStart && (
          <button
            type="button"
            disabled={busy}
            onClick={() => void run(() => startService(service.launchSpecId, service.name))}
            className="flex items-center gap-1.5 rounded border border-emerald-900/60 bg-emerald-950/40 px-2.5 py-1 text-xs text-emerald-400 transition-colors hover:bg-emerald-950/70 disabled:opacity-50"
          >
            <Play className="h-3 w-3" strokeWidth={1.8} /> Start
          </button>
        )}
        {canRestart && (
          <button
            type="button"
            disabled={busy}
            onClick={() => void run(() => restartService(managed?.managedId ?? '', service.name))}
            className="flex items-center gap-1.5 rounded border border-sky-900/60 bg-sky-950/40 px-2.5 py-1 text-xs text-sky-400 transition-colors hover:bg-sky-950/70 disabled:opacity-50"
          >
            <RotateCw className="h-3 w-3" strokeWidth={1.8} /> Restart
          </button>
        )}
        {canStop && (
          <ConfirmButton
            label="Stop"
            confirmLabel="Confirm stop"
            disabled={busy}
            onConfirm={() => void run(() => stopService(managed?.managedId ?? '', service.name))}
            className="flex items-center gap-1.5 rounded border border-amber-900/60 bg-amber-950/40 px-2.5 py-1 text-xs text-amber-400 transition-colors hover:bg-amber-950/70 disabled:opacity-50"
          />
        )}
        {stopTimedOut && managed !== null && (
          <ConfirmButton
            label="Force stop"
            confirmLabel="Confirm force stop"
            disabled={busy}
            onConfirm={() => void run(() => stopService(managed.managedId, service.name, true))}
            className="flex items-center gap-1.5 rounded border border-red-900/60 bg-red-950/40 px-2.5 py-1 text-xs text-red-400 transition-colors hover:bg-red-950/70 disabled:opacity-50"
          />
        )}
        {managed !== null && alive && (
          <button
            type="button"
            onClick={() => openLogs(openLogManagedId === managed.managedId ? null : managed.managedId)}
            className="flex items-center gap-1.5 rounded border border-slate-700 bg-slate-900 px-2.5 py-1 text-xs text-slate-400 transition-colors hover:text-slate-200"
          >
            <Terminal className="h-3 w-3" strokeWidth={1.8} /> Logs
          </button>
        )}
      </div>
      {expanded && (
        <div className="border-t border-slate-800/60 px-4 py-2 text-xs text-slate-500">
          <p>
            <span className="text-slate-400">Launch spec:</span>{' '}
            <span className="font-mono">{service.source}</span>{' '}
            (managed by LocalStack — the frontend only references the opaque spec id)
          </p>
        </div>
      )}
      {openLogManagedId === managed?.managedId && (
        <div className="px-4 pb-3">
          <LogPanel name={service.name} />
        </div>
      )}
    </div>
  )
}

/**
 * Workspaces — managed service lifecycle. A workspace exists only after
 * the user explicitly creates it for a project; its launch specs are
 * backend-derived and referenced by opaque ids. External dependencies
 * (detected PostgreSQL, Ollama, Docker…) are shown as external facts and
 * never started or stopped from here.
 */
export function WorkspacesPage() {
  usePortListeners()
  const load = useWorkspaceStore((state) => state.load)
  const refresh = useWorkspaceStore((state) => state.refresh)
  const workspaces = useWorkspaceStore((state) => state.workspaces)
  const loading = useWorkspaceStore((state) => state.loading)
  const refreshing = useWorkspaceStore((state) => state.refreshing)
  const error = useWorkspaceStore((state) => state.error)
  const startWorkspace = useWorkspaceStore((state) => state.startWorkspace)
  const stopWorkspace = useWorkspaceStore((state) => state.stopWorkspace)
  const projects = usePortsStore((state) => state.projects)
  const loadConflicts = useConflictsStore((state) => state.load)
  const readiness = useConflictsStore((state) => state.readiness)
  const lastPortReport = useConflictsStore((state) => state.lastPortReport)
  const portSuggestions = useConflictsStore((state) => state.portSuggestions)
  const evaluateConflict = useConflictsStore((state) => state.evaluateConflict)
  const [creatingFor, setCreatingFor] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [createError, setCreateError] = useState<string | null>(null)

  useEffect(() => {
    void load()
    void loadConflicts()
    const releaseWorkspace = subscribeWorkspacePolling('workspaces')
    const releaseConflicts = subscribeConflictsPolling('workspaces')
    return () => {
      releaseWorkspace()
      releaseConflicts()
    }
  }, [load, loadConflicts])

  const projectsWithoutWorkspace = projects.filter(
    (project) => !workspaces.some((w) => w.projectRoot === project.rootPath),
  )

  async function handleCreate(root: string): Promise<void> {
    setBusy(true)
    setCreateError(null)
    try {
      await useWorkspaceStore.getState().create(root)
      setCreatingFor(null)
    } catch (cause) {
      setCreateError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="mx-auto w-full max-w-5xl px-6 py-8">
      <div className="mb-6">
        <div className="flex items-center justify-between">
          <h1 className="text-xl font-semibold text-slate-100">Workspaces</h1>
          <RefreshButton onClick={() => void refresh()} refreshing={refreshing} />
        </div>
        <p className="mt-1 text-sm text-slate-500">
          Managed service lifecycle. Creating a workspace is always an explicit user action;
          launch commands are derived by LocalStack from project manifests, never typed in.
        </p>
      </div>

      {error !== null && (
        <div className="mb-4 rounded-lg border border-red-900/60 bg-red-950/30 px-4 py-3 text-sm text-red-300">
          {error}
        </div>
      )}

      {lastPortReport !== null && (
        <ConflictDialog report={lastPortReport} suggestions={portSuggestions} />
      )}

      {loading ? (
        <div className="rounded-lg border border-slate-800 bg-slate-900/40 px-4 py-6 text-center text-sm text-slate-500">
          Loading workspaces…
        </div>
      ) : workspaces.length === 0 ? (
        <div className="rounded-lg border border-dashed border-slate-800 p-10 text-center">
          <p className="text-sm text-slate-400">No workspaces yet.</p>
          <p className="mt-1 text-xs text-slate-600">
            Create one from a detected project below. LocalStack will suggest launch commands it
            found in the project's own manifest files.
          </p>
        </div>
      ) : (
        <div className="space-y-6">
          {workspaces.map((workspace) => (
            <section key={workspace.id} className="rounded-lg border border-slate-800">
              <div className="flex items-center gap-3 border-b border-slate-800 bg-slate-900/60 px-4 py-3">
                <FolderOpen className="h-4 w-4 text-violet-400" strokeWidth={1.8} />
                <h2 className="text-sm font-semibold text-slate-100">{workspace.name}</h2>
                <span
                  className={`rounded border px-1.5 py-0.5 text-xs ${STATUS_STYLES[workspace.status]}`}
                >
                  {workspace.status}
                </span>
                <span
                  className="flex-1 truncate font-mono text-xs text-slate-500"
                  title={workspace.projectRoot}
                >
                  {workspace.projectRoot}
                </span>
                <button
                  type="button"
                  onClick={() => void startWorkspace(workspace.id, workspace.name)}
                  className="rounded border border-emerald-900/60 bg-emerald-950/40 px-2.5 py-1 text-xs text-emerald-400 hover:bg-emerald-950/70"
                >
                  Start workspace
                </button>
                <ConfirmButton
                  label="Stop managed"
                  confirmLabel="Confirm stop"
                  onConfirm={() => void stopWorkspace(workspace.id, workspace.name)}
                  className="rounded border border-amber-900/60 bg-amber-950/40 px-2.5 py-1 text-xs text-amber-400 hover:bg-amber-950/70"
                />
                <button
                  type="button"
                  onClick={() => void useWorkspaceStore.getState().remove(workspace.id)}
                  className="rounded border border-slate-700 bg-slate-900 px-2 py-1 text-slate-500 hover:text-slate-300"
                  aria-label="Remove workspace (managed processes keep running)"
                  title="Remove workspace — running managed processes are NOT killed"
                >
                  <Trash2 className="h-3.5 w-3.5" strokeWidth={1.8} />
                </button>
              </div>
              <div className="space-y-2 p-3">
                {(() => {
                  const view = readiness.find((r) => r.workspaceId === workspace.id)
                  return view !== undefined && (view.issues.length > 0 || view.dependencies.length > 0) ? (
                    <div className="space-y-2 rounded-lg border border-slate-800/60 bg-slate-950/40 p-3">
                      {view.dependencies.length > 0 && (
                        <div>
                          <p className="mb-1.5 flex items-center gap-1.5 text-xs font-semibold uppercase tracking-wider text-slate-500">
                            <Network className="h-3 w-3" strokeWidth={1.8} /> Dependencies
                          </p>
                          <div className="flex flex-wrap gap-2">
                            {view.dependencies.map((d) => (
                              <DependencyBadge key={d.id} dependency={d} />
                            ))}
                          </div>
                        </div>
                      )}
                      <IssuesList issues={view.issues} />
                      {view.conflicts.length > 0 && (
                        <button
                          type="button"
                          onClick={() => void evaluateConflict(view.conflicts[0]!.requestedPort)}
                          className="flex items-center gap-1.5 rounded border border-red-900/60 bg-red-950/40 px-2.5 py-1 text-xs text-red-300 hover:bg-red-950/70"
                        >
                          <ShieldAlert className="h-3 w-3" strokeWidth={1.8} /> Show conflict details
                        </button>
                      )}
                    </div>
                  ) : null
                })()}
                {workspace.services.map((service) => (
                  <ServiceRow key={service.id} service={service} />
                ))}
                {workspace.services.length === 0 && (
                  <p className="px-2 py-3 text-center text-xs text-slate-500">
                    No manageable services were detected in this project's manifests.
                  </p>
                )}
              </div>
            </section>
          ))}
        </div>
      )}

      {/* Create-workspace entry from detected projects */}
      <section className="mt-8" aria-labelledby="create-workspace-heading">
        <h2
          id="create-workspace-heading"
          className="mb-3 text-sm font-semibold uppercase tracking-wider text-slate-400"
        >
          Create a workspace
        </h2>
        {createError !== null && (
          <p className="mb-3 rounded border border-red-900/60 bg-red-950/30 px-3 py-2 text-xs text-red-300">
            {createError}
          </p>
        )}
        {projectsWithoutWorkspace.length === 0 ? (
          <p className="rounded-lg border border-dashed border-slate-800 p-6 text-center text-sm text-slate-500">
            {projects.length === 0
              ? 'No running projects detected yet — start a dev server and its project appears here.'
              : 'Every detected project already has a workspace.'}
          </p>
        ) : (
          <ul className="space-y-2">
            {projectsWithoutWorkspace.map((project) => (
              <li
                key={project.id}
                className="flex items-center gap-3 rounded-lg border border-slate-800 bg-slate-900/60 px-4 py-3"
              >
                <span className="text-sm font-medium text-slate-200">{project.name}</span>
                <span className="flex-1 truncate font-mono text-xs text-slate-500" title={project.rootPath}>
                  {project.rootPath}
                </span>
                {creatingFor === project.id ? (
                  busy ? (
                    <span className="text-xs text-slate-500">Creating…</span>
                  ) : (
                    <button
                      type="button"
                      onClick={() => void handleCreate(project.rootPath)}
                      className="rounded border border-emerald-900/60 bg-emerald-950/40 px-2.5 py-1 text-xs text-emerald-400 hover:bg-emerald-950/70"
                    >
                      Confirm create
                    </button>
                  )
                ) : (
                  <button
                    type="button"
                    onClick={() => setCreatingFor(project.id)}
                    className="rounded border border-slate-700 bg-slate-900 px-2.5 py-1 text-xs text-slate-300 hover:border-slate-600"
                  >
                    Create workspace
                  </button>
                )}
              </li>
            ))}
          </ul>
        )}
      </section>

      <p className="mt-6 text-xs text-slate-600">
        Managed services are processes LocalStack launched itself — they alone get graceful
        targeted stops, restart, and log capture. Externally discovered services keep the Phase 5
        control path and are never auto-restarted. Removing a workspace or closing LocalStack
        never kills running managed processes.
      </p>
    </div>
  )
}
