import { create } from 'zustand'

import {
  createWorkspace,
  getServiceLogs,
  listWorkspaces,
  removeWorkspace,
  restartManagedService,
  startManagedService,
  startWorkspaceServices,
  stopManagedService,
  stopWorkspaceServices,
} from '@/services/native/ports'
import { useControlStore } from '@/stores/controlStore'
import type { LogLine, ManagedActionOutcome, WorkspaceView } from '@/types/domain'

/** Workspace lifecycle actions for the session audit trail. */
export type WorkspaceAction =
  | 'workspace_start'
  | 'workspace_stop'
  | 'service_start'
  | 'service_stop'
  | 'service_force_stop'
  | 'service_restart'
  | 'workspace_create'
  | 'startup_failure'
  | 'port_conflict'

/** How often the workspace list (managed states) refreshes. */
const WORKSPACE_POLL_MS = 2_000

interface WorkspaceState {
  workspaces: WorkspaceView[]
  loading: boolean
  refreshing: boolean
  error: string | null
  lastUpdated: number | null
  /** Which managed service's logs are open (log viewer). */
  openLogManagedId: string | null
  /** Accumulated log lines for the open service (newest last). */
  logLines: LogLine[]
  /** Last polled log index (incremental fetch cursor). */
  logCursor: number

  load: () => Promise<void>
  refresh: () => Promise<void>
  create: (projectRoot: string) => Promise<WorkspaceView>
  remove: (workspaceId: string) => Promise<void>

  startService: (launchSpecId: string, subject: string) => Promise<ManagedActionOutcome>
  stopService: (managedId: string, subject: string, force?: boolean) => Promise<ManagedActionOutcome>
  restartService: (managedId: string, subject: string) => Promise<ManagedActionOutcome>
  startWorkspace: (workspaceId: string, subject: string) => Promise<ManagedActionOutcome[]>
  stopWorkspace: (workspaceId: string, subject: string) => Promise<ManagedActionOutcome[]>

  openLogs: (managedId: string | null) => void
  pollLogs: () => Promise<void>
  clearLogs: () => void
}

let pollTimer: ReturnType<typeof setInterval> | null = null
let nextEntryId = 1

function record(
  action: WorkspaceAction,
  subject: string,
  outcome: 'success' | 'failure' | 'stale',
  message: string,
  pid: number | null = null,
): void {
  const control = useControlStore.getState()
  const next = [
    ...control.history,
    { id: nextEntryId++, at: Date.now(), action, subject, pid, outcome, message },
  ].slice(-100)
  useControlStore.setState({ history: next })
}

/** Classify a backend refusal into an honest history outcome. */
function classify(code: string, message: string): 'success' | 'failure' | 'stale' {
  if (code === 'PORT_CONFLICT') return 'stale'
  if (
    code.startsWith('UNKNOWN_') ||
    code.startsWith('STALE_') ||
    code === 'ALREADY_RUNNING' ||
    code === 'NOT_RUNNING'
  ) {
    return 'stale'
  }
  void message
  return 'failure'
}

function outcomeMessage(outcome: ManagedActionOutcome): string {
  return outcome.message
}

/**
 * Workspace lifecycle state: managed start/stop/restart with honest
 * refusal handling and a session audit trail. All authority stays
 * server-side — this store only ever passes opaque ids back.
 */
