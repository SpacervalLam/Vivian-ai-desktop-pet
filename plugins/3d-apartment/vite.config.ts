import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import { fileURLToPath } from 'node:url';
const pluginRoot = fileURLToPath(new URL('./', import.meta.url));

/** Bundle the apartment and its runtime dependencies as one installable plugin script. */
export default defineConfig({
  plugins: [react()],
  define: { 'process.env.NODE_ENV': JSON.stringify('production') },
  build: {
    target: 'es2022',
    copyPublicDir: false,
    emptyOutDir: false,
    outDir: `${pluginRoot}/ui`,
    lib: {
      entry: `${pluginRoot}/src/apartmentPlugin.tsx`,
      name: 'VivianApartmentBundle',
      formats: ['iife'],
      fileName: () => 'room.js',
    },
    rollupOptions: {
      output: { inlineDynamicImports: true },
    },
  },
});
