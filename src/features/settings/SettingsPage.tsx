import { useEffect, useState } from 'react'
import { RotateCcw, Settings as SettingsIcon } from 'lucide-react'

import { RefreshButton } from '@/app/components/RefreshButton'
import { useSettingsStore } from '@/stores/settingsStore'
import type { AppSettings, CloseBehavior } from '@/types/domain'

/** Section wrapper: labeled group of related settings (spec §AE). */
function Section({
  title,
  children,
}: {
  title: string
  children: React.ReactNode
}) {
  return (
    <section className="rounded-lg border border-slate-800 bg-slate-900/40">
      <h2 className="border-b border-slate-800 px-4 py-2.5 text-xs font-semibold uppercase tracking-wide text-slate-500">
        {title}
      </h2>
      <div className="divide-y divide-slate-800/60">{children}</div>
    </section>
  )
}

/** One labeled setting row: name, description, and the control. */
function Row({
  label,
  description,
  htmlFor,
  children,
}: {
  label: string
  description: string
  htmlFor?: string
  children: React.ReactNode
}) {
  return (
    <div className="flex items-center justify-between gap-6 px-4 py-3">
      <div className="min-w-0">
        <label htmlFor={htmlFor} className="block text-sm font-medium text-slate-200">
          {label}
        </label>
        <p className="mt-0.5 text-xs text-slate-500">{description}</p>
      </div>
      <div className="shrink-0">{children}</div>
    </div>
  )
}

/** Accessible toggle: real checkbox input styled as a switch (spec §AE —
 * keyboard-operable, labeled, not color-only). */
function Toggle({
  checked,
  onChange,
  label,
}: {
  checked: boolean
  onChange: (value: boolean) => void
  label: string
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      onClick={() => onChange(!checked)}
      className={`relative h-6 w-11 rounded-full border transition-colors focus:outline-none focus-visible:ring-2 focus-visible:ring-sky-500 ${
        checked
          ? 'border-sky-600 bg-sky-600/80'
          : 'border-slate-700 bg-slate-800'
      }`}
    >
      <span
        className={`absolute top-0.5 h-4.5 w-4.5 rounded-full bg-slate-200 transition-all ${
          checked ? 'left-[1.375rem]' : 'left-0.5'
        }`}
        style={{ height: '1.125rem', width: '1.125rem' }}
      />
    </button>
  )
}

/** Select wrapper with a visible label association. */
function Select<T extends string>({
  id,
  value,
  options,
  onChange,
}: {
  id: string
  value: T
  options: { value: T; label: string }[]
  onChange: (value: T) => void
}) {
  return (
    <select
      id={id}
      value={value}
      onChange={(event) => onChange(event.target.value as T)}
      className="rounded-md border border-slate-700 bg-slate-900 px-2.5 py-1.5 text-xs text-slate-200 focus:outline-none focus-visible:ring-2 focus-visible:ring-sky-500"
    >
      {options.map((option) => (
        <option key={option.value} value={option.value}>
          {option.label}
        </option>
      ))}
    </select>
  )
}

/**
 * Settings (Phase 10C) — operational preferences only. Every change saves
 * immediately through the backend (validated/clamped) and reconfigures
 * live polling without an app restart (spec §I). Startup registration is
 * toggled through its dedicated command and always shows OS reality.
 */
