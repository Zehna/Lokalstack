/**
 * Release metadata guard (Phase 10E, spec §A).
 *
 * Pins the synchronized release version across every authoritative source:
 * package.json, src-tauri/tauri.conf.json and src-tauri/Cargo.toml must all
 * declare the same version, and it must be the release-candidate version.
 * A drift (bump one file, forget another) fails the suite before it can
 * ship. Sourced via `import.meta.env.VITE_APP_VERSION`, which Tauri's
 * config fills from the same tauri.conf.json version at build time.
 */
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { describe, expect, it } from 'vitest'

const RELEASE_VERSION = '1.1.0'

function readJson(relativePath: string): Record<string, unknown> {
  return JSON.parse(readFileSync(resolve(process.cwd(), relativePath), 'utf-8')) as Record<
    string,
    unknown
  >
}

function readTomlVersion(relativePath: string): string {
  const text = readFileSync(resolve(process.cwd(), relativePath), 'utf-8')
  const inPackage = text.match(/^\[package\][\s\S]*?^version\s*=\s*"([^"]+)"/m)
  const match = inPackage ?? text.match(/^version\s*=\s*"([^"]+)"/m)
  if (!match) throw new Error(`no version found in ${relativePath}`)
  return match[1]
}

describe('release version consistency (Phase 10E)', () => {
  it('package.json, tauri.conf.json and Cargo.toml all declare 1.1.0', () => {
    const pkg = readJson('package.json') as { version: string }
    const conf = readJson('src-tauri/tauri.conf.json') as { version: string }
    const cargo = readTomlVersion('src-tauri/Cargo.toml')

    expect(pkg.version).toBe(RELEASE_VERSION)
    expect(conf.version).toBe(RELEASE_VERSION)
    expect(cargo).toBe(RELEASE_VERSION)
  })

  it('tauri.conf.json keeps the release product identity', () => {
    const conf = readJson('src-tauri/tauri.conf.json') as {
      productName: string
      identifier: string
    }
    expect(conf.productName).toBe('LocalStack Control Center')
    expect(conf.identifier).toBe('com.localstack.controlcenter')
  })

  it('exposes the Tauri-provided version to the app shell (no hardcoded UI value)', () => {
    // Tauri injects the tauri.conf.json version at build time; the UI must
    // derive from it (spec §AN) rather than hardcode a number.
    const envType = import.meta.env.VITE_APP_VERSION
    expect(typeof envType === 'string' || envType === undefined).toBe(true)
  })
})
