import { create } from 'zustand'

import { getAiRuntimes } from '@/services/native/ports'
import { recordAuditEntry } from '@/stores/auditTrail'
import { createPollingOwner } from '@/stores/pollingOwner'
import type { AiRuntimeSnapshot } from '@/types/domain'

/** How often the AI runtime view refreshes while the page is active. */
const AI_POLL_MS = 10_000

interface AiRuntimeState {
  runtimes: AiRuntimeSnapshot[]
  loading: boolean
  refreshing: boolean
  error: string | null
  lastUpdated: number | null
  /** Per-runtime errors keyed by runtimeId (spec §56). */
  errorsByRuntime: Record<string, string>

  load: () => Promise<void>
  refresh: () => Promise<void>
  /** Manual refresh — bypasses the backend cache. */
  refreshRuntime: (runtimeId: string) => Promise<void>
}

/* -------------------------------------------------------------------------
 * Polling lifecycle — named-subscription, single shared timer (Phase 10A
 * ownership audit). Each consumer acquires by stable id and receives a
 * structurally-paired, idempotent release closure. Duplicate acquisition
 * from one consumer = ONE subscription; release cannot underflow.
 * ---------------------------------------------------------------------- */

const aiRuntimePollOwner = createPollingOwner(
  AI_POLL_MS,
  () => {
    void useAiRuntimeStore.getState().refresh()
  },
  () => {
    void useAiRuntimeStore.getState().refresh()
  },
)

/**
 * Acquire the 10 s AI probe cycle for one logical consumer. Idempotent per
 * consumer. Returns the release closure for the consumer's cleanup.
 */
export function subscribeAiRuntimePolling(consumerId: string): () => void {
  return aiRuntimePollOwner.acquire(consumerId)
}

/** Convenience acquire with an anonymous consumer id. */
export function startAiRuntimePolling(): () => void {
  return subscribeAiRuntimePolling('anonymous')
}

/** Test observability: distinct consumers / timer state. */
export function aiRuntimePollingSubscribers(): number {
  return aiRuntimePollOwner.consumerCount()
}

/** Test observability: whether the shared interval exists. */
export function aiRuntimePollingRunning(): boolean {
  return aiRuntimePollOwner.isRunning()
}

/** Test-only teardown: release every consumer and stop the timer. */
export function resetAiRuntimePolling(): void {
  aiRuntimePollOwner.releaseAll()
}

/** Record a transition event in the shared session audit trail. */
function record(
  action: 'ai_runtime_ready' | 'ai_runtime_unavailable' | 'ai_runtime_loading' | 'ai_model_loaded' | 'ai_model_unloaded',
  subject: string,
  outcome: 'success' | 'failure' | 'stale',
  message: string,
): void {
  recordAuditEntry(action, subject, outcome, message)
}

/**
 * Transition tracking (spec §41): emit one event per change, never per poll.
 * Keyed by runtime for health and by model id for load/unload.
 */
function recordTransitions(
  previous: AiRuntimeSnapshot[],
  current: AiRuntimeSnapshot[],
): void {
  const prevByRuntime = new Map(previous.map((r) => [r.runtimeId, r]))
  for (const runtime of current) {
    const prev = prevByRuntime.get(runtime.runtimeId)
    if (prev === undefined) continue
    if (prev.health !== runtime.health) {
      if (runtime.health === 'ready') {
        record('ai_runtime_ready', runtime.displayName, 'success', `${runtime.displayName} runtime is ready.`)
      } else if (runtime.health === 'unavailable') {
        record('ai_runtime_unavailable', runtime.displayName, 'stale', `${runtime.displayName} is unreachable.`)
      } else if (runtime.health === 'loading') {
        record('ai_runtime_loading', runtime.displayName, 'stale', `${runtime.displayName} is loading a model.`)
      }
    }
    // Model load/unload transitions (loaded-model list membership).
    const prevModels = new Set(prev.loadedModels.map((m) => m.id))
    for (const model of runtime.loadedModels) {
      if (!prevModels.has(model.id)) {
        record('ai_model_loaded', `${runtime.displayName} / ${model.id}`, 'success', `Model ${model.id} loaded into memory.`)
      }
    }
    const currentModels = new Set(runtime.loadedModels.map((m) => m.id))
    for (const model of prev.loadedModels) {
      if (!currentModels.has(model.id)) {
        record('ai_model_unloaded', `${runtime.displayName} / ${model.id}`, 'stale', `Model ${model.id} left memory.`)
      }
    }
  }
}

/**
 * AI runtime state — a dedicated store (spec §56) so the full model
 * inventories never overload portsStore. Polling is slower than discovery
 * (10 s) and fully decoupled from the 3 s port cycle; a hung runtime costs
 * only its own 2 s backend timeout, never the base scan.
 */
export const useAiRuntimeStore = create<AiRuntimeState>()((set, get) => {
  async function runRefresh(bypassCache: boolean): Promise<void> {
    if (get().refreshing) return
    set({ refreshing: true })
    try {
      const previous = get().runtimes
      const runtimes = (await getAiRuntimes(bypassCache)) ?? []
      recordTransitions(previous, runtimes)
      const errorsByRuntime: Record<string, string> = {}
      for (const runtime of runtimes) {
        if (runtime.errorLabel !== undefined) {
          errorsByRuntime[runtime.runtimeId] = runtime.errorLabel
        }
      }
      set({
        runtimes,
        errorsByRuntime,
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
    runtimes: [],
    loading: true,
    refreshing: false,
    error: null,
    lastUpdated: null,
    errorsByRuntime: {},

    load: async () => {
      await runRefresh(false)
    },

    refresh: async () => {
      await runRefresh(false)
    },

    refreshRuntime: async (runtimeId) => {
      // Backend-side per-runtime bypass is not exposed in Phase 8's single
      // command; a manual refresh bypasses the whole cache (spec §26) —
      // acceptable because AI probing is off the discovery path.
      void runtimeId
      await runRefresh(true)
    },
  }
})
