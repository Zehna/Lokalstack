import { MockDataBadge } from '@/features/dashboard/components/MockDataBadge'
import { ServiceList } from './components/ServiceList'
import { useDashboardStore } from '@/stores/dashboardStore'

/**
 * Services view — Phase 0 placeholder. Reuses the same mock dataset as the
 * dashboard until the discovery engines provide real data.
 */
export function ServicesPage() {
  const services = useDashboardStore((state) => state.services)

  return (
    <PlaceholderPage
      title="Services"
      description="Every service currently detected on localhost, with process and health details."
    >
      <div className="mb-3 flex items-center justify-between">
        <span className="text-sm text-slate-500">{services.length} placeholder services</span>
        <MockDataBadge />
      </div>
      <ServiceList services={services} emptyMessage="No services detected yet." />
    </PlaceholderPage>
  )
}

/** Shared layout for Phase 0 placeholder views. */
export function PlaceholderPage({
  title,
  description,
  children,
}: {
  title: string
  description: string
  children?: React.ReactNode
}) {
  return (
    <div className="mx-auto w-full max-w-5xl px-6 py-8">
      <h1 className="text-xl font-semibold text-slate-100">{title}</h1>
      <p className="mt-1 mb-6 text-sm text-slate-500">{description}</p>
      {children}
      <p className="mt-8 rounded-lg border border-slate-800 bg-slate-900/40 px-4 py-3 text-xs text-slate-500">
        This view arrives in a later phase — the shell reserves the layout today.
      </p>
    </div>
  )
}
