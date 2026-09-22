/** In-app notification banner (role=status, spec §28/§30). */
export function NotificationBanner({
  message,
  onDismiss,
}: {
  message: string | null
  onDismiss: () => void
}) {
  if (message === null) return null
  return (
    <div
      role="status"
      className="flex items-center justify-between gap-4 rounded border border-sky-800 bg-sky-950/60 px-3 py-2"
    >
      <p className="text-sm text-sky-100">{message}</p>
      <button
        type="button"
        aria-label="Dismiss notification"
        onClick={onDismiss}
        className="rounded border border-sky-700 px-2 py-0.5 text-xs text-sky-200 hover:bg-sky-900"
      >
        Dismiss
      </button>
    </div>
  )
}
