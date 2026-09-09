/**
 * Polling ownership core (Phase 10A hardening audit).
 *
 * A `PollingOwner` manages exactly one interval for a domain and hands out
 * **named, idempotent subscriptions**:
 *
 * - `acquire(id)` is idempotent per `id` — a consumer that accidentally
 *   acquires twice still holds ONE logical subscription.
 * - `acquire(id)` returns a release closure structurally paired with that
 *   acquisition; every cleanup path returns the closure itself.
 * - Release is backed by `Set.delete`, so it cannot underflow — releasing an
 *   absent id (double cleanup, stray closure) is a no-op, never a negative
 *   count.
 * - The timer exists while ≥ 1 distinct consumer holds a subscription and is
 *   cleared the instant the set becomes empty.
 *
 * StrictMode mount → unmount → remount is safe: the release drops the last
 * consumer (timer stops), the remount re-acquires (timer restarts). No leak.
 */

/** Idempotent cleanup closure returned by `acquire`. */
export type PollingRelease = () => void

export interface PollingOwner {
  /**
   * Acquire a named subscription and (if this is the first consumer) start
   * the shared timer. Duplicate ids from the same logical consumer are
   * idempotent. Returns the structurally-paired release closure.
   */
  acquire: (consumerId: string) => PollingRelease
  /** Number of *distinct* active consumers. */
  consumerCount: () => number
  /** Whether one specific consumer currently holds a subscription. */
  hasConsumer: (consumerId: string) => boolean
  /** Whether the shared interval currently exists. */
  isRunning: () => boolean
  /** Release every consumer and stop the timer (test teardown only). */
  releaseAll: () => void
}

/** Create a polling owner with one interval and named consumer leases. */
export function createPollingOwner(
  intervalMs: number,
  onTick: () => void,
  /** Optional one-shot when the timer is created (e.g. immediate refresh). */
  onStart?: () => void,
): PollingOwner {
  let timer: ReturnType<typeof setInterval> | null = null
  const consumers = new Set<string>()

  function startIfNeeded(): void {
    if (timer !== null) return
    onStart?.()
    timer = setInterval(onTick, intervalMs)
  }

  function stopIfNeeded(): void {
    if (consumers.size === 0 && timer !== null) {
      clearInterval(timer)
      timer = null
    }
  }

  return {
    acquire(consumerId: string): PollingRelease {
      // Set semantics: the same id twice = one subscription, never two.
      consumers.add(consumerId)
      startIfNeeded()
      return () => {
        // Delete of an absent id is a no-op — double release cannot
        // underflow the consumer set or stop a timer other consumers need.
        consumers.delete(consumerId)
        stopIfNeeded()
      }
    },
    consumerCount: () => consumers.size,
    hasConsumer: (consumerId: string) => consumers.has(consumerId),
    isRunning: () => timer !== null,
    releaseAll(): void {
      consumers.clear()
      stopIfNeeded()
    },
  }
}
