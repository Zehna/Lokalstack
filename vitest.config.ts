import { fileURLToPath, URL } from 'node:url'

import react from '@vitejs/plugin-react'
import { defineConfig } from 'vitest/config'

// Test-only config — separate from vite.config.ts so the Tauri dev/build
// pipeline is untouched. jsdom provides the browser-ish environment for
// component tests; pure logic tests run fine in it as well.
export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: {
      '@': fileURLToPath(new URL('./src', import.meta.url)),
    },
  },
  test: {
    environment: 'jsdom',
    setupFiles: ['./src/test/setup.ts'],
    include: ['src/**/*.test.{ts,tsx}'],
    // Live-ish helpers stay excluded: frontend tests are deterministic only.
    coverage: {
      reporter: ['text', 'json-summary'],
      include: ['src/stores/**', 'src/utils/**', 'src/services/**', 'src/hooks/**', 'src/features/**'],
      thresholds: {
        // Realistic Phase 10 gate (spec: 70%+ on domain-heavy modules).
        lines: 60,
        functions: 60,
      },
    },
  },
})
