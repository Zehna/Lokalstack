/**
 * Diagnostics feature (Phase 11C Task 17) — shared UI-local types.
 * UI-only: these never cross the Tauri boundary (wire DTOs live in
 * src/types/domain.ts).
 */
import type { ExportProfileDto } from '@/types/domain'

/** Tab identifiers of the Diagnostics page. */
export type DiagnosticsTab = 'overview' | 'health' | 'incidents' | 'bundles'

/** Labels for the three privacy profiles (spec §12). */
export const EXPORT_PROFILE_LABELS: Record<ExportProfileDto, string> = {
  'safe-share': 'Safe Share',
  'developer-detail': 'Developer Detail',
  'full-forensics': 'Full Forensics',
}

/** Sensitive identity categories listed in the Full Forensics warning. */
export const FULL_FORENSICS_CATEGORIES = [
  'Windows username',
  'Hostname',
  'Local and private IP addresses',
  'MAC addresses',
  'Windows SID',
  'Machine identifier (MachineGuid)',
  'Full local user and project paths',
]
