import { Component, type ErrorInfo, type ReactNode } from 'react'

/**
 * Phase 10B (spec §K): application-level render guard.
 *
 * A rendering exception in one view must never blank the whole WebView.
 * The boundary keeps the app shell (sidebar, navigation) mounted, shows a
 * recoverable error surface without any raw stack trace, and offers:
 *  - Retry View      — remounts the failed view (resetting internal state)
 *  - Go to Dashboard — calls `onNavigateDashboard` so the shell switches view
 *
 * Reset safety: navigation away (resetKey change) clears the captured error;
 * retry only happens on explicit user action, so a deterministically throwing
 * view cannot create an infinite render loop.
 */
interface ErrorBoundaryProps {
  /** Changing this remounts/clears the error (view navigation). */
  resetKey?: string
  /** Shell callback: navigate back to the Dashboard view. */
  onNavigateDashboard?: () => void
  /** Shown instead of the subtree once an error is captured. */
  children: ReactNode
}

interface ErrorBoundaryState {
  error: Error | null
}

export class ErrorBoundary extends Component<ErrorBoundaryProps, ErrorBoundaryState> {
  state: ErrorBoundaryState = { error: null }

  static getDerivedStateFromError(error: Error): ErrorBoundaryState {
    return { error }
  }

  componentDidCatch(error: Error, info: ErrorInfo): void {
    // Local development detail only — never rendered to the user, never sent
    // anywhere (spec §W: local-only diagnostics, no telemetry).
    console.error('[ErrorBoundary] view render failed:', error.message, info.componentStack)
  }

  componentDidUpdate(prevProps: ErrorBoundaryProps): void {
    // Navigating to a different view clears the error so the new view gets a
    // fresh chance to render.
    if (prevProps.resetKey !== this.props.resetKey && this.state.error !== null) {
      this.setState({ error: null })
    }
  }

  private readonly retry = (): void => {
    this.setState({ error: null })
  }

  render(): ReactNode {
    const { error } = this.state
    if (error === null) {
      return this.props.children
    }

    return (
      <div
        role="alert"
        className="flex h-full flex-col items-center justify-center gap-4 p-8 text-center"
      >
        <h2 className="text-lg font-semibold">Something went wrong in this view.</h2>
        <p className="max-w-md text-sm opacity-70">
          The view could not be rendered. Your data is safe — retry the view or return to the
          Dashboard.
        </p>
        <div className="flex gap-3">
          <button
            type="button"
            onClick={this.retry}
            className="rounded-md border border-slate-700 bg-slate-900 px-3 py-1.5 text-sm font-medium text-slate-200 transition-colors hover:border-slate-600 hover:bg-slate-800"
          >
            Retry View
          </button>
          <button
            type="button"
            onClick={this.props.onNavigateDashboard}
            className="rounded-md border border-slate-700 px-3 py-1.5 text-sm font-medium text-slate-300 transition-colors hover:border-slate-600 hover:bg-slate-800"
          >
            Go to Dashboard
          </button>
        </div>
      </div>
    )
  }
}
