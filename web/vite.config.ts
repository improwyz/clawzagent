import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import tailwindcss from '@tailwindcss/vite';

const gatewayProxy = {
  '/api': { target: 'http://127.0.0.1:3000', changeOrigin: true },
  '/ws': { target: 'http://127.0.0.1:3000', ws: true },
};

export default defineConfig({
  plugins: [react(), tailwindcss()],
  server: {
    port: 5173,
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
  },
});