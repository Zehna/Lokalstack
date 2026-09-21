import { ErrorBoundary } from './components/ErrorBoundary'
import { Sidebar } from './components/Sidebar'
import { NAV_ITEMS } from './navigation'
import { AiServicesPage } from '@/features/ai-services/AiServicesPage'
import { DashboardPage } from '@/features/dashboard/DashboardPage'
import { DockerPage } from '@/features/docker/DockerPage'
import { HistoryPage } from '@/features/history/HistoryPage'
import { PortsPage } from '@/features/ports/PortsPage'
import { ProjectsPage } from '@/features/projects/ProjectsPage'
import { ServicesPage } from '@/features/services/ServicesPage'
import { SettingsPage } from '@/features/settings/SettingsPage'
import { WorkspacesPage } from '@/features/workspaces/WorkspacesPage'
import { useAppStore } from '@/stores/appStore'
import type { ViewId } from '@/types/domain'

/** View registry — one component per navigation entry. */
const VIEWS: Record<ViewId, () => React.ReactElement> = {
  // Placeholder until Task 17 lands the real DiagnosticsPage.
  diagnostics: () => <></>,
  dashboard: DashboardPage,
  projects: ProjectsPage,
  services: ServicesPage,
  ports: PortsPage,
  workspaces: WorkspacesPage,
  'ai-services': AiServicesPage,
  docker: DockerPage,
  history: HistoryPage,
  settings: SettingsPage,
}

/**
 * Desktop shell: left navigation plus the active view. Phase 11C (§43-I):
 * navigation state lives ONLY in `useAppStore` (single source of truth) —
 * Sidebar, the ErrorBoundary dashboard reset, and store-driven navigation
 * all share it. Navigation never waits for backend IPC (see appStore).
 */
export function AppLayout() {
  const activeView = useAppStore((state) => state.activeView)
  const setActiveView = useAppStore((state) => state.setActiveView)
  const ActiveView = VIEWS[activeView]
  const activeLabel = NAV_ITEMS.find((item) => item.id === activeView)?.label ?? activeView

  return (
    <div className="flex h-full">
      <Sidebar activeView={activeView} onSelectView={setActiveView} />
      <main className="min-w-0 flex-1 overflow-y-auto" aria-label={activeLabel}>
        {/* Phase 10B (spec §K): a render exception in one view shows a
            recoverable surface; the shell (sidebar + navigation) stays up. */}
        <ErrorBoundary resetKey={activeView} onNavigateDashboard={() => setActiveView('dashboard')}>
          <ActiveView />
        </ErrorBoundary>
      </main>
    </div>
  )
}
