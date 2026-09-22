/**
 * Full Forensics fresh per-export confirmation (spec §9/§12-C).
 *
 * This is a USER-INTENT / UX gate: the backend validates bundle ID +
 * profile + trusted flow; this modal records explicit user intent in the
 * official LocalStack flow. Focus is moved into the dialog on open.
 */
import { useEffect, useRef } from 'react'
import { FULL_FORENSICS_CATEGORIES } from '../types'

export function FullForensicsConfirm({
  onCancel,
  onConfirm,
}: {
  onCancel: () => void
  onConfirm: () => void
}) {
  const confirmRef = useRef<HTMLButtonElement>(null)

  useEffect(() => {
    confirmRef.current?.focus()
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onCancel()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [onCancel])

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label="Confirm Full Forensics export"
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/70"
    >
      <div className="w-[30rem] rounded-lg border border-amber-700 bg-slate-900 p-4">
        <h2 className="text-sm font-semibold text-amber-300">
          Confirm Full Forensics export
        </h2>
        <p className="mt-2 text-sm text-slate-300">
          Full Forensics retains the following sensitive identity categories:
        </p>
        <ul className="mt-2 list-disc pl-5 text-sm text-slate-400">
          {FULL_FORENSICS_CATEGORIES.map((c) => (
            <li key={c}>{c}</li>
          ))}
        </ul>
        <p className="mt-2 text-sm text-slate-300">
          Passwords, tokens, API keys, authorization headers, cookies, and
          private-key material are still removed. The exported file is NOT
          encrypted by LocalStack — review before sharing.
        </p>
        <div className="mt-4 flex justify-end gap-2">
          <button
            type="button"
            onClick={onCancel}
            className="rounded border border-slate-700 px-3 py-1.5 text-sm text-slate-300 hover:bg-slate-800"
          >
            Cancel
          </button>
          <button
            ref={confirmRef}
            type="button"
            onClick={onConfirm}
            className="rounded bg-amber-600 px-3 py-1.5 text-sm font-medium text-white hover:bg-amber-500"
          >
            Confirm export
          </button>
        </div>
      </div>
    </div>
  )
}
