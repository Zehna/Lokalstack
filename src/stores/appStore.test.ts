import { describe, expect, it } from 'vitest'

import { useAppStore } from '@/stores/appStore'

describe('appStore', () => {
  it('starts on the dashboard view', () => {
    expect(useAppStore.getState().activeView).toBe('dashboard')
  })

  it('switches the active view', () => {
    useAppStore.getState().setActiveView('docker')
    expect(useAppStore.getState().activeView).toBe('docker')
    useAppStore.getState().setActiveView('dashboard')
  })
})
