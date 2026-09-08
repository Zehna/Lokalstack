import { useEffect, useState } from 'react'
import { BrainCircuit, ChevronDown, ChevronRight, Cpu, ExternalLink, RefreshCw } from 'lucide-react'

import { RefreshButton } from '@/app/components/RefreshButton'
import { useAiRuntimeStore } from '@/stores/aiRuntimeStore'
import { usePortListeners } from '@/hooks'
import type { AiHealth, AiRuntimeSnapshot } from '@/types/domain'
import { formatBytes, formatTime } from '@/utils/format'

/** Tailwind classes per runtime health state. */
const HEALTH_STYLES: Record<AiHealth, string> = {
  ready: 'border-emerald-900/60 bg-emerald-950/40 text-emerald-400',
  loading: 'border-sky-900/60 bg-sky-950/40 text-sky-400',
  busy: 'border-amber-900/60 bg-amber-950/40 text-amber-400',
  degraded: 'border-amber-900/60 bg-amber-950/40 text-amber-400',
  unavailable: 'border-red-900/60 bg-red-950/40 text-red-400',
  unknown: 'border-slate-700 bg-slate-950 text-slate-500',
}

/** Health badge for one runtime. */
function HealthBadge({ health }: { health: AiHealth }) {
  return (
    <span className={`rounded border px-1.5 py-0.5 text-xs font-medium uppercase ${HEALTH_STYLES[health]}`}>
      {health}
    </span>
  )
}

