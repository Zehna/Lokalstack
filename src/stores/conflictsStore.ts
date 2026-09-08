import { create } from 'zustand'

import {
  addWorkspaceDependency,
  evaluatePort,
  findFreePorts,
  getWorkspacesReadiness,
  listDependencyTargets,
  removeWorkspaceDependency,
} from '@/services/native/ports'
import { useControlStore } from '@/stores/controlStore'
import type {
  DependencyTarget,
  PortCandidate,
  PortConflictReport,
  TargetOption,
  WorkspaceReadinessView,
} from '@/types/domain'

/** Session audit trail actions for Phase 7 intelligence. */
export type ConflictAction =
  | 'conflict_detected'
  | 'conflict_resolved'
  | 'dependency_unavailable'
  | 'dependency_recovered'
  | 'workspace_blocked'
  | 'workspace_recovered'
  | 'free_port_suggested'

/** How often the readiness view is re-evaluated (aligned with workspace poll). */
const READINESS_POLL_MS = 2_000

interface ConflictsState {
  /** Readiness per workspace from the latest poll. */
  readiness: WorkspaceReadinessView[]
  loading: boolean
  refreshing: boolean
  error: string | null
  lastUpdated: number | null

  /** Latest per-port conflict report (from evaluatePort). */
  lastPortReport: PortConflictReport | null
  /** Free-port suggestions for the last evaluated port. */
  portSuggestions: PortCandidate[]

  load: () => Promise<void>
  refresh: () => Promise<void>
  /** Evaluate one port and fetch advisory alternatives. */
  evaluateConflict: (port: number) => Promise<void>
  clearPortReport: () => void

  addDependency: (
    workspaceId: string,
    sourceServiceId: string,
    target: DependencyTarget,
    required: boolean,
  ) => Promise<void>
  removeDependency: (workspaceId: string, dependencyId: string) => Promise<void>
  dependencyTargets: (workspaceId: string) => Promise<TargetOption[]>
}

let pollTimer: ReturnType<typeof setInterval> | null = null
let nextEntryId = 1

function record(
  action: ConflictAction,
  subject: string,
  outcome: 'success' | 'failure' | 'stale',
  message: string,
): void {
  const control = useControlStore.getState()
  const next = [
    ...control.history,
    { id: nextEntryId++, at: Date.now(), action, subject, pid: null, outcome, message },
  ].slice(-100)
  useControlStore.setState({ history: next })
}

/**
 * Port conflict + dependency readiness state. Purely derived from the
 * backend's own evaluation — no ownership or dependency logic lives here.
 * History records *transitions* (deduped by stable issue identities), not
 * polling noise (spec §41–42).
 */
export const useConflictsStore = create<ConflictsState>()((set, get) => {
  /** Stable issue keys seen on the previous cycle → deduped transitions. */
  let seenIssues = new Set<string>()
  let seenStatus = new Map<string, string>()

  function issueKeys(views: WorkspaceReadinessView[]): Set<string> {
    const keys = new Set<string>()
    for (const view of views) {
      for (const issue of view.issues) {
        keys.add(`${view.workspaceId}|${issue.code}|${issue.dependencyId ?? ''}|${issue.port ?? ''}`)
      }
    }
    return keys
  }

  function recordTransitions(views: WorkspaceReadinessView[]): void {
    const now = issueKeys(views)
    for (const view of views) {
      const depsUnavailable = new Map<string, string>()
      for (const issue of view.issues) {
        if (issue.code === 'DEPENDENCY_UNAVAILABLE') {
          depsUnavailable.set(issue.dependencyId ?? issue.message, issue.message)
        }
      }
      // Dependency transitions: one event per change, not per poll.
      for (const [depId, message] of depsUnavailable) {
        const key = `${view.workspaceId}|${depId}`
        if (!seenIssues.has(key)) {
          record('dependency_unavailable', view.workspaceName, 'stale', message)
        }
      }
      const prevUnavailableForWorkspace = [...seenIssues].filter(
        (k) => k.startsWith(`${view.workspaceId}|`) && k.includes('DEPENDENCY_UNAVAILABLE'),
      )
      for (const key of prevUnavailableForWorkspace) {
        const depId = key.split('|').slice(1).join('|')
        if (!depsUnavailable.has(depId)) {
          record('dependency_recovered', view.workspaceName, 'success', 'Dependency became available again.')
        }
      }
    }

    // Workspace-level blocked/recovered transitions.
    for (const view of views) {
      const prev = seenStatus.get(view.workspaceId)
      if (prev !== view.status) {
        if (view.status === 'blocked' && prev !== undefined) {
          record('workspace_blocked', view.workspaceName, 'stale', 'Workspace became blocked.')
        }
        if (prev === 'blocked' && view.status !== 'blocked') {
          record('workspace_recovered', view.workspaceName, 'success', 'Workspace is no longer blocked.')
        }
        seenStatus.set(view.workspaceId, view.status)
      }
    }
    seenIssues = now
  }

  function ensurePolling(): void {
    if (pollTimer !== null) return
    pollTimer = setInterval(() => {
      void get().refresh()
    }, READINESS_POLL_MS)
  }

  async function runRefresh(): Promise<void> {
    if (get().refreshing) return
    set({ refreshing: true })
    try {
      const readiness = await getWorkspacesReadiness()
      recordTransitions(readiness)
      set({ readiness, error: null, lastUpdated: Date.now(), loading: false, refreshing: false })
    } catch (cause) {
      set({
        loading: false,
        refreshing: false,
        error: cause instanceof Error ? cause.message : String(cause),
      })
    }
  }

  return {
    readiness: [],
    loading: true,
    refreshing: false,
    error: null,
    lastUpdated: null,
    lastPortReport: null,
    portSuggestions: [],

    load: async () => {
      ensurePolling()
      await runRefresh()
    },

    refresh: async () => {
      await runRefresh()
    },

    evaluateConflict: async (port) => {
      try {
        const report = await evaluatePort(port)
        const suggestions = await findFreePorts(port)
        set({ lastPortReport: report, portSuggestions: suggestions })
        if (report.kind !== 'no_conflict' && report.kind !== 'already_running') {
          record(
            'conflict_detected',
            `Port ${port}`,
            'stale',
            report.message,
          )
          record('free_port_suggested', `Port ${port}`, 'success', 'Advisory alternatives computed.')
        }
      } catch (cause) {
        record(
          'conflict_detected',
          `Port ${port}`,
          'failure',
          cause instanceof Error ? cause.message : String(cause),
        )
        throw cause
      }
    },

    clearPortReport: () => set({ lastPortReport: null, portSuggestions: [] }),

    addDependency: async (workspaceId, sourceServiceId, target, required) => {
      await addWorkspaceDependency(workspaceId, sourceServiceId, target, required)
      await get().refresh()
    },

    removeDependency: async (workspaceId, dependencyId) => {
      await removeWorkspaceDependency(workspaceId, dependencyId)
      await get().refresh()
    },

    dependencyTargets: (workspaceId) => listDependencyTargets(workspaceId),
  }
})
