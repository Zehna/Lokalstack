import type { ViewId } from '@/types/domain'

/** Which feature a view belongs to — used for file placement, not logic. */
export type FeatureGroup = 'dashboard' | 'projects' | 'services' | 'ports' | 'workspaces' | 'ai-services' | 'docker' | 'history' | 'settings'

export interface NavItem {
  id: ViewId
  label: string
  feature: FeatureGroup
}

/**
 * Single source of truth for the left navigation and the active view.
 * Phase 0 routes are placeholders — only the Dashboard renders content.
 */
export const NAV_ITEMS: NavItem[] = [
  { id: 'dashboard', label: 'Dashboard', feature: 'dashboard' },
  { id: 'projects', label: 'Projects', feature: 'projects' },
  { id: 'services', label: 'Services', feature: 'services' },
  { id: 'ports', label: 'Ports', feature: 'ports' },
  { id: 'workspaces', label: 'Workspaces', feature: 'workspaces' },
  { id: 'ai-services', label: 'AI Services', feature: 'ai-services' },
  { id: 'docker', label: 'Docker', feature: 'docker' },
  { id: 'history', label: 'History', feature: 'history' },
  { id: 'settings', label: 'Settings', feature: 'settings' },
]
