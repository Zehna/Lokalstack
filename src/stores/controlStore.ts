import { create } from 'zustand'

import { endProcess, openServiceUrl } from '@/services/native/ports'
import type { ControlTarget, StopResult } from '@/types/domain'

/** One completed or failed control action, newest last. */
export interface ControlHistoryEntry {
  /** Monotonic id for stable React keys. */
  id: number
  /** Unix epoch ms when the action was attempted. */
  at: number
  /** `end_process` | `open` — the action performed. */
  action:
    | 'end_process'
    | 'open'
    // Phase 6 managed-lifecycle events share the same audit trail.
    | 'workspace_start'
    | 'workspace_stop'
    | 'service_start'
    | 'service_stop'
    | 'service_force_stop'
    | 'service_restart'
    | 'workspace_create'
    | 'startup_failure'
    | 'port_conflict'
    // Phase 7 conflict/dependency intelligence (transition events).
    | 'conflict_detected'
    | 'conflict_resolved'
    | 'dependency_unavailable'
    | 'dependency_recovered'
    | 'workspace_blocked'
    | 'workspace_recovered'
    | 'free_port_suggested'
    // Phase 8 AI runtime transitions (deduped).
    | 'ai_runtime_ready'
    | 'ai_runtime_unavailable'
    | 'ai_runtime_loading'
    | 'ai_model_loaded'
    | 'ai_model_unloaded'
  /** What the action targeted (human-facing display name). */
  subject: string
  /** PID when the action had one (advisory, from the snapshot). */
  pid: number | null
  /** `success` | `failure` | `stale` — stale means the target was refused. */
  outcome: 'success' | 'failure' | 'stale'
  /** Human-readable summary of what happened. */
  message: string
}

/** In-flight action kind, for per-PID spinners and disabled buttons. */
type PendingAction =
  | { kind: 'end_process'; pid: number | null }
  | { kind: 'open'; pid: number | null }
  | null

interface ControlState {
  /** The action currently running, if any. One at a time — control actions
   * are rare, deliberate events, not background work. */
  pending: PendingAction
  /** Completed actions this session, oldest first. */
  history: ControlHistoryEntry[]
  /** End the process behind the opaque target id (explicit user
   * confirmation required — the caller owns the confirm UX). The backend
   * revalidates identity and recomputes eligibility before acting. */
  endProcess: (target: ControlTarget, pid: number) => Promise<StopResult>
  /** Open a localhost URL from the snapshot in the default browser. */
  open: (url: string, pid: number | null, subject: string) => Promise<void>
  /** Clear the history (user action). */
  clearHistory: () => void
}

let nextHistoryId = 1

function classifyError(cause: unknown): {
  outcome: ControlHistoryEntry['outcome']
  message: string
} {
  const message = cause instanceof Error ? cause.message : String(cause)
  if (
    message.startsWith('STALE_TARGET') ||
    message.startsWith('UNKNOWN_TARGET') ||
    message.startsWith('REFUSED_BY_POLICY') ||
    message.startsWith('IDENTITY_UNVERIFIABLE') ||
    message.includes('changed since it was discovered') ||
    message.includes('no longer valid')
  ) {
    return {
      outcome: 'stale',
      message:
        'This control target is no longer valid. Refresh and try again — nothing was terminated.',
    }
  }
  return { outcome: 'failure', message }
}

/**
 * Control-action state: End Process / open, with a session history.
 *
 * The frontend holds no authority: it sends only the opaque target id the
 * backend issued. Every refusal (unknown id, stale identity, policy
 * denial) is recorded honestly in the history.
 */
export const useControlStore = create<ControlState>()((set, get) => {
  function record(entry: Omit<ControlHistoryEntry, 'id' | 'at'>): void {
    set((state) => ({
      history: [
        ...state.history,
        { ...entry, id: nextHistoryId++, at: Date.now() },
      ].slice(-100),
    }))
  }

  return {
    pending: null,
    history: [],

    endProcess: async (target, pid) => {
      if (get().pending !== null) {
        throw new Error('Another control action is already running.')
      }
      set({ pending: { kind: 'end_process', pid } })
      try {
        const result = await endProcess(target.id)
        record({
          action: 'end_process',
          subject: target.displayName,
          pid,
          outcome: result.stopped ? 'success' : 'failure',
          message: result.message,
        })
        return result
      } catch (cause) {
        const { outcome, message } = classifyError(cause)
        record({ action: 'end_process', subject: target.displayName, pid, outcome, message })
        throw cause
      } finally {
        set({ pending: null })
      }
    },

    open: async (url, pid, subject) => {
      if (get().pending !== null) {
        throw new Error('Another control action is already running.')
      }
      set({ pending: { kind: 'open', pid } })
      try {
        await openServiceUrl(url, pid)
        record({ action: 'open', subject, pid, outcome: 'success', message: `Opened ${url}` })
      } catch (cause) {
        const { outcome, message } = classifyError(cause)
        record({ action: 'open', subject, pid, outcome, message })
        throw cause
      } finally {
        set({ pending: null })
      }
    },

    clearHistory: () => set({ history: [] }),
  }
})
