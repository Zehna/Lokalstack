/**
 * Presentation helpers for `ServiceIdentity` — badge classes, labels and
 * tooltips. Pure functions so they stay trivially testable.
 */

import type { Confidence, ServiceCategory, ServiceIdentity } from '@/types/domain'

/** Tailwind classes for the confidence badge, strongest → weakest. */
export function confidenceBadgeClass(confidence: Confidence): string {
  switch (confidence) {
    case 'exact':
      return 'border-emerald-900/60 bg-emerald-950/40 text-emerald-400'
    case 'high':
      return 'border-sky-900/60 bg-sky-950/40 text-sky-400'
    case 'medium':
      return 'border-amber-900/60 bg-amber-950/40 text-amber-400'
    default:
      return 'border-slate-800 bg-slate-900 text-slate-500'
  }
}

/** Human label for a confidence level, capitalized. */
export function confidenceLabel(confidence: Confidence): string {
  return confidence.charAt(0).toUpperCase() + confidence.slice(1)
}

/** Tooltip explaining what a confidence level means. */
export function confidenceTooltip(confidence: Confidence): string {
  switch (confidence) {
    case 'exact':
      return 'The executable itself identifies the service (e.g. postgres.exe).'
    case 'high':
      return 'Strong command-line or path evidence.'
    case 'medium':
      return 'Plausible but not conclusive evidence.'
    default:
      return 'Weak indication — may easily be wrong.'
  }
}

/** Tailwind classes for the category badge. */
export function categoryBadgeClass(category: ServiceCategory): string {
  switch (category) {
    case 'frontend':
      return 'border-violet-900/60 bg-violet-950/40 text-violet-300'
    case 'backend':
      return 'border-sky-900/60 bg-sky-950/40 text-sky-300'
    case 'database':
      return 'border-emerald-900/60 bg-emerald-950/40 text-emerald-300'
    case 'ai':
      return 'border-fuchsia-900/60 bg-fuchsia-950/40 text-fuchsia-300'
    case 'infrastructure':
      return 'border-amber-900/60 bg-amber-950/40 text-amber-300'
    default:
      return 'border-slate-800 bg-slate-900 text-slate-500'
  }
}

/** One-line evidence summary, e.g. `process_name: postgres.exe · command_line: vite`. */
export function evidenceSummary(identity: ServiceIdentity): string {
  return identity.evidence.map((e) => `${e.source}: ${e.value}`).join(' · ')
}
