import { useCallback, useState } from 'react'
import { ExternalLink, Ban } from 'lucide-react'

import type { PidControl } from '@/types/domain'
import { useControlStore } from '@/stores/controlStore'

/**
 * A destructive action button that requires two clicks: the first arms it
 * (label changes, it highlights), the second confirms. Auto-disarms after a
 * short window or when the pointer leaves the button.
 */
export function ConfirmButton({
  label,
  confirmLabel,
  onConfirm,
  disabled = false,
  title,
  className = '',
}: {
  label: string
  confirmLabel: string
  onConfirm: () => void
  disabled?: boolean
  title?: string
  className?: string
}) {
  const [armed, setArmed] = useState(false)

  const handleClick = useCallback(() => {
    if (!armed) {
      setArmed(true)
      return
    }
    setArmed(false)
    onConfirm()
  }, [armed, onConfirm])

  return (
    <button
      type="button"
      onClick={handleClick}
      onMouseLeave={() => {
        if (armed) setArmed(false)
      }}
      disabled={disabled}
      title={title}
      className={`inline-flex items-center gap-1 rounded border px-2 py-1 text-xs font-medium transition-colors disabled:cursor-not-allowed disabled:opacity-40 ${
        armed
          ? 'border-red-500 bg-red-900/60 text-white'
          : 'border-slate-700 bg-slate-900 text-slate-300 hover:border-red-700 hover:text-red-300'
      } ${className}`}
    >
      {armed ? confirmLabel : label}
    </button>
  )
}

/**
 * Open + End Process controls for one process, driven entirely by the
 * snapshot's `PidControl` capability.
 *
 * Honest semantics (hardened Phase 5): externally discovered processes have
 * NO targeted graceful stop (`gracefulStopSupported` is false — the process
 * was not launched in a LocalStack-managed process group), so the action is
 * labeled **End Process** and its copy says the process will be terminated.
 * It always requires the two-click confirmation, and the backend re-checks
 * identity and eligibility before acting. Nothing here decides eligibility —
 * the native layer does.
 */
export function ControlActions({
  control,
  compact = false,
  onStopped,
}: {
  control: PidControl
  /** Called after a successful stop so the parent can react (e.g. refresh). */
  compact?: boolean
  onStopped?: () => void
}) {
  const endProcessAction = useControlStore((state) => state.endProcess)
  const open = useControlStore((state) => state.open)
  const pending = useControlStore((state) => state.pending)

  const [error, setError] = useState<string | null>(null)

  const busy =
    pending !== null && (pending.pid === control.pid || pending.pid === null)

  const handleEndProcess = useCallback(async () => {
    const target = control.target
    if (target === null) return
    setError(null)
    try {
      const result = await endProcessAction(target, control.pid)
      if (result.stopped) {
        onStopped?.()
      } else {
        setError(result.message)
      }
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    }
  }, [control.target, control.pid, endProcessAction, onStopped])

  const handleOpen = useCallback(
    async (url: string) => {
      setError(null)
      try {
        await open(
          url,
          control.pid,
          control.target?.displayName ?? `PID ${control.pid}`,
        )
      } catch (cause) {
        setError(cause instanceof Error ? cause.message : String(cause))
      }
    },
    [control.pid, control.target, open],
  )

  const stopBlocked = !control.capability.canStop
  const primaryUrl = control.urls[0] ?? null

  return (
    <span className="inline-flex flex-wrap items-center gap-1.5">
      {primaryUrl !== null && control.capability.canOpen && (
        <button
          type="button"
          onClick={() => void handleOpen(primaryUrl)}
          disabled={busy}
          title={`Open ${primaryUrl} in your browser`}
          className="inline-flex items-center gap-1 rounded border border-slate-700 bg-slate-900 px-2 py-1 text-xs font-medium text-sky-300 transition-colors hover:border-sky-700 hover:text-sky-200 disabled:cursor-not-allowed disabled:opacity-40"
        >
          <ExternalLink className="h-3 w-3" strokeWidth={1.8} />
          {!compact && 'Open'}
        </button>
      )}

      {stopBlocked ? (
        <span
          className="cursor-not-allowed rounded border border-slate-800 bg-slate-950 px-2 py-1 text-xs text-slate-600"
          title={control.capability.reason || 'Control not available'}
        >
          <Ban className="mr-1 inline h-3 w-3" strokeWidth={1.8} />
          Stop unavailable
        </span>
      ) : (
        <ConfirmButton
          label={compact ? 'End' : 'End Process'}
          confirmLabel={`Terminate ${control.target?.displayName ?? ''} now`}
          onConfirm={() => void handleEndProcess()}
          disabled={busy || control.target === null}
          title={`Terminates this process (PID ${control.pid}). ${
            control.capability.gracefulStopSupported
              ? 'A targeted graceful stop is available.'
              : control.capability.gracefulStopReason +
                ' The process will be terminated after your confirmation.'
          }`}
        />
      )}

      {error !== null && (
        <span className="max-w-64 truncate text-xs text-red-400" title={error}>
          {error}
        </span>
      )}
    </span>
  )
}
