/**
 * Phase 10B (spec §AC) — frontend static security guards.
 *
 * Scans production frontend source for forbidden control patterns. The
 * frontend must never control a raw PID, submit arbitrary launch commands,
 * or wrap `invoke` generically — it sends opaque ids issued by the backend.
 *
 * Comments and string literals are stripped before scanning: a forbidden
 * call is code, not documentation.
 */
import { readFileSync, readdirSync, statSync } from 'node:fs'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'

const FORBIDDEN: Array<[string, string]> = [
  // endProcess must be called with an opaque target id — never a pid.
  ['endProcess(pid', 'raw pid passed to endProcess — opaque target id required'],
  ['endProcess(Number(', 'numeric coercion into endProcess — opaque target id required'],
  ['endProcess(listener.pid', 'raw pid passed to endProcess — opaque target id required'],
  ['invoke(', 'raw Tauri invoke belongs only in the native client module'],
  ['new LaunchSpec(', 'frontend may never construct launch specs'],
  ['{ program:', 'frontend may never submit a program path to the backend'],
  ['cmd /c', 'shell interpretation is forbidden in the frontend'],
]

/** Directories excluded (test code legitimately quotes patterns). */
const EXCLUDED_DIRS = new Set(['test', 'node_modules'])

function listFiles(dir: string, out: string[] = []): string[] {
  for (const entry of readdirSync(dir)) {
    const full = join(dir, entry)
    const stat = statSync(full)
    if (stat.isDirectory()) {
      if (!EXCLUDED_DIRS.has(entry)) listFiles(full, out)
    } else if (/\.tsx?$/.test(entry) && !/\.test\.tsx?$/.test(entry)) {
      out.push(full)
    }
  }
  return out
}

function stripCommentsAndStrings(source: string): string {
  let out = ''
  let i = 0
  let inBlockComment = false
  while (i < source.length) {
    if (inBlockComment) {
      if (source[i] === '*' && source[i + 1] === '/') {
        inBlockComment = false
        out += ' '
        i += 2
      } else {
        if (source[i] === '\n') out += '\n'
        i += 1
      }
      continue
    }
    if (source[i] === '/' && source[i + 1] === '/') {
      while (i < source.length && source[i] !== '\n') i += 1
      continue
    }
    if (source[i] === '/' && source[i + 1] === '*') {
      inBlockComment = true
      out += ' '
      i += 2
      continue
    }
    if (source[i] === '"' || source[i] === "'" || source[i] === '`') {
      const quote = source[i]
      i += 1
      while (i < source.length && source[i] !== quote) {
        if (source[i] === '\\') i += 1
        i += 1
      }
      i += 1
      out += ' '
      continue
    }
    out += source[i]
    i += 1
  }
  return out
}

describe('frontend static safety guards (Phase 10B §AC)', () => {
  const srcRoot = join(__dirname)
  const files = listFiles(srcRoot)

  it('covers the whole production frontend tree', () => {
    expect(files.length).toBeGreaterThan(30)
  })

  it('has no raw-Tauri or arbitrary-control patterns outside the native client', () => {
    for (const file of files) {
      // The native client module is the single trusted seam — it *defines*
      // the opaque-id API surface. Every other file may only consume it.
      const isNativeClient = file.replace(/\\/g, '/').includes('services/native/')
      if (isNativeClient) continue
      const code = stripCommentsAndStrings(readFileSync(file, 'utf-8'))
      for (const [pattern, reason] of FORBIDDEN) {
        expect(
          code.includes(pattern),
          `${file} contains forbidden pattern ${JSON.stringify(pattern)} — ${reason}`,
        ).toBe(false)
      }
    }
  })
})
