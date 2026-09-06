import { useState } from 'react'

import { Sidebar } from './components/Sidebar'
import { NAV_ITEMS } from './navigation'
import { AiServicesPage } from '@/features/ai-services/AiServicesPage'
import { DashboardPage } from '@/features/dashboard/DashboardPage'
import { HistoryPage } from '@/features/history/HistoryPage'
import { PortsPage } from '@/features/ports/PortsPage'
import { ProjectsPage } from '@/features/projects/ProjectsPage'
import { ServicesPage } from '@/features/services/ServicesPage'
import { SettingsPage } from '@/features/settings/SettingsPage'
import { WorkspacesPage } from '@/features/workspaces/WorkspacesPage'
import type { ViewId } from '@/types/domain'

/** Placeholder view registry — one component per navigation entry. */
const VIEWS: Record<ViewId, () => React.ReactElement> = {
  dashboard: DashboardPage,
  projects: ProjectsPage,
  services: ServicesPage,
  ports: PortsPage,
  workspaces: WorkspacesPage,
  'ai-services': AiServicesPage,
  history: HistoryPage,
  settings: SettingsPage,
}

/**
 * Desktop shell: left navigation plus the active placeholder view.
 * Phase 0 keeps the state local; it moves into Zustand when views need to
 * share data across features.
 */
export function AppLayout() {
  const [activeView, setActiveView] = useState<ViewId>('dashboard')
  const ActiveView = VIEWS[activeView]
  const activeLabel = NAV_ITEMS.find((item) => item.id === activeView)?.label ?? activeView

  return (
    <div className="flex h-full">
      <Sidebar activeView={activeView} onSelectView={setActiveView} />
      <main className="min-w-0 flex-1 overflow-y-auto" aria-label={activeLabel}>
        <ActiveView />
      </main>
    </div>
  )
}