export function SettingsPage() {
  const settings = useSettingsStore((state) => state.settings)
  const loading = useSettingsStore((state) => state.loading)
  const saving = useSettingsStore((state) => state.saving)
  const error = useSettingsStore((state) => state.error)
  const correctedNote = useSettingsStore((state) => state.correctedNote)
  const loaded = useSettingsStore((state) => state.loaded)
  const load = useSettingsStore((state) => state.load)
  const save = useSettingsStore((state) => state.save)
  const reset = useSettingsStore((state) => state.reset)
  const toggleStartup = useSettingsStore((state) => state.toggleStartup)
  const dismissNotices = useSettingsStore((state) => state.dismissNotices)

  // Local echo for the interval input (numeric typing); committed on blur/
  // Enter. Everything else saves immediately on toggle.
  const [intervalText, setIntervalText] = useState<string>('')
  const [confirmingReset, setConfirmingReset] = useState(false)

  useEffect(() => {
    if (!loaded) void load()
  }, [loaded, load])

  useEffect(() => {
    if (settings !== null && intervalText === '') {
      setIntervalText(String(settings.portRefreshIntervalMs))
    }
  }, [settings, intervalText])

  const commitInterval = (): void => {
    if (settings === null) return
    const parsed = Number(intervalText)
    if (!Number.isFinite(parsed) || parsed <= 0) {
      // Invalid input: revert to the backend-validated value (spec §D).
      setIntervalText(String(settings.portRefreshIntervalMs))
      return
    }
    void save({ portRefreshIntervalMs: Math.round(parsed) })
  }

  const patch = (partial: Partial<AppSettings>): void => {
    void save(partial)
  }

  if (loading && settings === null) {
    return (
      <div className="mx-auto w-full max-w-5xl px-6 py-8">
        <h1 className="text-xl font-semibold text-slate-100">Settings</h1>
        <p className="mt-1 text-sm text-slate-500">Loading settings…</p>
      </div>
    )
  }

  if (settings === null) {
    return (
      <div className="mx-auto w-full max-w-5xl px-6 py-8">
        <h1 className="text-xl font-semibold text-slate-100">Settings</h1>
        <p className="mt-1 text-sm text-red-300">
          {error ?? 'Settings are unavailable right now.'}
        </p>
        <div className="mt-3">
          <RefreshButton onClick={() => void load()} refreshing={loading} label="Retry" />
        </div>
      </div>
    )
  }

  return (
    <div className="mx-auto w-full max-w-5xl px-6 py-8">
      <div className="mb-6">
        <div className="flex items-center justify-between">
          <h1 className="flex items-center gap-2 text-xl font-semibold text-slate-100">
            <SettingsIcon className="h-5 w-5 text-slate-400" strokeWidth={1.8} />
            Settings
          </h1>
          <div className="flex items-center gap-2">
            {confirmingReset ? (
              <>
                <span className="text-xs text-slate-400">Reset all settings to defaults?</span>
                <button
                  type="button"
                  onClick={() => {
                    void reset()
                    setConfirmingReset(false)
                  }}
                  className="rounded-md border border-red-800 bg-red-950/50 px-2.5 py-1.5 text-xs font-medium text-red-300 hover:bg-red-900/50 focus:outline-none focus-visible:ring-2 focus-visible:ring-red-500"
                >
                  Confirm reset
                </button>
                <button
                  type="button"
                  onClick={() => setConfirmingReset(false)}
                  className="rounded-md border border-slate-700 bg-slate-900 px-2.5 py-1.5 text-xs font-medium text-slate-300 hover:bg-slate-800 focus:outline-none focus-visible:ring-2 focus-visible:ring-sky-500"
                >
                  Cancel
                </button>
              </>
            ) : (
              <button
                type="button"
                onClick={() => setConfirmingReset(true)}
                className="inline-flex items-center gap-1.5 rounded-md border border-slate-700 bg-slate-900 px-2.5 py-1.5 text-xs font-medium text-slate-300 transition-colors hover:border-slate-600 hover:bg-slate-800 focus:outline-none focus-visible:ring-2 focus-visible:ring-sky-500"
              >
                <RotateCcw className="h-3.5 w-3.5" strokeWidth={1.8} />
                Reset to Defaults
              </button>
            )}
          </div>
        </div>
        <p className="mt-1 text-sm text-slate-500">
          Operational preferences only — changes apply immediately and persist locally.
        </p>
        <p className="mt-0.5 text-xs text-slate-600">
          Version {import.meta.env.VITE_APP_VERSION} — derived from app metadata, never
          hardcoded.
        </p>
      </div>

      {/* Save/correction/error notices (spec §AC) — distinct, human-safe. */}
      {saving && <p className="mb-3 text-xs text-sky-300">Saving…</p>}
      {correctedNote !== null && (
        <p
          role="status"
          className="mb-3 rounded-lg border border-amber-900/60 bg-amber-950/30 px-3 py-2 text-xs text-amber-300"
        >
          Invalid value corrected — {correctedNote}
        </p>
      )}
      {error !== null && (
        <p
          role="alert"
          className="mb-3 flex items-center justify-between rounded-lg border border-red-900/60 bg-red-950/30 px-3 py-2 text-xs text-red-300"
        >
          <span>Failed to save: {error}</span>
          <button
            type="button"
            onClick={dismissNotices}
            className="ml-3 text-red-400 hover:text-red-200"
          >
            Dismiss
          </button>
        </p>
      )}

      <div className="space-y-4">
        <Section title="Discovery & Polling">
          <Row
            label="Auto Refresh"
            description="Automatically refresh local services, ports and readiness."
            htmlFor="setting-auto-refresh"
          >
            <Toggle
              checked={settings.autoRefresh}
              onChange={(value) => patch({ autoRefresh: value })}
              label="Auto Refresh"
            />
          </Row>
          <Row
            label="Refresh interval"
            description="Base discovery cadence in milliseconds (1000–60000). Manual refresh always works."
            htmlFor="setting-interval"
          >
            <input
              id="setting-interval"
              type="number"
              min={1000}
              max={60000}
              step={500}
              value={intervalText}
              onChange={(event) => setIntervalText(event.target.value)}
              onBlur={commitInterval}
              onKeyDown={(event) => {
                if (event.key === 'Enter') commitInterval()
              }}
              className="w-28 rounded-md border border-slate-700 bg-slate-900 px-2 py-1.5 text-xs text-slate-200 focus:outline-none focus-visible:ring-2 focus-visible:ring-sky-500"
            />
          </Row>
          <Row
            label="AI runtime polling"
            description="Periodically probe detected local AI runtimes. Manual refresh always works."
            htmlFor="setting-ai-polling"
          >
            <Toggle
              checked={settings.aiPollingEnabled}
              onChange={(value) => patch({ aiPollingEnabled: value })}
              label="AI runtime polling"
            />
          </Row>
          <Row
            label="Docker polling"
            description="Periodically refresh Docker engine/container state. Manual refresh always works."
            htmlFor="setting-docker-polling"
          >
            <Toggle
              checked={settings.dockerPollingEnabled}
              onChange={(value) => patch({ dockerPollingEnabled: value })}
              label="Docker polling"
            />
          </Row>
        </Section>

        <Section title="Window">
          <Row
            label="Close behavior"
            description="Exit closes LocalStack. Minimize to tray keeps it running in the notification area. Closing never stops your services."
          >
            <Select<CloseBehavior>
              id="setting-close-behavior"
              value={settings.closeBehavior}
              options={[
                { value: 'exit', label: 'Exit' },
                { value: 'minimize_to_tray', label: 'Minimize to tray' },
              ]}
              onChange={(value) => patch({ closeBehavior: value })}
            />
          </Row>
          <Row
            label="Launch minimized"
            description="Start with the main window hidden; reopen it from the tray icon."
            htmlFor="setting-launch-minimized"
          >
            <Toggle
              checked={settings.launchMinimized}
              onChange={(value) => patch({ launchMinimized: value })}
              label="Launch minimized"
            />
          </Row>
        </Section>

        <Section title="System">
          <Row
            label="Run at Windows startup"
            description={
              settings.runAtStartup !== settings.startupRegistered
                ? 'Syncing with Windows registration…'
                : settings.startupRegistered
                  ? 'Registered with Windows (current user).'
                  : 'Not registered with Windows.'
            }
            htmlFor="setting-run-at-startup"
          >
            <Toggle
              checked={settings.startupRegistered}
              onChange={(value) => void toggleStartup(value)}
              label="Run at Windows startup"
            />
          </Row>
          {/* Theme preference is deliberately NOT exposed (spec §12 defer
              rule): the app ships one dark palette, so a System/Light/Dark
              selector would be an inert, misleading control. The backend
              model reserves the field for post-v1.0 theming work. */}
        </Section>
      </div>

      <p className="mt-6 text-xs text-slate-600">
        Settings contain operational preferences only — never credentials.
      </p>
    </div>
  )
}
