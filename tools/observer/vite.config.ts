/// <reference types="vitest/config" />
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vitest/config';
import react from '@vitejs/plugin-react';

const here = path.dirname(fileURLToPath(import.meta.url));

// Single source of truth for the observation read contract: the UMD module in
// tools/inspector (AWR-OBS-002) is aliased, not copied. It is CommonJS, so it
// must be pre-bundled in dev (optimizeDeps.include) and transformed by the
// commonjs plugin in build (commonjsOptions.include).
const contractPath = path.resolve(here, '../inspector/observation-contract.js');

export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: {
      'observation-contract': contractPath,
    },
  },
  optimizeDeps: {
    include: ['observation-contract'],
  },
  build: {
    commonjsOptions: {
      include: [/observation-contract/, /node_modules/],
    },
  },
  server: {
    fs: {
      // The contract lives outside this package root.
      allow: [here, path.resolve(here, '..')],
    },
    proxy: {
      '/api': 'http://127.0.0.1:7390',
    },
  },
  test: {
    environment: 'jsdom',
    globals: true,
    setupFiles: ['./src/test-setup.ts'],
    css: false,
  },
});
