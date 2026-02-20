import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'

// The freeq server's --web-addr (HTTP/WebSocket listener)
const FREEQ_WEB = process.env.FREEQ_WEB || 'http://localhost:8080'

export default defineConfig({
  plugins: [react(), tailwindcss()],
  server: {
    proxy: {
      '/irc': {
        target: FREEQ_WEB.replace('http', 'ws'),
        ws: true,
      },
      '/api': {
        target: FREEQ_WEB,
      },
      '/auth': {
        target: FREEQ_WEB,
      },
    },
  },
})
