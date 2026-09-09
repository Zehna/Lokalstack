/**
 * AI Services page — read-only runtime observability UX (Phase 10A, spec §34).
 * Also asserts no inference/model-mutation controls exist.
 */
import { render, screen, cleanup } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { mockNativeClient, restoreNativeClient } from '@/test/nativeMock'
import { AiServicesPage } from '@/features/ai-services/AiServicesPage'
import { makeAiRuntime } from '@/test/fixtures'
import { resetAiRuntimePolling, useAiRuntimeStore } from '@/stores/aiRuntimeStore'

describe('AiServicesPage', () => {
  beforeEach(() => {
    mockNativeClient()
  })
  afterEach(() => {
    cleanup()
    resetAiRuntimePolling()
    restoreNativeClient()
    useAiRuntimeStore.setState({
      runtimes: [],
      loading: true,
      refreshing: false,
      error: null,
      lastUpdated: null,
      errorsByRuntime: {},
    })
    vi.restoreAllMocks()
  })

  it('shows the empty state when no AI runtimes exist (normal, not an error)', () => {
    useAiRuntimeStore.setState({ runtimes: [], loading: false })
    render(<AiServicesPage />)
    expect(screen.getByText('No AI runtimes detected.')).toBeInTheDocument()
  })

  it('renders a ready runtime with its version and model count', () => {
    useAiRuntimeStore.setState({
      runtimes: [makeAiRuntime({ version: '0.12.6' })],
      loading: false,
    })
    render(<AiServicesPage />)
    expect(screen.getByText('Ollama')).toBeInTheDocument()
    expect(screen.getByText('ready')).toBeInTheDocument()
    expect(screen.getByText('v0.12.6')).toBeInTheDocument()
    expect(screen.getByText('llama3.1:8b')).toBeInTheDocument()
    expect(screen.getByText('1 installed · 0 loaded')).toBeInTheDocument()
  })

  it('renders an unavailable runtime with its error label', () => {
    useAiRuntimeStore.setState({
      runtimes: [
        makeAiRuntime({ health: 'unavailable', errorLabel: 'Connection refused' }),
      ],
      loading: false,
      errorsByRuntime: { 'ollama-11434': 'Connection refused' },
    })
    render(<AiServicesPage />)
    expect(screen.getByText('unavailable')).toBeInTheDocument()
    expect(screen.getByText('Connection refused')).toBeInTheDocument()
  })

  it('renders a loading-model runtime health distinctly', () => {
    useAiRuntimeStore.setState({
      runtimes: [makeAiRuntime({ health: 'loading' })],
      loading: false,
    })
    render(<AiServicesPage />)
    expect(screen.getByText('loading')).toBeInTheDocument()
  })

  it('shows a loaded model with VRAM when present', () => {
    useAiRuntimeStore.setState({
      runtimes: [
        makeAiRuntime({
          loadedModels: [{ id: 'llama3.1:8b', vramBytes: 4_900_000_000 }],
        }),
      ],
      loading: false,
    })
    render(<AiServicesPage />)
    expect(screen.getByText('Loaded in memory')).toBeInTheDocument()
    expect(screen.getByText(/VRAM/)).toBeInTheDocument()
  })

  it('NO inference or model-management controls exist', () => {
    useAiRuntimeStore.setState({ runtimes: [makeAiRuntime()], loading: false })
    render(<AiServicesPage />)
    const forbidden = [/^generate$/i, /^chat$/i, /^pull/i, /^delete$/i, /^load$/i, /^unload$/i, /^run$/i]
    for (const pattern of forbidden) {
      expect(screen.queryByRole('button', { name: pattern })).not.toBeInTheDocument()
    }
  })
})
