/** Support Bundles tab: metadata-first rows with per-row actions. */
import type { BundleMetaDto } from '@/types/domain'
import { EXPORT_PROFILE_LABELS } from '../types'

export function BundlesTab({
  bundles,
  onExport,
  onDelete,
  onMarkReviewed,
}: {
  bundles: BundleMetaDto[]
  onExport: (bundleId: string) => void
  onDelete: (bundleId: string) => void
  onMarkReviewed: (bundleId: string) => void
}) {
  const openFolder = async () => {
    const { openDiagnosticsFolder } = await import('@/services/native/diagnostics')
    await openDiagnosticsFolder().catch(() => {})
  }

  return (
    <section aria-label="Support bundles">
      <div className="mb-3 flex justify-end">
        <button
          type="button"
          onClick={() => void openFolder()}
          className="rounded border border-slate-700 px-2 py-1 text-xs text-slate-200 hover:bg-slate-800"
        >
          Open Folder
        </button>
      </div>
      {bundles.length === 0 ? (
        <p className="text-sm text-slate-500">No support bundles stored.</p>
      ) : (
        <ul className="divide-y divide-slate-800/60 rounded border border-slate-800">
          {bundles.map((b) => (
            <li key={b.bundleId} className="flex items-start justify-between gap-4 px-4 py-3">
              <div className="min-w-0">
                <p className="text-sm font-medium text-slate-200">
                  {b.subsystem} · {b.severity} · {new Date(b.createdAtMs).toLocaleString()}
                </p>
                <p className="text-xs text-slate-500">
                  Trigger: {b.trigger} · Size: {Math.ceil(b.encryptedSizeBytes / 1024)} KiB ·{' '}
                  Integrity: {b.integrity} · Review status: {b.reviewState === 'new' ? 'New' : 'Reviewed'}
                </p>
                <p className="text-xs text-slate-500">Fingerprint: {b.fingerprint}</p>
              </div>
              <div className="flex shrink-0 flex-wrap gap-2">
                <button
                  type="button"
                  aria-label={`Export bundle ${b.bundleId}`}
                  onClick={() => onExport(b.bundleId)}
                  className="rounded border border-slate-700 px-2 py-1 text-xs text-slate-200 hover:bg-slate-800"
                >
                  Export
                </button>
                <button
                  type="button"
                  aria-label={`Delete bundle ${b.bundleId}`}
                  onClick={() => onDelete(b.bundleId)}
                  className="rounded border border-slate-700 px-2 py-1 text-xs text-slate-200 hover:bg-slate-800"
                >
                  Delete
                </button>
                {b.reviewState === 'new' && (
                  <button
                    type="button"
                    aria-label={`Mark bundle ${b.bundleId} reviewed`}
                    onClick={() => onMarkReviewed(b.bundleId)}
                    className="rounded border border-slate-700 px-2 py-1 text-xs text-slate-200 hover:bg-slate-800"
                  >
                    Mark Reviewed
                  </button>
                )}
              </div>
            </li>
          ))}
        </ul>
      )}
      <p className="mt-3 text-xs text-slate-500">
        Export profiles: {EXPORT_PROFILE_LABELS['safe-share']},{' '}
        {EXPORT_PROFILE_LABELS['developer-detail']}, {EXPORT_PROFILE_LABELS['full-forensics']}.{' '}
        Exported support bundles are not encrypted by LocalStack. Review them before sharing.
      </p>
    </section>
  )
}
