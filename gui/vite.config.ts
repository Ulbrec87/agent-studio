import { resolve } from 'node:path'
import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

// Tauri expects a fixed dev port (see crates/as-gui/tauri.conf.json `devUrl`).
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  resolve: {
    alias: {
      '@shared': resolve(__dirname, 'shared'),
      '@': resolve(__dirname, 'src')
    }
  },
  server: {
    port: 1420,
    strictPort: true,
    watch: { ignored: ['**/crates/as-gui/**'] }
  },
  build: {
    // Monaco and xterm are large; the app is loaded from disk, so chunk size is not a concern.
    chunkSizeWarningLimit: 6000
  }
})
