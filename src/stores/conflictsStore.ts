import { create } from 'zustand'

import { createPollingOwner } from '@/stores/pollingOwner'
import {
  addWorkspaceDependency,
  evaluatePort,
  findFreePorts,
  getWorkspacesReadiness,
  listDependencyTargets,
  removeWorkspaceDependency,
} from '@/services/native/ports'
import { recordAuditEntry } from '@/stores/auditTrail'
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

/* -------------------------------------------------------------------------
 * Polling lifecycle — named-subscription, single shared timer (Phase 10A
 * ownership audit). Each consumer acquires by stable id and receives a
 * structurally-paired, idempotent release closure. Duplicate acquisition
 * from one consumer = ONE subscription; release cannot underflow.
 * ---------------------------------------------------------------------- */

const conflictsPollOwner = createPollingOwner(
  READINESS_POLL_MS,
  () => {
    void useConflictsStore.getState().refresh()
  },
  () => {
    void useConflictsStore.getState().refresh()
  },
)

/**
 * Acquire the 2 s readiness cycle for one logical consumer. Idempotent per
 * consumer. Returns the release closure for the consumer's cleanup.
 */
export function subscribeConflictsPolling(consumerId: string): () => void {
  return conflictsPollOwner.acquire(consumerId)
}

/** Convenience acquire with an anonymous consumer id. */
export function startConflictsPolling(): () => void {
  return subscribeConflictsPolling('anonymous')
}

/** Test observability: distinct consumers / timer state. */
export function conflictsPollingSubscribers(): number {
  return conflictsPollOwner.consumerCount()
}

/** Test observability: whether the shared interval exists. */
export function conflictsPollingRunning(): boolean {
  return conflictsPollOwner.isRunning()
}

/** Test-only teardown: release every consumer and stop the timer. */
export function resetConflictsPolling(): void {
  conflictsPollOwner.releaseAll()
}

// Phase 10C (spec §I): readiness polling follows the base Auto Refresh
// setting (it is the workspace/readiness intelligence layer).
import { registerPollingApplier } from '@/stores/settingsStore'

registerPollingApplier((settings) => {
  conflictsPollOwner.configure({ enabled: settings.autoRefresh })
})

/**
 * Transition state — module-level because the store instance can be reset in
 * tests while the transition machine must stay consistent across `refresh()`
 * calls. Reset via `resetConflictsTransitions()` in test setup.
 *
 * - `unavailableDeps`: workspace|dependency keys currently UNAVAILABLE.
 * - `conflictKeys`: workspace|service|port|owner keys currently in conflict.
 * - `seenStatus`: last workspace status for blocked/recovered edges.
 */
let unavailableDeps = new Set<string>()
let conflictKeys = new Set<string>()
let seenStatus = new Map<string, string>()

/** Test helper: clear the transition machine's previous-state snapshots. */
export function resetConflictsTransitions(): void {
  unavailableDeps = new Set()
  conflictKeys = new Set()
  seenStatus = new Map()
}

function record(
  action: ConflictAction,
  subject: string,
  outcome: 'success' | 'failure' | 'stale',
  message: string,
): void {
  recordAuditEntry(action, subject, outcome, message)
}

/** Stable identity for an unavailable dependency: workspace + dependency id. */
function dependencyKey(workspaceId: string, dependencyId: string): string {
  return `${workspaceId}|${dependencyId}`
}

/** Stable identity for one port conflict: workspace + service + port + owner. */
function conflictKey(report: PortConflictReport): string {
  const owner = report.owner
  return [
    report.requestedBy.workspaceId ?? '',
    report.requestedBy.serviceId ?? '',
    String(report.requestedPort),
    report.kind,
    owner ? `${owner.pid}:${owner.lifecycle}` : '',
  ].join('|')
}

/**
 * Dependency transition machine (Phase 10A, spec §10):
 *
 *   AVAILABLE   → AVAILABLE   : none
 *   AVAILABLE   → UNAVAILABLE : dependency_unavailable (once)
 *   UNAVAILABLE → UNAVAILABLE : none
 *   UNAVAILABLE → AVAILABLE   : dependency_recovered (once)
 *
 * Stable identity = workspaceId + dependencyId — never array index, never
 * display name. Recovery requires the same dependency to be *present and
 * available* in the new poll; a dependency absent from a poll (workspace
 * removed) clears silently — no fabricated recovery.
 */
