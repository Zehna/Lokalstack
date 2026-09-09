/**
 * Shared test fixtures — minimal, honest domain objects mirroring the Rust
 * DTOs. Tests import and tweak these instead of hand-building large literals,
 * so a DTO field rename breaks one file, not thirty.
 */
import type {
  AiRuntimeSnapshot,
  ContainerPortOwnership,
  ControlCapability,
  DockerContainer,
  DockerEngineSnapshot,
  ManagedActionOutcome,
  PortConflictReport,
  PortListener,
  PidControl,
  PortListenersResponse,
  ProcessInfo,
  ProjectIdentity,
  ServiceIdentity,
  WorkspaceReadinessView,
  WorkspaceView,
} from '@/types/domain'

export const LISTENER: PortListener = {
  protocol: 'tcp',
  ipVersion: 4,
  localAddress: '127.0.0.1',
  port: 3000,
  pid: 4212,
  state: 'LISTEN',
}

export function makeProcess(overrides: Partial<ProcessInfo> = {}): ProcessInfo {
  return {
    pid: 4212,
    name: 'node.exe',
    executablePath: 'C:\\Program Files\\nodejs\\node.exe',
    startedAt: 1_700_000_000_000,
    memoryBytes: 120_000_000,
    commandLine: 'node vite --port 3000',
    cpuPercent: 1.5,
    accessible: true,
    ...overrides,
  }
}

export function makeService(overrides: Partial<ServiceIdentity> = {}): ServiceIdentity {
  return {
    kind: 'vite',
    displayName: 'Vite',
    category: 'frontend',
    confidence: 'high',
    evidence: [{ source: 'command_line', value: 'vite' }],
    ...overrides,
  }
}

export function makeProject(overrides: Partial<ProjectIdentity> = {}): ProjectIdentity {
  return {
    id: 'D:\\Projects\\historyai',
    name: 'historyai',
    rootPath: 'D:\\Projects\\historyai',
    kind: 'node_js',
    git: { isRepository: true, rootPath: 'D:\\Projects\\historyai', branch: 'main' },
    packageManager: 'npm',
    startCommand: { command: 'npm run dev', confidence: 'high', evidence: [] },
    confidence: 'exact',
    evidence: [],
    ...overrides,
  }
}

export const SYSTEM_PID = 1234

export function makeSystemControl(): PidControl {
  const capability: ControlCapability = {
    canOpen: false,
    canStop: false,
    gracefulStopSupported: false,
    gracefulStopReason: 'Process was not launched in a LocalStack-managed process group.',
    canRestart: false,
    reason: 'Windows system process',
  }
  return { pid: SYSTEM_PID, capability, urls: [], target: null }
}

export function makeDevControl(pid = 4212, id = 'target-abc'): PidControl {
  const capability: ControlCapability = {
    canOpen: true,
    canStop: true,
    gracefulStopSupported: false,
    gracefulStopReason: 'Process was not launched in a LocalStack-managed process group.',
    canRestart: false,
    reason: '',
  }
  return {
    pid,
    capability,
    urls: ['http://localhost:3000'],
    target: { id, displayName: 'Vite — historyai', processName: 'node.exe' },
  }
}

/** Full snapshot response with one dev service and one system process. */
export function makeSnapshot(
  overrides: Partial<PortListenersResponse> = {},
): PortListenersResponse {
  return {
    listeners: [LISTENER],
    processes: [makeProcess()],
    services: [{ pid: 4212, ...makeService() }],
    projects: [makeProject()],
    projectLinks: [{ pid: 4212, projectId: 'D:\\Projects\\historyai' }],
    controls: [makeDevControl()],
    lastUpdated: 1_700_000_100_000,
    durationMs: 42,
    ...overrides,
  }
}

export function makeWorkspace(overrides: Partial<WorkspaceView> = {}): WorkspaceView {
  return {
    id: 'ws-1',
    projectRoot: 'D:\\Projects\\historyai',
    name: 'historyai',
    status: 'stopped',
    services: [
      {
        id: 'svc-1',
        name: 'Vite Dev Server',
        role: 'frontend',
        expectedPort: 3000,
        launchSpecId: 'spec-1',
        source: 'package.json scripts.dev',
        managed: null,
      },
    ],
    ...overrides,
  }
}

export function makeReadiness(overrides: Partial<WorkspaceReadinessView> = {}): WorkspaceReadinessView {
  return {
    workspaceId: 'ws-1',
    workspaceName: 'historyai',
    status: 'running',
    issues: [],
    dependencies: [],
    conflicts: [],
    ...overrides,
  }
}

export function makeConflictReport(
  overrides: Partial<PortConflictReport> = {},
): PortConflictReport {
  return {
    requestedPort: 3000,
    requestedBy: { workspaceName: 'historyai' },
    kind: 'other_project',
    severity: 'blocking',
    message: 'Port 3000 is owned by another project.',
    owner: {
      pid: 999,
      processName: 'node.exe',
      projectName: 'other-project',
      lifecycle: 'external',
    },
    listeners: [{ ipVersion: 4, address: '0.0.0.0', pid: 999 }],
    resolutions: ['show_owner', 'find_free_port'],
    ...overrides,
  }
}

export function makeOutcome(overrides: Partial<ManagedActionOutcome> = {}): ManagedActionOutcome {
  return { ok: true, code: 'OK', message: 'Started.', ...overrides }
}

export function makeAiRuntime(overrides: Partial<AiRuntimeSnapshot> = {}): AiRuntimeSnapshot {
  return {
    runtimeId: 'ollama-11434',
    pid: 8000,
    serviceKind: 'ollama',
    displayName: 'Ollama',
    endpoint: 'http://127.0.0.1:11434',
    health: 'ready',
    capabilities: {
      models: true,
      loadedModels: true,
      version: true,
      health: true,
      metrics: false,
      gpuStats: false,
      queue: false,
    },
    models: [{ id: 'llama3.1:8b' }],
    loadedModels: [],
    capturedAt: 1_700_000_200_000,
    latencyMs: 12,
    ...overrides,
  }
}

export function makeContainer(overrides: Partial<DockerContainer> = {}): DockerContainer {
  return {
    id: 'abc123def456',
    shortId: 'abc123def456',
    name: 'historyai-frontend-1',
    image: 'historyai/frontend:dev',
    state: 'running',
    status: 'Up 2 minutes',
    health: 'none',
    ports: [{ protocol: 'tcp', containerPort: 3000, hostIp: '0.0.0.0', hostPort: 3000, published: true }],
    networks: [{ name: 'bridge', ipAddress: '172.17.0.2' }],
    mounts: [],
    ...overrides,
  }
}

export function makeDockerSnapshot(
  overrides: Partial<DockerEngineSnapshot> = {},
): DockerEngineSnapshot {
  const container = makeContainer()
  const ownership: ContainerPortOwnership = {
    hostPort: 3000,
    containerId: container.id,
    containerName: container.name,
    containerPort: 3000,
    protocol: 'tcp',
    image: container.image,
  }
  return {
    available: true,
    engine: { version: '27.0.0', apiVersion: '1.47', os: 'linux', arch: 'x86_64' },
    containers: [container],
    projectLinks: [],
    portOwnerships: [ownership],
    truncated: false,
    capturedAt: 1_700_000_300_000,
    latencyMs: 30,
    ...overrides,
  }
}