/** One expandable runtime card with its model inventory. */
function RuntimeCard({ runtime }: { runtime: AiRuntimeSnapshot }) {
  const [expanded, setExpanded] = useState(false)
  const refreshRuntime = useAiRuntimeStore((state) => state.refreshRuntime)
  const refreshing = useAiRuntimeStore((state) => state.refreshing)

  const caps = runtime.capabilities
  const loadedById = new Map(runtime.loadedModels.map((m) => [m.id, m]))

  return (
    <section className="rounded-lg border border-slate-800">
      <div className="flex items-center gap-3 border-b border-slate-800 bg-slate-900/60 px-4 py-3">
        <button
          type="button"
          onClick={() => setExpanded((value) => !value)}
          className="text-slate-500 hover:text-slate-300"
          aria-label={expanded ? 'Collapse details' : 'Expand details'}
        >
          {expanded ? <ChevronDown className="h-4 w-4" /> : <ChevronRight className="h-4 w-4" />}
        </button>
        <BrainCircuit className="h-4 w-4 text-violet-400" strokeWidth={1.8} />
        <h2 className="text-sm font-semibold uppercase tracking-wide text-slate-100">
          {runtime.displayName}
        </h2>
        <HealthBadge health={runtime.health} />
        <span className="font-mono text-xs text-slate-500">{runtime.endpoint}</span>
        <span className="flex-1" />
        {caps.version && runtime.version !== undefined && (
          <span className="rounded border border-slate-700 bg-slate-950 px-1.5 py-0.5 text-xs text-slate-400">
            v{runtime.version}
          </span>
        )}
        {caps.models && (
          <span className="text-xs text-slate-500">
            {runtime.models.length} installed · {runtime.loadedModels.length} loaded
          </span>
        )}
        <button
          type="button"
          disabled={refreshing}
          onClick={() => void refreshRuntime(runtime.runtimeId)}
          className="flex items-center gap-1.5 rounded border border-slate-700 bg-slate-900 px-2.5 py-1 text-xs text-slate-400 hover:text-slate-200 disabled:opacity-50"
          title="Manual refresh bypasses the runtime cache"
        >
          <RefreshCw className={`h-3 w-3 ${refreshing ? 'animate-spin' : ''}`} strokeWidth={1.8} /> Refresh
        </button>
      </div>

      <div className="space-y-3 p-4">
        {runtime.errorLabel !== undefined && (
          <p className="rounded border border-red-900/60 bg-red-950/30 px-3 py-2 text-xs text-red-300">
            {runtime.errorLabel}
          </p>
        )}

        {/* llama.cpp props */}
        {runtime.props !== undefined && (
          <div className="flex flex-wrap gap-2 text-xs text-slate-400">
            {runtime.props.contextSize !== undefined && (
              <span className="rounded border border-slate-700 bg-slate-950 px-2 py-0.5">
                Context {runtime.props.contextSize.toLocaleString()}
              </span>
            )}
            {runtime.props.slotsTotal !== undefined && (
              <span className="rounded border border-slate-700 bg-slate-950 px-2 py-0.5">
                Slots {runtime.props.slotsIdle ?? '—'}/{runtime.props.slotsTotal} idle
              </span>
            )}
          </div>
        )}

        {/* ComfyUI device/queue */}
        {runtime.resources !== undefined && (
          <div className="flex flex-wrap items-center gap-2 text-xs text-slate-400">
            {runtime.resources.device !== undefined && (
              <span className="flex items-center gap-1 rounded border border-slate-700 bg-slate-950 px-2 py-0.5">
                <Cpu className="h-3 w-3" strokeWidth={1.8} /> {runtime.resources.device}
              </span>
            )}
            {runtime.resources.vramTotalBytes !== undefined && (
              <span className="rounded border border-slate-700 bg-slate-950 px-2 py-0.5">
                VRAM {formatBytes(runtime.resources.vramFreeBytes ?? 0)} free / {formatBytes(runtime.resources.vramTotalBytes)}
              </span>
            )}
            {runtime.resources.queueRunning !== undefined && (
              <span className="rounded border border-slate-700 bg-slate-950 px-2 py-0.5">
                Queue {runtime.resources.queueRunning} running · {runtime.resources.queuePending ?? 0} pending
              </span>
            )}
          </div>
        )}

        {/* Model inventory — only when the provider exposes it. */}
        {caps.models && runtime.models.length > 0 && (
          <div className="overflow-x-auto">
            <table className="w-full text-left text-xs">
              <thead>
                <tr className="border-b border-slate-800 text-slate-500">
                  <th className="px-2 py-1.5 font-medium">Model</th>
                  {runtime.models.some((m) => m.parameterSize !== undefined) && (
                    <th className="px-2 py-1.5 font-medium">Parameters</th>
                  )}
                  {runtime.models.some((m) => m.quantization !== undefined) && (
                    <th className="px-2 py-1.5 font-medium">Quantization</th>
                  )}
                  {runtime.models.some((m) => m.sizeBytes !== undefined) && (
                    <th className="px-2 py-1.5 font-medium">Disk</th>
                  )}
                  {caps.loadedModels && <th className="px-2 py-1.5 font-medium">Loaded</th>}
                  {runtime.models.some((m) => m.vramBytes !== undefined) && (
                    <th className="px-2 py-1.5 font-medium">VRAM</th>
                  )}
                </tr>
              </thead>
              <tbody>
                {runtime.models.map((model) => {
                  const loaded = loadedById.get(model.id)
                  return (
                    <tr key={model.id} className="border-b border-slate-800/50 text-slate-300">
                      <td className="px-2 py-1.5 font-mono">{model.id}</td>
                      {runtime.models.some((m) => m.parameterSize !== undefined) && (
                        <td className="px-2 py-1.5">{model.parameterSize ?? '—'}</td>
                      )}
                      {runtime.models.some((m) => m.quantization !== undefined) && (
                        <td className="px-2 py-1.5">{model.quantization ?? '—'}</td>
                      )}
                      {runtime.models.some((m) => m.sizeBytes !== undefined) && (
                        <td className="px-2 py-1.5">{model.sizeBytes !== undefined ? formatBytes(model.sizeBytes) : '—'}</td>
                      )}
                      {caps.loadedModels && (
                        <td className="px-2 py-1.5">
                          {loaded !== undefined ? (
                            <span className="rounded border border-emerald-900/60 bg-emerald-950/40 px-1.5 py-0.5 text-emerald-400">
                              loaded
                            </span>
                          ) : (
                            <span className="text-slate-600">—</span>
                          )}
                        </td>
                      )}
                      {runtime.models.some((m) => m.vramBytes !== undefined) && (
                        <td className="px-2 py-1.5">
                          {(loaded?.vramBytes ?? model.vramBytes) !== undefined
                            ? formatBytes(loaded?.vramBytes ?? model.vramBytes ?? 0)
                            : '—'}
                        </td>
                      )}
                    </tr>
                  )
                })}
              </tbody>
            </table>
          </div>
        )}
        {caps.models && runtime.models.length === 0 && (
          <p className="text-xs text-slate-600">No models installed.</p>
        )}

        {/* Loaded-model VRAM summary (Ollama /api/ps). */}
        {caps.loadedModels && runtime.loadedModels.length > 0 && (
          <div className="rounded-lg border border-slate-800 bg-slate-950/60 px-3 py-2">
            <p className="text-xs font-semibold uppercase tracking-wider text-slate-500">Loaded in memory</p>
            {runtime.loadedModels.map((model) => (
              <p key={model.id} className="mt-1 text-xs text-slate-300">
                <span className="font-mono">{model.id}</span>
                {model.vramBytes !== undefined && (
                  <span className="text-slate-500"> · VRAM {formatBytes(model.vramBytes)}</span>
                )}
                {model.expiresAt !== undefined && (
                  <span className="text-slate-500"> · expires {formatTime(model.expiresAt)}</span>
                )}
              </p>
            ))}
          </div>
        )}

        {/* Details */}
        {expanded && (
          <div className="border-t border-slate-800/60 pt-2 text-xs text-slate-500">
            <p>Provider: {runtime.serviceKind} · PID {runtime.pid}</p>
            <p>
              Last probe {formatTime(runtime.capturedAt)} · {runtime.latencyMs} ms
            </p>
            <p className="mt-1">
              Capabilities:{' '}
              {[
                caps.version && 'version',
                caps.models && 'models',
                caps.loadedModels && 'loaded',
                caps.health && 'health',
                caps.metrics && 'metrics',
                caps.gpuStats && 'gpu',
                caps.queue && 'queue',
              ]
                .filter(Boolean)
                .join(', ') || 'none reported'}
            </p>
            <p className="mt-1 text-slate-600">
              Observability only — LocalStack never sends prompts, pulls, deletes, loads, or unloads
              models.
            </p>
          </div>
        )}
      </div>
    </section>
  )
}