export const useWorkspaceStore = create<WorkspaceState>()((set, get) => {
  function ensurePolling(): void {
    if (pollTimer !== null) return
    pollTimer = setInterval(() => {
      void get().refresh()
    }, WORKSPACE_POLL_MS)
  }

  async function runRefresh(): Promise<void> {
    if (get().refreshing) return
    set({ refreshing: true })
    try {
      const workspaces = await listWorkspaces()
      set({
        workspaces,
        error: null,
        lastUpdated: Date.now(),
        loading: false,
        refreshing: false,
      })
    } catch (cause) {
      set({
        loading: false,
        refreshing: false,
        error: cause instanceof Error ? cause.message : String(cause),
      })
    }
  }

  return {
    workspaces: [],
    loading: true,
    refreshing: false,
    error: null,
    lastUpdated: null,
    openLogManagedId: null,
    logLines: [],
    logCursor: 0,

    load: async () => {
      if (get().loading === false && get().workspaces.length === 0) {
        set({ loading: true })
      }
      ensurePolling()
      await runRefresh()
    },

    refresh: async () => {
      await runRefresh()
    },

    create: async (projectRoot) => {
      try {
        const workspace = await createWorkspace(projectRoot)
        record('workspace_create', workspace.name, 'success', `Workspace created for ${projectRoot}`)
        set((state) => ({
          workspaces: [...state.workspaces, workspace],
        }))
        return workspace
      } catch (cause) {
        const message = cause instanceof Error ? cause.message : String(cause)
        record('workspace_create', projectRoot, 'failure', message)
        throw cause
      }
    },

    remove: async (workspaceId) => {
      await removeWorkspace(workspaceId)
      set((state) => ({
        workspaces: state.workspaces.filter((w) => w.id !== workspaceId),
      }))
    },

    startService: async (launchSpecId, subject) => {
      const outcome = await startManagedService(launchSpecId)
      if (outcome.ok) {
        record('service_start', subject, 'success', outcomeMessage(outcome))
      } else if (outcome.code === 'PORT_CONFLICT') {
        record('port_conflict', subject, 'stale', outcomeMessage(outcome))
      } else if (outcome.code === 'ALREADY_RUNNING') {
        record('service_start', subject, 'stale', outcomeMessage(outcome))
      } else {
        record('service_start', subject, classify(outcome.code, outcome.message), outcomeMessage(outcome))
      }
      void get().refresh()
      return outcome
    },

    stopService: async (managedId, subject, force = false) => {
      const outcome = await stopManagedService(managedId, force)
      if (outcome.ok) {
        record(
          force ? 'service_force_stop' : 'service_stop',
          subject,
          'success',
          outcomeMessage(outcome),
        )
      } else if (outcome.code === 'STOP_TIMEOUT') {
        record('service_stop', subject, 'failure', outcomeMessage(outcome))
      } else {
        record(
          force ? 'service_force_stop' : 'service_stop',
          subject,
          classify(outcome.code, outcome.message),
          outcomeMessage(outcome),
        )
      }
      void get().refresh()
      return outcome
    },

    restartService: async (managedId, subject) => {
      const outcome = await restartManagedService(managedId)
      record(
        'service_restart',
        subject,
        outcome.ok ? 'success' : classify(outcome.code, outcome.message),
        outcomeMessage(outcome),
      )
      void get().refresh()
      return outcome
    },

    startWorkspace: async (workspaceId, subject) => {
      const outcomes = await startWorkspaceServices(workspaceId)
      const failed = outcomes.find((o) => !o.ok)
      if (failed !== undefined && failed.code === 'PORT_CONFLICT') {
        record('port_conflict', subject, 'stale', failed.message)
      }
      record(
        'workspace_start',
        subject,
        outcomes.every((o) => o.ok) ? 'success' : 'failure',
        failed !== undefined
          ? `Stopped early: ${failed.message}`
          : `${outcomes.length} service(s) started.`,
      )
      void get().refresh()
      return outcomes
    },

    stopWorkspace: async (workspaceId, subject) => {
      const outcomes = await stopWorkspaceServices(workspaceId)
      record(
        'workspace_stop',
        subject,
        outcomes.every((o) => o.ok) || outcomes.length === 0 ? 'success' : 'failure',
        outcomes.length === 0
          ? 'No managed processes were running.'
          : `${outcomes.filter((o) => o.ok).length}/${outcomes.length} stopped.`,
      )
      void get().refresh()
      return outcomes
    },

    openLogs: (managedId) => {
      set({ openLogManagedId: managedId, logLines: [], logCursor: 0 })
    },

    pollLogs: async () => {
      const managedId = get().openLogManagedId
      if (managedId === null) return
      try {
        const batch = await getServiceLogs(managedId, get().logCursor)
        if (batch.lines.length > 0) {
          set((state) => ({
            logLines: [...state.logLines, ...batch.lines].slice(-1_000),
            logCursor: batch.lastIndex,
          }))
        } else {
          set({ logCursor: batch.lastIndex })
        }
      } catch {
        // The managed process may have exited; the next refresh updates
        // the state view. Log polling failures are not fatal.
      }
    },

    clearLogs: () => set({ logLines: [], logCursor: 0 }),
  }
})
