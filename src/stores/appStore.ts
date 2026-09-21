import { create } from 'zustand'

import { updateDiagnosticsContext } from '@/services/native/diagnostics'
import type { ViewId } from '@/types/domain'

interface AppState {
  /** Which navigation view is currently active in the desktop shell. */
  activeView: ViewId
  setActiveView: (view: ViewId) => void
}

/**
 * Global UI state: active view selection of the shell. Phase 11C (§43-I):
 * this store is the SINGLE canonical navigation state — AppLayout, Sidebar,
 * ErrorBoundary, and Dashboard all consume it. The store update is
 * synchronous; the diagnostics context report is a fire-and-forget side
 * effect that can never block or undo navigation (failures are swallowed).
 */
export const useAppStore = create<AppState>()((set) => ({
  activeView: 'dashboard',
  setActiveView: (view) => {
    // 1. Synchronous visible-state change — navigation never awaits IPC.
    set({ activeView: view })
    // 2. Best-effort diagnostics context (panic-hook cache). Fire-and-forget.
    void updateDiagnosticsContext(view).catch(() => {})
  },
}))
