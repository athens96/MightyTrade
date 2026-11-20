import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

// Dev: /api is proxied to Flask so the browser sees one origin (cookies stay SameSite=Strict).
// Prod: nginx does the same (see deploy/nginx.conf).
export default defineConfig({
  plugins: [react()],
  server: {
    port: 5173,
    proxy: { '/api': 'http://127.0.0.1:5000' },
  },
})
