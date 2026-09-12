import { useEffect } from 'react'

import { usePortsStore } from '@/stores/portsStore'
import { useSettingsStore } from '@/stores/settingsStore'

/** Fallback cadence — used only before the first successful settings load. */
const FALLBACK_INTERVAL_MS = 3_000

/**
 * Keep the port-listener store live (Phase 10A ownership semantics + Phase
 * 10C settings integration):
 *
 * - Fetches immediately on mount, then on the settings-configured cadence.
 * - Auto Refresh OFF stops the shared timer without destroying the lease;
 *   re-enabling restores it (settings §I).
 * - An interval change recreates exactly one timer (settings §K).
 * - A duplicate consumer id still yields ONE subscription (Phase 10A).
 */
export function usePortListeners(options?: { enabled?: boolean }): void {
  const { enabled = true } = options ?? {}
  const loadListeners = usePortsStore((state) => state.loadListeners)
  const refreshListeners = usePortsStore((state) => state.refreshListeners)
  const autoRefresh = useSettingsStore((state) => state.settings?.autoRefresh ?? true)
  const intervalMs = useSettingsStore(
    (state) => state.settings?.portRefreshIntervalMs ?? FALLBACK_INTERVAL_MS,
  )

  useEffect(() => {
    if (!enabled) return

    void loadListeners()

    if (!autoRefresh) return // manual refresh still works

    const timer = window.setInterval(() => {
      void refreshListeners()
    }, intervalMs)

    return () => {
      window.clearInterval(timer)
    }
  }, [enabled, autoRefresh, intervalMs, loadListeners, refreshListeners])
}