function recordDependencyTransitions(views: WorkspaceReadinessView[]): void {
  const nowUnavailable = new Set<string>()
  for (const view of views) {
    for (const issue of view.issues) {
      if (issue.code === 'DEPENDENCY_UNAVAILABLE') {
        nowUnavailable.add(dependencyKey(view.workspaceId, issue.dependencyId ?? issue.message))
      }
    }
  }

  // UNAVAILABLE is in the previous set but not the current one — check whether
  // the same dependency is now present and available before declaring recovery.
  for (const key of unavailableDeps) {
    if (nowUnavailable.has(key)) continue
    const [workspaceId, depId] = [key.slice(0, key.indexOf('|')), key.slice(key.indexOf('|') + 1)]
    const view = views.find((v) => v.workspaceId === workspaceId)
    const recovered =
      view !== undefined &&
      view.dependencies.some((d) => (d.id === depId || d.id === '') && d.state === 'available')
    if (recovered) {
      record(
        'dependency_recovered',
        view?.workspaceName ?? workspaceId,
        'success',
        `Dependency ${depId} became available again.`,
      )
    }
    // Not present in this poll (workspace removed): clear silently.
  }

  // AVAILABLE → UNAVAILABLE.
  for (const key of nowUnavailable) {
    if (unavailableDeps.has(key)) continue
    const workspaceId = key.slice(0, key.indexOf('|'))
    const depId = key.slice(key.indexOf('|') + 1)
    const view = views.find((v) => v.workspaceId === workspaceId)
    const issue = view?.issues.find(
      (i) => i.code === 'DEPENDENCY_UNAVAILABLE' && (i.dependencyId ?? i.message) === depId,
    )
    record(
      'dependency_unavailable',
      view?.workspaceName ?? workspaceId,
      'stale',
      issue?.message ?? 'Dependency became unavailable.',
    )
  }

  unavailableDeps = nowUnavailable
}

/**
 * Port-conflict transition machine (Phase 10A, spec §11):
 *
 *   none      → conflict : conflict_detected
 *   conflict  → conflict (same identity) : none
 *   conflict  → conflict (different owner) : conflict_resolved + conflict_detected
 *   conflict  → none      : conflict_resolved
 *
 * Identity = workspace + service + port + kind + owner (PID:lifecycle), so an
 * owner change is one resolved-then-detected pair, not silent mutation.
 */
function recordConflictTransitions(views: WorkspaceReadinessView[]): void {
  const nowConflicts = new Set<string>()
  for (const view of views) {
    for (const report of view.conflicts) {
      if (report.kind !== 'no_conflict' && report.kind !== 'already_running') {
        nowConflicts.add(conflictKey(report))
      }
    }
  }

  for (const key of nowConflicts) {
    if (!conflictKeys.has(key)) {
      record('conflict_detected', 'Workspace conflict', 'stale', 'A port conflict is blocking readiness.')
    }
  }
  for (const key of conflictKeys) {
    if (!nowConflicts.has(key)) {
      record('conflict_resolved', 'Workspace conflict', 'success', 'A port conflict cleared.')
    }
  }
  conflictKeys = nowConflicts
}

/**
 * Workspace blocked/recovered transitions — status-edge events only.
 * Untracked workspaces (first sighting) never emit "blocked" retroactively.
 */
function recordWorkspaceStatusTransitions(views: WorkspaceReadinessView[]): void {
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
}

/** Apply all transition machines to one readiness poll. */
function recordTransitions(views: WorkspaceReadinessView[]): void {
  recordDependencyTransitions(views)
  recordConflictTransitions(views)
  recordWorkspaceStatusTransitions(views)
}

/**
 * Port conflict + dependency readiness state. Purely derived from the
 * backend's own evaluation — no ownership or dependency logic lives here.
 * History records *transitions* (deduped by stable issue identities), not
 * polling noise (spec §41–42).
 */
export const useConflictsStore = create<ConflictsState>()((set, get) => {
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
