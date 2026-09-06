/**
 * Domain adapter for the Services page: groups listener rows by owning PID
 * and merges the process intelligence from the native snapshot.
 *
 * This is pure derived frontend logic — it never scans anything and never
 * calls Tauri; the native layer already sampled each PID exactly once per
 * cycle, this just reshapes the result per view.
 */

import type { PortListener, ProcessInfo } from '@/types/domain'

/** One active local process with the ports its listener rows occupy. */
export interface GroupedProcess {
  /** Owning process ID. */
  pid: number
  /**
   * Process intelligence for the PID, or `null` if the snapshot has no
   * entry (should not happen while the native merge is intact — kept honest
   * instead of assumed).
   */
  process: ProcessInfo | null
  /** Display name: process name when known, otherwise the honest fallback. */
  displayName: string
  /** Distinct ports this process listens on, ascending. */
  ports: number[]
  /** Distinct bind addresses across those listener rows, ascending. */
  addresses: string[]
  /** CPU percent from the last delta window, or null (first sample). */
  cpuPercent: number | null
  /** Working set in bytes, or null when inaccessible. */
  memoryBytes: number | null
  /** Whether the process could be inspected at all. */
  accessible: boolean
}

/**
 * Group listener rows by PID. Multiple listener rows (e.g. the same port on
 * IPv4 + IPv6) collapse into one card with a port list — exactly the
 * "ACTIVE LOCAL PROCESSES" concept from the roadmap, with zero extra
 * native scanning.
 */
export function groupListenersByProcess(
  listeners: PortListener[],
  processByPid: ReadonlyMap<number, ProcessInfo>,
): GroupedProcess[] {
  const byPid = new Map<number, { ports: Set<number>; addresses: Set<string> }>()

  for (const listener of listeners) {
    let entry = byPid.get(listener.pid)
    if (entry === undefined) {
      entry = { ports: new Set(), addresses: new Set() }
      byPid.set(listener.pid, entry)
    }
    entry.ports.add(listener.port)
    entry.addresses.add(listener.localAddress)
  }

  const groups: GroupedProcess[] = []
  for (const [pid, { ports, addresses }] of byPid) {
    const process = processByPid.get(pid) ?? null
    groups.push({
      pid,
      process,
      displayName: process?.name ?? 'Unavailable',
      ports: [...ports].sort((a, b) => a - b),
      addresses: [...addresses].sort((a, b) => a.localeCompare(b)),
      cpuPercent: process?.cpuPercent ?? null,
      memoryBytes: process?.memoryBytes ?? null,
      accessible: process?.accessible ?? false,
    })
  }

  // Most interesting first: named processes, then more ports, then lower PID.
  return groups.sort((a, b) => {
    if (a.accessible !== b.accessible) return a.accessible ? -1 : 1
    const byPorts = b.ports.length - a.ports.length
    if (byPorts !== 0) return byPorts
    return a.pid - b.pid
  })
}
