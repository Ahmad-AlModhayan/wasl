import { defineConfig } from 'vitest/config';

// Tauri expects a fixed port and fails if it is taken.
export default defineConfig({
  clearScreen: false,
  // kahf-design's public/ is the app's: its kahf.css loads IBM Plex Sans, Plex Sans Arabic and Plex Mono from /fonts/,
  // served from the app itself (the CSP stays font-src 'self').
  publicDir: 'node_modules/@kahf/design/public',
  server: { port: 1420, strictPort: true, watch: { ignored: ['**/src-tauri/**'] } },
  envPrefix: ['VITE_', 'TAURI_ENV_'],
  build: { target: 'es2022', outDir: 'dist', emptyOutDir: true },
  test: { environment: 'node', include: ['src/**/*.test.ts'] },
});
