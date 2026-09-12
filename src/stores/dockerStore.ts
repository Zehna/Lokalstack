import { create } from 'zustand'

import { getDockerSnapshot, refreshDocker } from '@/services/native/ports'
import { recordAuditEntry } from '@/stores/auditTrail'
import { createPollingOwner } from '@/stores/pollingOwner'
import { registerPollingApplier } from '@/stores/settingsStore'
import type { ContainerProjectLink, DockerContainer, DockerEngineSnapshot } from '@/types/domain'

/** Engine/list polling cadence (spec §65) — backend caches at 5 s too. */
const DOCKER_POLL_MS = 5_000

interface DockerState {
  /** Last snapshot; `available === false` is an honest state, not an error. */
  snapshot: DockerEngineSnapshot | null
  loading: boolean
  refreshing: boolean
  error: string | null
  lastUpdated: number | null

  load: () => Promise<void>
  refresh: () => Promise<void>
}

/* -------------------------------------------------------------------------
 * Polling lifecycle — named-subscription, single shared timer (Phase 10A
 * ownership architecture, migrated in the Phase 10C correction so settings
 * can stop AND resume polling live). The Docker page is the single
 * consumer ('docker-page'); duplicate start is idempotent and stop cannot
 * underflow — identical semantics to the previous raw interval, plus live
 * settings reconfiguration (spec §I): disable stops the timer, re-enable
 * recreates exactly one timer for the still-mounted consumer, no remount.
 * ---------------------------------------------------------------------- */

const dockerPollOwner = createPollingOwner(
  DOCKER_POLL_MS,
  () => {
    void useDockerStore.getState().load()
  },
  () => {
    void useDockerStore.getState().load()
  },
)

/** Start Docker polling for the mounted page (idempotent). */
export function startDockerPolling(): void {
  dockerPollOwner.acquire('docker-page')
}

/** Stop the mounted page's polling (idempotent, cannot underflow). */
export function stopDockerPolling(): void {
  dockerPollOwner.release('docker-page')
}

/** Test observability: whether the shared interval exists. */
export function dockerPollingRunning(): boolean {
  return dockerPollOwner.isRunning()
}

/** Test-only teardown: release every consumer and restore default config. */
export function resetDockerPolling(): void {
  dockerPollOwner.releaseAll()
  // releaseAll clears leases but not settings state; restore the pristine
  // default (enabled) so the next test/module consumer starts unconfigured.
  dockerPollOwner.configure({ enabled: true })
}

// Phase 10C (spec §I): Docker auto-polling follows the settings toggle —
// BOTH directions. Disable stops the timer; re-enable resumes immediately
// for the still-mounted consumer (no navigation/remount required). Manual
// refresh always works — it calls the store action directly.
registerPollingApplier((settings) => {
  dockerPollOwner.configure({ enabled: settings.dockerPollingEnabled })
})

/** Record a transition event in the shared session audit trail. */
function record(
  action: 'docker_engine_available' | 'docker_engine_unavailable' | 'container_started_observed' | 'container_stopped_observed' | 'container_health_unhealthy' | 'container_health_recovered',
  subject: string,
  outcome: 'success' | 'failure' | 'stale',
  message: string,
): void {
  recordAuditEntry(action, subject, outcome, message)
}

/**
 * Transition tracking (spec §43): one event per change, never per poll.
 * All wording makes clear these are *observations* — LocalStack did not
 * initiate container start/stop.
 */
function recordTransitions(
  previous: DockerContainer[],
  current: DockerContainer[],
): void {
  const prevState = new Map(previous.map((c) => [c.id, c]))
  for (const container of current) {
    const before = prevState.get(container.id)
    if (!before) {
      if (container.state === 'running') {
        record(
          'container_started_observed',
          container.name,
          'success',
          `Observed container ${container.name} running (not initiated by LocalStack)`,
        )
      }
    } else {
      if (before.state !== 'running' && container.state === 'running') {
        record(
          'container_started_observed',
          container.name,
          'success',
          `Observed container ${container.name} started (not initiated by LocalStack)`,
        )
      }
      if (before.state === 'running' && container.state !== 'running') {
        record(
          'container_stopped_observed',
          container.name,
          'stale',
          `Observed container ${container.name} ${container.state} (not initiated by LocalStack)`,
        )
      }
      if (before.health !== 'unhealthy' && container.health === 'unhealthy') {
        record(
          'container_health_unhealthy',
          container.name,
          'failure',
          `Container ${container.name} health is unhealthy`,
        )
      }
      if (before.health === 'unhealthy' && container.health === 'healthy') {
        record(
          'container_health_recovered',
          container.name,
          'success',
          `Container ${container.name} health recovered`,
        )
      }
    }
  }
}

/** Human-facing label for a typed Docker failure (no pipe internals). */
function failureLabel(snapshot: DockerEngineSnapshot): string {
  const failure = snapshot.error
  if (!failure) return 'Docker Engine is not currently available.'
  switch (failure.kind) {
    case 'docker_unavailable':
      return 'Docker Engine is not currently available.'
    case 'access_denied':
      return 'Docker Engine detected but access was denied.'
    case 'timeout':
      return 'Docker Engine did not respond in time.'
    case 'api_unsupported':
      return `Docker API not supported by this engine: ${failure.detail}`
    case 'malformed_response':
      return 'Docker Engine returned an unexpected payload.'
    case 'response_too_large':
      return 'Docker Engine response exceeded the size limit.'
    case 'engine_error':
      return 'Docker Engine error.'
  }
}

export const useDockerStore = create<DockerState>((set, get) => ({
  snapshot: null,
  loading: false,
  refreshing: false,
  error: null,
  lastUpdated: null,

  load: async () => {
    const existing = get().snapshot
    if (existing) set({ loading: false })
    else set({ loading: true })
    try {
      const snapshot = await getDockerSnapshot()
      if (snapshot === undefined || snapshot === null) {
        set({ loading: false, error: 'Docker engine returned no data' })
        return
      }
      if (existing?.available) recordTransitions(existing.containers, snapshot.containers)
      set({
        snapshot,
        loading: false,
        error: snapshot.available ? null : failureLabel(snapshot),
        lastUpdated: Date.now(),
      })
    } catch (error) {
      set({ loading: false, error: error instanceof Error ? error.message : String(error) })
    }
  },

  refresh: async () => {
    set({ refreshing: true })
    try {
      const existing = get().snapshot
      const snapshot = await refreshDocker()
      if (snapshot === undefined || snapshot === null) {
        set({ refreshing: false, error: 'Docker engine returned no data' })
        return
      }
      if (existing?.available) recordTransitions(existing.containers, snapshot.containers)
      set({
        snapshot,
        refreshing: false,
        error: snapshot.available ? null : failureLabel(snapshot),
        lastUpdated: Date.now(),
      })
    } catch (error) {
      set({ refreshing: false, error: error instanceof Error ? error.message : String(error) })
    }
  },
}))



/** Convenience selector: project link per container id. */
export function linksByContainer(
  links: ContainerProjectLink[],
): Map<string, ContainerProjectLink> {
  return new Map(links.map((l) => [l.containerId, l]))
}
