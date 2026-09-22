/**
 * GitHub Safe Share issue workflow (Phase 11C Task 19, spec §24).
 * Explicit user action → backend prepares a Safe-Share title/body + the
 * FIXED repository issue URL → user confirms → the backend-provided URL is
 * opened and the Safe-Share ZIP is exported locally. The bundle is never
 * uploaded automatically; no PAT/OAuth exists anywhere in this flow.
 */
import { useEffect, useState } from 'react'
import {
  exportSupportBundle,
  prepareLocalstackGithubIssue,
  revealExportResult,
} from '@/services/native/diagnostics'
import type { GitHubIssueDraftDto } from '@/types/domain'

export function GitHubIssueDialog({
  bundleId,
  onClose,
}: {
  bundleId: string | undefined
  onClose: () => void
}) {
  const [draft, setDraft] = useState<GitHubIssueDraftDto | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [exportedFile, setExportedFile] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  useEffect(() => {
    let cancelled = false
    prepareLocalstackGithubIssue(bundleId)
      .then((d) => {
        if (!cancelled) setDraft(d)
      })
      .catch((e: unknown) => {
        if (!cancelled) setError(e instanceof Error ? e.message : String(e))
      })
    return () => {
      cancelled = true
    }
  }, [bundleId])

  async function confirm() {
    if (!draft) return
    setBusy(true)
    try {
      // window.open receives ONLY the backend-provided fixed URL — the
      // frontend never constructs or modifies a GitHub URL.
      window.open(draft.issueUrl, '_blank', 'noopener,noreferrer')
      const outcome = await exportSupportBundle(bundleId ?? '', 'safe-share', false)
      setExportedFile(outcome.fileName)
      await revealExportResult(outcome.exportId).catch(() => {})
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label="Create GitHub issue"
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/70 p-4"
    >
      <div className="w-full max-w-lg rounded-lg border border-slate-700 bg-slate-900 p-5">
        <h2 className="mb-3 text-lg font-semibold text-slate-100">Create GitHub Issue</h2>

        {error !== null && (
          <div role="alert" className="mb-3 rounded border border-red-700 bg-red-950/60 px-3 py-2 text-sm text-red-200">
            GitHub issue preparation failed: {error}
          </div>
        )}

        {draft !== null && (
          <>
            <div className="mb-3 rounded border border-amber-700 bg-amber-950/50 px-3 py-2 text-xs text-amber-200">
              This prepares a Safe Share summary and a sanitized support bundle.
              Review the exported ZIP before attaching it — exported bundles are
              not encrypted by LocalStack. The bundle is never uploaded
              automatically; attaching it is your explicit choice.
            </div>
            <div className="mb-3 max-h-60 overflow-auto rounded border border-slate-700 bg-slate-950/70 p-3 text-xs text-slate-300">
              <p className="mb-1 font-semibold text-slate-200">{draft.title}</p>
              <pre className="whitespace-pre-wrap font-mono">{draft.body}</pre>
            </div>
          </>
        )}

        {exportedFile !== null && (
          <p role="status" className="mb-3 text-sm text-emerald-300">
            Exported: {exportedFile}
          </p>
        )}

        <div className="flex justify-end gap-2">
          <button
            type="button"
            onClick={onClose}
            className="rounded border border-slate-700 px-3 py-1.5 text-sm text-slate-200 hover:bg-slate-800"
          >
            Cancel
          </button>
          <button
            type="button"
            disabled={draft === null || busy || exportedFile !== null}
            onClick={() => void confirm()}
            className="rounded bg-emerald-700 px-3 py-1.5 text-sm font-medium text-white hover:bg-emerald-600 disabled:opacity-50"
          >
            Continue to GitHub
          </button>
        </div>
      </div>
    </div>
  )
}
