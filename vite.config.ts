import { fileURLToPath, URL } from 'node:url'
import { readFileSync } from 'node:fs'

import tailwindcss from '@tailwindcss/vite'
import react from '@vitejs/plugin-react'
import { defineConfig } from 'vite'

// The Tauri version from tauri.conf.json — exposed to the app as
// `import.meta.env.VITE_APP_VERSION` so the UI never hardcodes a version.
const tauriConf = JSON.parse(
  readFileSync(fileURLToPath(new URL('./src-tauri/tauri.conf.json', import.meta.url)), 'utf-8'),
) as { version: string }

// Tauri expects a fixed dev port — fail loudly instead of drifting.
export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: {
      '@': fileURLToPath(new URL('./src', import.meta.url)),
    },
  },
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
  },
  define: {
    'import.meta.env.VITE_APP_VERSION': JSON.stringify(tauriConf.version),
  },
})
