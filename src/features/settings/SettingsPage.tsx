import { PlaceholderPage } from '@/features/services/ServicesPage'

/**
 * Settings view — Phase 0 placeholder. Note for later phases: settings must
 * stay read-only regarding the OS; LocalStack never mutates system state.
 */
export function SettingsPage() {
  return (
    <PlaceholderPage
      title="Settings"
      description="App preferences, refresh intervals and scan scope (localhost only)."
    />
  )
}
