/**
 * Domain adapter for the Services page: groups listener rows by owning PID
 * and merges the process intelligence + service identity from the native
 * snapshot.
 *
 * This is pure derived frontend logic — it never scans anything and never
 * calls Tauri; the native layer already sampled each PID exactly once per
 * cycle, this just reshapes the result per view.
 */

import type { PortListener, ProcessInfo, ServiceIdentity } from '@/types/domain'

/** One active local process with its ports, identity and resources. */
export interface GroupedProcess {
  /** Owning process ID. */
  pid: number
  /** Process intelligence for the PID, or `null` if the snapshot lacks it. */
  process: ProcessInfo | null
  /** Service identity for the PID, or `null` if the snapshot lacks it. */
  identity: ServiceIdentity | null
  /** Display name: service identity, else process name, else honest fallback. */
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
 * IPv4 + IPv6) collapse into one card with a port list — the native layer
 * sampled each PID once; this adapter adds no scanning.
 */
export function groupListenersByProcess(
  listeners: PortListener[],
  processByPid: ReadonlyMap<number, ProcessInfo>,
  serviceByPid: ReadonlyMap<number, ServiceIdentity> = new Map(),
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
    const identity = serviceByPid.get(pid) ?? null
    groups.push({
      pid,
      process,
      identity,
      displayName:
        identity?.displayName ?? process?.name ?? 'Unavailable',
      ports: [...ports].sort((a, b) => a - b),
      addresses: [...addresses].sort((a, b) => a.localeCompare(b)),
      cpuPercent: process?.cpuPercent ?? null,
      memoryBytes: process?.memoryBytes ?? null,
      accessible: process?.accessible ?? false,
    })
  }

  // Most interesting first: identified services, then more ports, then PID.
  return groups.sort((a, b) => {
    const aIdentified = a.identity !== null && a.identity.category !== 'unknown' ? 0 : 1
    const bIdentified = b.identity !== null && b.identity.category !== 'unknown' ? 0 : 1
    if (aIdentified !== bIdentified) return aIdentified - bIdentified
    const byPorts = b.ports.length - a.ports.length
    if (byPorts !== 0) return byPorts
    return a.pid - b.pid
  })
}
