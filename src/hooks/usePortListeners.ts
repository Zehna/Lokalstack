import { useEffect } from 'react'

import { usePortsStore } from '@/stores/portsStore'

/** Interval between automatic listener refreshes. */
const REFRESH_INTERVAL_MS = 3_000

/**
 * Keep the port-listener store live.
 *
 * - Fetches immediately on mount, then every `refreshIntervalMs` (≈3 s).
 * - Timers are cleaned up on unmount; the store drops overlapping requests,
 *   so a slow response never stacks a second one.
 * - `enabled` lets pages opt out when they don't need live data.
 */
export function usePortListeners(options?: { enabled?: boolean; refreshIntervalMs?: number }): void {
  const { enabled = true, refreshIntervalMs = REFRESH_INTERVAL_MS } = options ?? {}
  const loadListeners = usePortsStore((state) => state.loadListeners)
  const refreshListeners = usePortsStore((state) => state.refreshListeners)

  useEffect(() => {
    if (!enabled) return

    void loadListeners()

    const timer = window.setInterval(() => {
      void refreshListeners()
    }, refreshIntervalMs)

    return () => {
      window.clearInterval(timer)
    }
  }, [enabled, refreshIntervalMs, loadListeners, refreshListeners])
}
