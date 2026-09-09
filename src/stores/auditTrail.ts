/**
 * Session audit trail — one bounded, shared history for every store.
 *
 * Phase 10A consolidation: previously each store kept its own `nextEntryId`
 * counter, so History entries from different stores could collide on React
 * keys. There is now exactly one id space and one bound (100 entries),
 * owned by this module. The internal action type is a closed string union
 * covering every action emitted across all stores.
 */
import { useControlStore, type ControlHistoryEntry } from '@/stores/controlStore'

/** Every audit-trail action emitted by any store (closed union). */
export type AuditAction = ControlHistoryEntry['action']

/** Bound of the shared in-memory audit trail (entries kept, oldest last). */
export const AUDIT_TRAIL_LIMIT = 100

let nextEntryId = 1

/** Test hook: reset the shared id counter (ids only — history lives in the store). */
export function resetAuditIds(): void {
  nextEntryId = 1
}

/**
 * Append one entry to the shared history with a globally unique id and the
 * common 100-entry bound. `pid` stays null for non-process events.
 */
export function recordAuditEntry(
  action: AuditAction,
  subject: string,
  outcome: ControlHistoryEntry['outcome'],
  message: string,
  pid: number | null = null,
): void {
  const control = useControlStore.getState()
  const next = [
    ...control.history,
    { id: nextEntryId++, at: Date.now(), action, subject, pid, outcome, message },
  ].slice(-AUDIT_TRAIL_LIMIT)
  useControlStore.setState({ history: next })
}
