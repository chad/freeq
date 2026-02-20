import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'

// The freeq server's --web-addr (HTTP/WebSocket listener)
const FREEQ_WEB = process.env.FREEQ_WEB || 'http://127.0.0.1:8080'

export default defineConfig({
  plugins: [react(), tailwindcss()],
  server: {
    host: '127.0.0.1',
    proxy: {
      '/irc': {
        target: FREEQ_WEB,
        ws: true,
        changeOrigin: false, // preserve browser Host header
      },
      '/api': {
        target: FREEQ_WEB,
        changeOrigin: false,
      },
      '/auth': {
        target: FREEQ_WEB,
        changeOrigin: false, // server needs browser Host for redirect_uri
      },
    },
  },
})
