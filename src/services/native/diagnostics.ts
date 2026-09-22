/**
 * Native diagnostics client — Phase 11C (Task 15).
 *
 * One of the ONLY modules allowed to call Tauri `invoke` (see ports.ts for
 * the established boundary; safetyGuards.test.ts enforces it). Every
 * function wraps exactly one Task-14 backend command with its typed DTO.
 * Opaque IDs cross the bridge; no filesystem path is ever sent or received
 * as authority — exported file names are display-only text.
 */

import { invoke } from '@tauri-apps/api/core'

import type {
  BundleDetailDto,
  BundleMetaDto,
  DiagnosticsOverviewDto,
  ExportOutcomeDto,
  ExportProfileDto,
  GitHubIssueDraftDto,
  HealthCheckResultDto,
  IncidentDto,
  SupportSummaryDto,
  ViewId,
} from '@/types/domain'

/** Positional ViewId discriminant (NAV_ITEMS order, Rust NAV_LEN = 10). */
const VIEW_ID_POSITIONS: Record<ViewId, number> = {
  dashboard: 0,
  projects: 1,
  services: 2,
  ports: 3,
  workspaces: 4,
  'ai-services': 5,
  docker: 6,
  history: 7,
  settings: 8,
  diagnostics: 9,
}

export function getDiagnosticsOverview(): Promise<DiagnosticsOverviewDto> {
  return invoke<DiagnosticsOverviewDto>('get_diagnostics_overview')
}

export function runDeepHealthChecks(): Promise<HealthCheckResultDto[]> {
  return invoke<HealthCheckResultDto[]>('run_deep_health_checks')
}

export function listIncidents(): Promise<IncidentDto[]> {
  return invoke<IncidentDto[]>('list_incidents')
}

export function markIncidentReviewed(incidentId: string): Promise<void> {
  return invoke('mark_incident_reviewed', { incidentId })
}

export function listSupportBundles(): Promise<BundleMetaDto[]> {
  return invoke<BundleMetaDto[]>('list_support_bundles')
}

export function getSupportBundleDetail(bundleId: string): Promise<BundleDetailDto> {
  return invoke('get_support_bundle_detail', { bundleId })
}

export function exportSupportBundle(
  bundleId: string,
  profile: ExportProfileDto,
  confirmed: boolean,
): Promise<ExportOutcomeDto> {
  return invoke('export_support_bundle', { bundleId, profile, confirmed })
}

export function deleteSupportBundle(bundleId: string): Promise<void> {
  return invoke('delete_support_bundle', { bundleId })
}

export function openDiagnosticsFolder(): Promise<void> {
  return invoke('open_diagnostics_folder')
}

export function getSupportSummary(bundleId?: string): Promise<SupportSummaryDto> {
  return invoke('get_support_summary', { bundleId })
}

export function prepareLocalstackGithubIssue(bundleId?: string): Promise<GitHubIssueDraftDto> {
  return invoke('prepare_localstack_github_issue', { bundleId })
}

/**
 * Navigation-time only (spec §43-I): report the active view to the backend
 * cache for the panic hook. Fire-and-forget by contract — callers never
 * await and failures are swallowed upstream.
 */
export function updateDiagnosticsContext(view: ViewId): Promise<boolean> {
  return invoke('update_diagnostics_context', {
    viewId: VIEW_ID_POSITIONS[view],
  })
}

export function revealExportResult(exportId: string): Promise<void> {
  return invoke('reveal_export_result', { exportId })
}
