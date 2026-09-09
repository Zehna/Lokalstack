/**
 * Test boundary for the native client — the single Tauri seam.
 *
 * Component/store tests never mock `@tauri-apps/api/core` directly; they
 * mock this module. `mockNativeClient()` installs a resolved mock of every
 * exported client function and returns the underlying `vi.fn`s keyed by
 * client-function name so tests can set return values per command.
 *
 * The map key is the *client function name* (e.g. `getPortListeners`), not
 * the Rust command name — tests stay readable and refactor-safe.
 */
import { vi } from 'vitest'

import * as native from '@/services/native/ports'

type NativeModule = typeof native

/** All client functions, mocked to resolve `undefined` until overridden. */
export type NativeMocks = { [K in keyof NativeModule]: ReturnType<typeof vi.fn> }

/** Install a full mock of the native client and return the keyed `vi.fn`s. */
export function mockNativeClient(): NativeMocks {
  const mocks = {} as Record<string, ReturnType<typeof vi.fn>>
  for (const key of Object.keys(native) as (keyof NativeModule)[]) {
    const fn = vi.fn().mockResolvedValue(undefined)
    mocks[key] = fn
    // `as never` bridges vitest's generic Mock type to the per-function
    // signature — safe because every consumer re-points it at valid returns
    // via `mockResolvedValue(...)` before use.
    vi.spyOn(native, key).mockImplementation(fn as never)
  }
  return mocks as NativeMocks
}

/** Remove all native-client spies (called after each test in stores tests). */
export function restoreNativeClient(): void {
  vi.restoreAllMocks()
}
