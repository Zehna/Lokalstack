/** Export profile chooser (spec §12): Safe Share / Developer Detail / Full
 * Forensics, with the portable-ZIP warning. */
import type { ExportProfileDto } from '@/types/domain'
import { EXPORT_PROFILE_LABELS } from '../types'

const PROFILES: ExportProfileDto[] = ['safe-share', 'developer-detail', 'full-forensics']

export function ExportProfileDialog({
  open,
  bundleId,
  onSelect,
  onCancel,
}: {
  open: boolean
  bundleId: string | null
  onSelect: (profile: ExportProfileDto) => void
  onCancel: () => void
}) {
  if (!open || bundleId === null) return null
  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label="Choose export profile"
      className="fixed inset-0 z-40 flex items-center justify-center bg-black/60"
    >
      <div className="w-[28rem] rounded-lg border border-slate-700 bg-slate-900 p-4">
        <h2 className="text-sm font-semibold text-slate-100">Choose export profile</h2>
        <p className="mt-1 text-xs text-slate-500">
          Exported support bundles are not encrypted by LocalStack. Review them before sharing.
        </p>
        <div className="mt-3 grid gap-2">
          {PROFILES.map((p) => (
            <button
              key={p}
              type="button"
              onClick={() => onSelect(p)}
              className="rounded border border-slate-700 px-3 py-2 text-left text-sm text-slate-200 hover:bg-slate-800"
            >
              {EXPORT_PROFILE_LABELS[p]}
              <span className="block text-xs text-slate-500">{profileHint(p)}</span>
            </button>
          ))}
        </div>
        <div className="mt-3 flex justify-end">
          <button
            type="button"
            onClick={onCancel}
            className="rounded border border-slate-700 px-3 py-1.5 text-sm text-slate-300 hover:bg-slate-800"
          >
            Cancel
          </button>
        </div>
      </div>
    </div>
  )
}

function profileHint(p: ExportProfileDto): string {
  switch (p) {
    case 'safe-share':
      return 'Machine identity removed; generalized paths. Default for support.'
    case 'developer-detail':
      return 'Full paths and project names; unique machine IDs removed.'
    case 'full-forensics':
      return 'Approved local identity retained; secrets still removed. Requires confirmation.'
  }
}
