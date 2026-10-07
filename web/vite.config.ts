import { defineConfig } from 'vitest/config'
import react from '@vitejs/plugin-react'
import { resolve } from 'node:path'
export default defineConfig({
  plugins: [react()],
  build: {
    rollupOptions: {
      input: {
        board: resolve(import.meta.dirname, 'index.html'),
        console: resolve(import.meta.dirname, 'console.html'),
      },
    },
  },
  test: { include: ['src/**/*.test.ts'] },
})
