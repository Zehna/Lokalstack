/**
 * Phase 10D (spec §F, §G, §AL) — large-fixture render tests.
 *
 * Renders each major page with realistic worst-case datasets (no
 * virtualization needed unless measured pathologies appear) and asserts the
 * full dataset is present and render completes inside a generous, stable
 * budget. Budgets are broad regression ceilings (seconds), not benchmarks —
 * they fail only on pathological costs (e.g. accidental O(n²) per-row work).
 * Ceilings must absorb scheduler noise from parallel vitest workers and v8
 * coverage instrumentation (Phase 10F: healthy renders measure 0.7–3 s there,
 * so a 5 s ceiling flapped against vitest's own 5 s default testTimeout).
 *
 * Fixture sizes follow spec §F: 300 ports, 150 service groups, 100 projects,
 * 50 workspaces, 500 AI models, 100 Docker containers, 100 history events.
 */
import { render, screen, cleanup } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { mockNativeClient, restoreNativeClient } from '@/test/nativeMock'
import {
  makeContainer,
  makeDockerSnapshot,
  makeAiRuntime,
  makeSnapshot,
  makeWorkspace,
} from '@/test/fixtures'
import { PortsPage } from '@/features/ports/PortsPage'
import { ProjectsPage } from '@/features/projects/ProjectsPage'
import { WorkspacesPage } from '@/features/workspaces/WorkspacesPage'
import { AiServicesPage } from '@/features/ai-services/AiServicesPage'
import { DockerPage } from '@/features/docker/DockerPage'
import { HistoryPage } from '@/features/history/HistoryPage'
import { groupListenersByProcess } from '@/features/services/groupProcesses'
import { usePortsStore } from '@/stores/portsStore'
import { useDockerStore, stopDockerPolling } from '@/stores/dockerStore'
import { useAiRuntimeStore } from '@/stores/aiRuntimeStore'
import { useWorkspaceStore } from '@/stores/workspaceStore'
import { useControlStore } from '@/stores/controlStore'
import type { PortListener, ProcessInfo, ProjectIdentity } from '@/types/domain'

/** Broad regression ceiling for one full page render (ms). */
const RENDER_CEILING_MS = 15_000
/** Broad ceiling for a pure derivation over 300 rows (ms). */
const DERIVE_CEILING_MS = 5_000
/** Harness timeout well above the ceilings so the ceiling assertion, not
 *  vitest's default 5 s testTimeout, decides the outcome under load. */
const RENDER_TEST_TIMEOUT_MS = 30_000

function timedRender(ui: React.ReactElement): number {
  const started = performance.now()
  render(ui)
  return performance.now() - started
}

function expectUnderBudget(ms: number, ceiling: number, label: string): void {
  expect(
    ms < ceiling,
    `${label} rendered in ${ms.toFixed(0)} ms — exceeds the ${ceiling} ms regression ceiling`,
  ).toBe(true)
}

function syntheticListener(port: number, pid: number): PortListener {
  return {
    protocol: 'tcp',
    ipVersion: 4,
    localAddress: '127.0.0.1',
    port,
    pid,
    state: 'LISTEN',
  }
}

function syntheticProcess(pid: number): ProcessInfo {
  return {
    pid,
    name: `proc-${pid}.exe`,
    executablePath: `C:\\dev\\proj-${pid}\\node.exe`,
    startedAt: 1_700_000_000_000,
    memoryBytes: 64_000_000,
    commandLine: `node vite --port ${pid}`,
    cpuPercent: 2.5,
    accessible: true,
  }
}

function syntheticProject(i: number): ProjectIdentity {
  return {
    id: `D:\\Projects\\proj-${i}`,
    name: `proj-${i}`,
    rootPath: `D:\\Projects\\proj-${i}`,
    kind: 'node_js',
    git: { isRepository: true, rootPath: `D:\\Projects\\proj-${i}`, branch: 'main' },
    packageManager: 'npm',
    startCommand: { command: 'npm run dev', confidence: 'high', evidence: [] },
    confidence: 'exact',
    evidence: [],
  }
}