/**
 * AI Services — real runtime observability for already-classified AI
 * services (Phase 3 detection reused). Health comes from adapter evidence,
 * never from a TCP listener alone.
 */
export function AiServicesPage() {
  usePortListeners()
  const load = useAiRuntimeStore((state) => state.load)
  const refresh = useAiRuntimeStore((state) => state.refresh)
  const runtimes = useAiRuntimeStore((state) => state.runtimes)
  const loading = useAiRuntimeStore((state) => state.loading)
  const refreshing = useAiRuntimeStore((state) => state.refreshing)
  const error = useAiRuntimeStore((state) => state.error)
  const lastUpdated = useAiRuntimeStore((state) => state.lastUpdated)

  useEffect(() => {
    void load()
  }, [load])

  return (
    <div className="mx-auto w-full max-w-5xl px-6 py-8">
      <div className="mb-6">
        <div className="flex items-center justify-between">
          <h1 className="text-xl font-semibold text-slate-100">AI Services</h1>
          <RefreshButton onClick={() => void refresh()} refreshing={refreshing} />
        </div>
        <p className="mt-1 text-sm text-slate-500">
          Local AI runtimes discovered by service classification, inspected read-only over loopback.
          Updated {formatTime(lastUpdated)}.
        </p>
      </div>

      {error !== null && (
        <div className="mb-4 rounded-lg border border-red-900/60 bg-red-950/30 px-4 py-3 text-sm text-red-300">
          {error}
        </div>
      )}

      {loading ? (
        <div className="rounded-lg border border-slate-800 bg-slate-900/40 px-4 py-6 text-center text-sm text-slate-500">
          Probing local AI runtimes…
        </div>
      ) : runtimes.length === 0 ? (
        <div className="rounded-lg border border-dashed border-slate-800 p-10 text-center">
          <p className="text-sm text-slate-400">No AI runtimes detected.</p>
          <p className="mt-1 text-xs text-slate-600">
            Start Ollama, llama.cpp server, or ComfyUI and it will appear here automatically.
          </p>
        </div>
      ) : (
        <div className="space-y-4">
          {runtimes.map((runtime) => (
            <RuntimeCard key={runtime.runtimeId} runtime={runtime} />
          ))}
        </div>
      )}

      <p className="mt-6 flex items-center gap-1.5 text-xs text-slate-600">
        <ExternalLink className="h-3 w-3" strokeWidth={1.8} />
        Probes are read-only, loopback-only, and never trigger model loads or inference.
      </p>
    </div>
  )
}
