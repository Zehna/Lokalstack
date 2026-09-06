import { create } from 'zustand'

import type { ViewId } from '@/types/domain'

interface AppState {
  /** Which navigation view is currently active in the desktop shell. */
  activeView: ViewId
  setActiveView: (view: ViewId) => void
}

/** Global UI state: active view selection of the shell. */
export const useAppStore = create<AppState>()((set) => ({
  activeView: 'dashboard',
  setActiveView: (view) => set({ activeView: view }),
}))
