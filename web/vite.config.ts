import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import tailwindcss from '@tailwindcss/vite';

const gatewayProxy = {
  '/api': { target: 'http://127.0.0.1:3000', changeOrigin: true },
  '/ws': { target: 'http://127.0.0.1:3000', ws: true },
};

/** Tauri build/dev sets TAURI_ENV_*; use relative assets in packaged app. */
const isTauri = Boolean(process.env.TAURI_ENV_PLATFORM);

export default defineConfig({
  plugins: [react(), tailwindcss()],
  base: isTauri ? './' : '/',
  server: {
    port: 5173,
    strictPort: true,
    proxy: gatewayProxy,
  },
  preview: {
    host: '0.0.0.0',
    port: 4173,
    strictPort: true,
    proxy: gatewayProxy,
  },
  build: {
    outDir: 'dist',
    emptyOutDir: true,
  },
});