describe('Phase 10D large-fixture renders (spec §F)', () => {
  beforeEach(() => {
    mockNativeClient()
  })
  afterEach(() => {
    cleanup()
    stopDockerPolling()
    restoreNativeClient()
    usePortsStore.setState({
      listeners: [], processByPid: new Map(), serviceByPid: new Map(),
      projects: [], projectByPid: new Map(), controlByPid: new Map(),
      durationMs: null, loading: false, refreshing: false, error: null, lastUpdated: null,
    })
    useDockerStore.setState({ snapshot: null, loading: false, refreshing: false, error: null, lastUpdated: null })
    useAiRuntimeStore.setState({ runtimes: [], loading: false, refreshing: false, error: null, lastUpdated: null })
    useWorkspaceStore.setState({ workspaces: [], loading: false, error: null } as Partial<Parameters<typeof useWorkspaceStore.setState>[0]>)
    useControlStore.setState({ history: [] })
    vi.restoreAllMocks()
  })

  it('renders 300 ports with process intelligence inside the ceiling', async () => {
    const listeners: PortListener[] = []
    const processes: ProcessInfo[] = []
    const projects: ProjectIdentity[] = []
    const services: { pid: number; kind: string; displayName: string; category: string; confidence: string; evidence: never[] }[] = []
    for (let i = 0; i < 300; i++) {
      const pid = 4000 + i
      listeners.push(syntheticListener(5000 + i, pid))
      processes.push(syntheticProcess(pid))
      projects.push(syntheticProject(i))
      services.push({ pid, kind: 'vite', displayName: `Service ${i}`, category: 'frontend', confidence: 'high', evidence: [] })
    }
    const snapshot = makeSnapshot({
      listeners,
      processes,
      projects,
      services: services as never,
      projectLinks: projects.map((p, i) => ({ pid: 4000 + i, projectId: p.id })),
      controls: [],
      lastUpdated: 1_700_000_100_000,
    })
    const { getPortListeners, getDockerSnapshot } = await import('@/services/native/ports')
    vi.mocked(getPortListeners).mockResolvedValue(snapshot)
    vi.mocked(getDockerSnapshot).mockResolvedValue(
      makeDockerSnapshot({ containers: [], portOwnerships: [] }),
    )

    const ms = timedRender(<PortsPage />)
    expect(await screen.findByText('5022')).toBeInTheDocument() // a mid-table port
    expect(screen.getByText(/of 300/)).toBeInTheDocument()
    expectUnderBudget(ms, RENDER_CEILING_MS, 'PortsPage @300 rows')
  }, RENDER_TEST_TIMEOUT_MS)

  it('derives 150 service groups from 300 listener rows inside the ceiling', () => {
    const listeners: PortListener[] = []
    const processByPid = new Map<number, ProcessInfo>()
    for (let i = 0; i < 150; i++) {
      const pid = 4000 + i
      // Two listener rows per process (same port on two addresses) collapse
      // into one group — 300 rows → 150 groups.
      listeners.push(syntheticListener(5000 + i, pid))
      listeners.push({ ...syntheticListener(5000 + i, pid), localAddress: '0.0.0.0' })
      processByPid.set(pid, syntheticProcess(pid))
    }
    const started = performance.now()
    const groups = groupListenersByProcess(listeners, processByPid)
    const ms = performance.now() - started
    expect(groups).toHaveLength(150)
    expect(groups[0].ports).toEqual([5000])
    expect(groups[0].addresses).toEqual(['0.0.0.0', '127.0.0.1'])
    expectUnderBudget(ms, DERIVE_CEILING_MS, 'groupListenersByProcess @300 rows/150 groups')
  })

  it('renders 100 project cards inside the ceiling', async () => {
    const projects = Array.from({ length: 100 }, (_, i) => syntheticProject(i))
    const processByPid = new Map<number, ProcessInfo>()
    const listeners: PortListener[] = []
    for (let i = 0; i < 100; i++) {
      listeners.push(syntheticListener(7000 + i, 8000 + i))
      processByPid.set(8000 + i, syntheticProcess(8000 + i))
    }
    const { getPortListeners, getDockerSnapshot } = await import('@/services/native/ports')
    vi.mocked(getPortListeners).mockResolvedValue(
      makeSnapshot({
        listeners,
        processes: [...processByPid.values()],
        projects,
        projectLinks: projects.map((p, i) => ({ pid: 8000 + i, projectId: p.id })),
        controls: [],
      }),
    )
    vi.mocked(getDockerSnapshot).mockResolvedValue(
      makeDockerSnapshot({ containers: [], portOwnerships: [] }),
    )

    const ms = timedRender(<ProjectsPage />)
    await screen.findByText('proj-50')
    expectUnderBudget(ms, RENDER_CEILING_MS, 'ProjectsPage @100')
    // Project names appear once per card title; with 100 seeded projects
    // there are ≥100 cards (an “Unknown Project” card may also exist).
    expect(screen.getAllByText(/proj-\d+/).length).toBeGreaterThanOrEqual(100)
    expect(screen.getAllByText('Detection details').length).toBeGreaterThanOrEqual(100)
  }, RENDER_TEST_TIMEOUT_MS)

  it('renders 50 workspaces inside the ceiling', async () => {
    const workspaces = Array.from({ length: 50 }, (_, i) =>
      makeWorkspace({
        id: `ws-${i}`,
        name: `workspace-${i}`,
        projectRoot: `D:\\Projects\\ws-${i}`,
      }),
    )
    vi.mocked((await import('@/services/native/ports')).getWorkspacesReadiness).mockResolvedValue([])
    useWorkspaceStore.setState({ workspaces, loading: false, error: null } as Partial<Parameters<typeof useWorkspaceStore.setState>[0]>)
    const ms = timedRender(<WorkspacesPage />)
    expectUnderBudget(ms, RENDER_CEILING_MS, 'WorkspacesPage @50')
    expect(screen.getAllByText(/workspace-/).length).toBeGreaterThanOrEqual(50)
  }, RENDER_TEST_TIMEOUT_MS)

  it('renders 500 AI models inside the ceiling', () => {
    const runtime = makeAiRuntime({
      models: Array.from({ length: 500 }, (_, i) => ({ id: `model-${i}:8b` })),
    })
    useAiRuntimeStore.setState({ runtimes: [runtime], loading: false, error: null, lastUpdated: 1_700_000_200_000 })
    const ms = timedRender(<AiServicesPage />)
    expectUnderBudget(ms, RENDER_CEILING_MS, 'AiServicesPage @500 models')
    expect(screen.getAllByText(/model-/).length).toBeGreaterThanOrEqual(500)
  }, RENDER_TEST_TIMEOUT_MS)

  it('renders 100 Docker containers inside the ceiling', () => {
    const containers = Array.from({ length: 100 }, (_, i) =>
      makeContainer({ id: `c${i}`, shortId: `c${i}`, name: `container-${i}`, ports: [] }),
    )
    useDockerStore.setState({
      snapshot: makeDockerSnapshot({ containers, portOwnerships: [] }),
      loading: false,
      error: null,
      lastUpdated: 1_700_000_300_000,
    })
    const ms = timedRender(<DockerPage />)
    expectUnderBudget(ms, RENDER_CEILING_MS, 'DockerPage @100 containers')
    expect(screen.getAllByText(/container-/).length).toBeGreaterThanOrEqual(100)
  }, RENDER_TEST_TIMEOUT_MS)

  it('renders 100 history events inside the ceiling', () => {
    const history = Array.from({ length: 100 }, (_, i) => ({
      id: i + 1,
      at: 1_700_000_000_000 + i,
      action: (i % 2 === 0 ? 'service_start' : 'service_stop') as 'service_start' | 'service_stop',
      subject: `Action ${i}`,
      pid: 4212,
      outcome: 'success' as const,
      message: `Action ${i} completed`,
    }))
    useControlStore.setState({ history })
    const ms = timedRender(<HistoryPage />)
    expectUnderBudget(ms, RENDER_CEILING_MS, 'HistoryPage @100')
    expect(screen.getAllByText(/Action \d/).length).toBeGreaterThanOrEqual(100)
  }, RENDER_TEST_TIMEOUT_MS)

  it('snapshot stores survive repeated re-seeding at large size (no derivation blowup)', () => {
    const listeners: PortListener[] = Array.from({ length: 300 }, (_, i) =>
      syntheticListener(5000 + i, 4000 + i),
    )
    const processes = Array.from({ length: 300 }, (_, i) => syntheticProcess(4000 + i))
    const started = performance.now()
    for (let round = 0; round < 10; round++) {
      usePortsStore.setState({
        listeners: [...listeners].reverse(),
        processByPid: new Map(processes.map((p) => [p.pid, p])),
      })
    }
    const ms = performance.now() - started
    expectUnderBudget(ms, DERIVE_CEILING_MS, '10 store reseeds @300 rows')
    expect(usePortsStore.getState().listeners).toHaveLength(300)
  })
})
