/// <reference types="vitest/config" />
import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'
import { execSync } from 'child_process'

// The freeq server's --web-addr (HTTP/WebSocket listener)
const FREEQ_WEB = process.env.FREEQ_WEB || 'http://127.0.0.1:8080'
const GIT_COMMIT = process.env.GIT_COMMIT || (() => {
  try { return execSync('git rev-parse --short HEAD').toString().trim() }
  catch { return 'unknown' }
})()

export default defineConfig({
  plugins: [react(), tailwindcss()],
  define: {
    '__FREEQ_TARGET__': JSON.stringify(FREEQ_WEB),
    '__GIT_COMMIT__': JSON.stringify(GIT_COMMIT),
  },
  test: {
    environment: 'node',
    include: ['src/**/*.test.{ts,tsx}'],
  },
  server: {
    host: '127.0.0.1',
    proxy: {
      '/irc': {
        target: FREEQ_WEB,
        ws: true,
        changeOrigin: false, // preserve browser Host so server builds localhost redirect URIs
      },
      '/api': {
        target: FREEQ_WEB,
        changeOrigin: false,
      },
      '/auth': {
        target: FREEQ_WEB,
        changeOrigin: false,
      },
      '/av': {
        target: FREEQ_WEB,
        ws: true,
        changeOrigin: false,
      },
    },
  },
})
