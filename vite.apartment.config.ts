import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import { resolve } from 'node:path';

/** Bundle the apartment and its runtime dependencies as one installable plugin script. */
export default defineConfig({
  plugins: [react()],
  build: {
    target: 'es2022',
    copyPublicDir: false,
    emptyOutDir: false,
    outDir: resolve('src-tauri/plugins/3d-apartment/ui'),
    lib: {
      entry: resolve('src/components/room/apartmentPlugin.tsx'),
      name: 'VivianApartmentBundle',
      formats: ['iife'],
      fileName: () => 'room.js',
    },
    rollupOptions: {
      output: { inlineDynamicImports: true },
    },
  },
});
