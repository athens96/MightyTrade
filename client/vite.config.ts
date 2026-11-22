import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import pkg from './package.json' with { type: 'json' }

// Dev: /api is proxied to Flask so the browser sees one origin (cookies stay SameSite=Strict).
// Prod: nginx does the same (see deploy/nginx.conf).
export default defineConfig({
  plugins: [react()],
  define: {
    __APP_VERSION__: JSON.stringify(pkg.version),
  },
  // Main chunk is dominated by lightweight-charts (~160 kB gzip total); quant page is split out.
  build: { chunkSizeWarningLimit: 600 },
  server: {
    port: 5173,
    proxy: { '/api': 'http://127.0.0.1:5001' },
  },
})
