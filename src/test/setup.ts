/**
 * Vitest setup: jest-dom matchers. The Tauri `invoke` mock is NOT global —
 * each test file opts in via `mockNativeClient` (see ./nativeMock.ts) so
 * every test declares exactly which commands it answers.
 */
import '@testing-library/jest-dom/vitest'
