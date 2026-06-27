import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'

// During `npm run dev` the Vite dev server proxies API calls to the Rust
// backend. In production the built assets are embedded into the binary and
// served from the same origin, so no proxy is needed.
export default defineConfig({
  plugins: [vue()],
  server: {
    port: 5173,
    proxy: {
      '/api': 'http://localhost:8080'
    }
  },
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    chunkSizeWarningLimit: 1024
  }
})
