/**
 * MOCK UI DATA — retained from Phase 0 for surfaces that are still mocks.
 *
 * Nothing here was discovered on the machine. These constants exist only so
 * mock-only surfaces (the mock Active Services list on the dashboard, the
 * Services page placeholder) can be laid out and reviewed. Components must
 * render this data with a visible "mock data" label. The dashboard's REAL
 * listener section does not use this file.
 */

import type { PortConflict, Service, SystemUsage } from '@/types/domain'

export const MOCK_SERVICES: Service[] = [
  {
    id: 'mock-nextjs',
    name: 'Next.js',
    type: 'frontend',
    port: 3000,
    pid: null,
    processName: null,
    status: 'running',
  },
  {
    id: 'mock-flask',
    name: 'Flask',
    type: 'backend',
    port: 5000,
    pid: null,
    processName: null,
    status: 'running',
  },
  {
    id: 'mock-postgres',
    name: 'PostgreSQL',
    type: 'database',
    port: 5432,
    pid: null,
    processName: null,
    status: 'running',
  },
  {
    id: 'mock-llamacpp',
    name: 'llama.cpp',
    type: 'ai',
    port: 8080,
    pid: null,
    processName: null,
    status: 'running',
  },
]

export const MOCK_CONFLICTS: PortConflict[] = []

/** Initial mock usage snapshot. It drifts via `useMockLiveUsage`. */
export const INITIAL_MOCK_USAGE: SystemUsage = {
  cpuPercent: 23,
  memoryPercent: 41,
}
