/// <reference types="vitest/config" />
import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import { execSync } from 'child_process'
import { fileURLToPath } from 'node:url'
import path from 'node:path'

// Vitest config: mirrors the test section of vite.config.ts but intentionally
// omits @tailwindcss/vite. The tailwind plugin loads a native binary that npm
// skips on unsupported engines (node < 20), which crashes config loading even
// when tailwind output is irrelevant to unit tests. The production build still
// uses vite.config.ts with tailwind included.
//
// Node 18 compatibility: jsdom v29 has two incompatibilities with node 18:
//   • html-encoding-sniffer CJS-requires @exodus/bytes/encoding-lite.js (pure ESM)
//   • webidl-conversions uses ArrayBuffer.prototype.resizable / SharedArrayBuffer
//     .prototype.growable which don't exist in node 18
// We fix both by loading scripts/esm-shim-require.cjs via NODE_OPTIONS --require
// before any fork worker starts. That file polyfills the missing properties and
// redirects the offending require() to a CJS-compatible shim.
const __dirname = path.dirname(fileURLToPath(import.meta.url))

const GIT_COMMIT = process.env.GIT_COMMIT || (() => {
  try { return execSync('git rev-parse --short HEAD').toString().trim() }
  catch { return 'unknown' }
})()

const SHIM = path.resolve(__dirname, 'scripts/esm-shim-require.cjs')

export default defineConfig({
  plugins: [react()],
  define: {
    '__FREEQ_TARGET__': JSON.stringify('http://127.0.0.1:8080'),
    '__GIT_COMMIT__': JSON.stringify(GIT_COMMIT),
  },
  resolve: {
    alias: {
      '@freeq/sdk': path.resolve(__dirname, '../freeq-sdk-js/src/index.ts'),
    },
  },
  test: {
    environment: 'node',
    include: ['src/**/*.test.{ts,tsx}'],
    setupFiles: ['./src/test-setup.ts'],
    // Inject the node 18 shim into every fork worker via NODE_OPTIONS.
    // vitest.test.env overrides are merged with the worker's environment
    // before the fork is spawned, so --require runs before any module loading.
    env: {
      NODE_OPTIONS: `--require ${SHIM}`,
    },
  },
})
