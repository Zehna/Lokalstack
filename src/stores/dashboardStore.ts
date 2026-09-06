import { create } from 'zustand'

import { MOCK_SERVICES } from '@/features/dashboard/mockData'
import type { Service } from '@/types/domain'

interface DashboardState {
  /**
   * MOCK UI DATA — used only by still-mock dashboard surfaces (the Services
   * preview). Real listener data lives in `portsStore` and never passes
   * through here.
   */
  services: Service[]
}

/** State for mock-only dashboard surfaces. Real data lives in `portsStore`. */
export const useDashboardStore = create<DashboardState>()(() => ({
  services: MOCK_SERVICES,
}))
