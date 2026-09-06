import type { Service } from '@/types/domain'
import { formatPort } from '@/utils/format'

/** Status dot color for a service lifecycle state. */
function statusDotClass(status: Service['status']): string {
  switch (status) {
    case 'running':
      return 'bg-emerald-400'
    case 'unhealthy':
      return 'bg-amber-400'
    case 'stopped':
      return 'bg-slate-600'
    default:
      return 'bg-slate-500'
  }
}

interface ServiceListProps {
  services: Service[]
  emptyMessage?: string
}

/**
 * Reusable presentation list of services. Knows nothing about where the data
 * comes from — the caller is responsible for labeling mock data.
 */
export function ServiceList({ services, emptyMessage = 'No services.' }: ServiceListProps) {
  if (services.length === 0) {
    return (
      <p className="rounded-lg border border-dashed border-slate-800 p-6 text-center text-sm text-slate-500">
        {emptyMessage}
      </p>
    )
  }

  return (
    <ul className="overflow-hidden rounded-lg border border-slate-800">
      {services.map((service, index) => (
        <li
          key={service.id}
          className={`flex items-center gap-3 bg-slate-900/60 px-4 py-3 ${
            index > 0 ? 'border-t border-slate-800' : ''
          }`}
        >
          <span className={`h-2 w-2 shrink-0 rounded-full ${statusDotClass(service.status)}`} />

          <span className="flex-1 truncate text-sm font-medium text-slate-200">
            {service.name ?? 'Unnamed (identity arrives in Phase 3)'}
          </span>

          <span className="w-20 text-xs capitalize text-slate-500">{service.type}</span>

          <span className="w-12 text-right font-mono text-xs text-slate-400">
            {formatPort(service.port)}
          </span>

          <span className="w-16 text-right font-mono text-xs text-slate-600">
            {service.pid === null ? '—' : String(service.pid)}
          </span>
        </li>
      ))}
    </ul>
  )
}
