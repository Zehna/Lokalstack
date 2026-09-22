import type { LucideIcon } from 'lucide-react'

import {
  Activity,
  Boxes,
  Bot,
  Clock,
  LayoutDashboard,
  PlugZap,
  Settings as SettingsIcon,
  Container,
  FolderKanban,
  Radar,
} from 'lucide-react'

import type { ViewId } from '@/types/domain'

import { NAV_ITEMS } from '../navigation'

interface SidebarProps {
  activeView: ViewId
  onSelectView: (view: ViewId) => void
}

/** Map of nav item ids to their Lucide icons. */
const NAV_ICONS: Record<ViewId, LucideIcon> = {
  dashboard: LayoutDashboard,
  projects: FolderKanban,
  services: PlugZap,
  ports: Radar,
  workspaces: Boxes,
  'ai-services': Bot,
  docker: Container,
  history: Clock,
  settings: SettingsIcon,
  diagnostics: Activity,
}

/**
 * Left navigation of the desktop shell. Pure presentation: it renders
 * `NAV_ITEMS` and reports selections upward.
 */
export function Sidebar({ activeView, onSelectView }: SidebarProps) {
  return (
    <nav
      aria-label="Primary"
      className="flex h-full w-56 shrink-0 flex-col border-r border-slate-800 bg-slate-950"
    >
      {/* Brand */}
      <div className="flex items-center gap-2.5 px-4 pt-5 pb-6">
        <div className="flex h-8 w-8 items-center justify-center rounded-lg bg-gradient-to-b from-sky-500 to-teal-500">
          <Radar className="h-4.5 w-4.5 text-slate-950" strokeWidth={2.5} />
        </div>
        <div className="leading-tight">
          <p className="text-sm font-semibold text-slate-100">LocalStack</p>
          <p className="text-[11px] text-slate-500">Control Center</p>
        </div>
      </div>

      {/* Nav items */}
      <ul className="flex-1 space-y-0.5 px-2">
        {NAV_ITEMS.map((item) => {
          const Icon = NAV_ICONS[item.id]
          const isActive = item.id === activeView
          return (
            <li key={item.id}>
              <button
                type="button"
                onClick={() => onSelectView(item.id)}
                aria-current={isActive ? 'page' : undefined}
                className={`flex w-full items-center gap-2.5 rounded-md px-3 py-2 text-sm transition-colors ${
                  isActive
                    ? 'bg-slate-800 font-medium text-slate-100'
                    : 'text-slate-400 hover:bg-slate-900 hover:text-slate-200'
                }`}
              >
                <Icon className="h-4 w-4 shrink-0" strokeWidth={isActive ? 2.2 : 1.8} />
                {item.label}
              </button>
            </li>
          )
        })}
      </ul>

      {/* Status note */}
      <div className="border-t border-slate-800 px-4 py-3">
        <p className="text-[11px] leading-relaxed text-slate-600">
          Phase 1 — live TCP listener discovery. Process &amp; service intelligence lands in
          Phase&nbsp;2+.
        </p>
      </div>
    </nav>
  )
}
