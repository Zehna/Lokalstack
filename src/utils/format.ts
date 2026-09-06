/** Formatting helpers shared by the UI layer. Pure functions, no side effects. */

/** Format a byte count as a human-readable string, e.g. `6.1 GB`; `null` renders as `—`. */
export function formatBytes(bytes: number | null): string {
  if (bytes === null || !Number.isFinite(bytes) || bytes < 0) {
    return '—'
  }
  const units = ['B', 'KB', 'MB', 'GB', 'TB']
  let value = bytes
  let unitIndex = 0
  while (value >= 1024 && unitIndex < units.length - 1) {
    value /= 1024
    unitIndex++
  }
  const rounded = unitIndex === 0 ? value : Math.round(value * 10) / 10
  return `${rounded} ${units[unitIndex]}`
}

/** Clamp a percentage into the 0–100 range. */
export function clampPercent(value: number): number {
  if (!Number.isFinite(value)) {
    return 0
  }
  return Math.max(0, Math.min(100, value))
}

/** Render a port as `:3000`, or a placeholder when unknown. */
export function formatPort(port: number | null): string {
  return port === null ? '—' : `:${port}`
}

/**
 * Render a CPU percentage with one decimal, or `—` when there is no delta
 * sample yet (first observation — never fabricate a `0%`).
 */
export function formatCpuPercent(cpuPercent: number | null): string {
  if (cpuPercent === null || !Number.isFinite(cpuPercent)) {
    return '—'
  }
  return `${cpuPercent.toFixed(1)}%`
}

/** Render an epoch-milliseconds timestamp as a locale time, or `—`. */
export function formatTime(epochMs: number | null): string {
  if (epochMs === null || !Number.isFinite(epochMs)) {
    return '—'
  }
  return new Date(epochMs).toLocaleTimeString()
}
