import { useEffect } from 'react'

import { AppLayout } from './AppLayout'
import { useSettingsStore } from '@/stores/settingsStore'

/** Module-level guard: settings load exactly once per app session, even
 * under StrictMode's double mount (settings §Z/§AA). */
let settingsBootstrapped = false

/**
 * Root component of the desktop shell.
 *
 * Loads persisted settings once at startup so polling configuration
 * (auto refresh, intervals, AI/Docker toggles) applies from the first
 * mount — the Settings page does not need to be visited (spec §I).
 */
export function App() {
  useEffect(() => {
    if (settingsBootstrapped) return
    settingsBootstrapped = true
    void useSettingsStore.getState().load()
  }, [])

  return <AppLayout />
}
