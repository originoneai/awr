/// <reference types="vitest/config" />
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vitest/config';
import react from '@vitejs/plugin-react';

const here = path.dirname(fileURLToPath(import.meta.url));

// Single source of truth for the observation read contract: the UMD module in
// tools/observer/server (AWR-OBS-002) is aliased, not copied. It is CommonJS, so it
// must be pre-bundled in dev (optimizeDeps.include) and transformed by the
// commonjs plugin in build (commonjsOptions.include).
const contractPath = path.resolve(here, './server/observation-contract.js');
const progressPath = path.resolve(here, '../project-progress-overview/public');

export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: {
      'observation-contract': contractPath,
      'awr-progress-map': progressPath,
    },
  },
  optimizeDeps: {
    include: ['observation-contract', 'awr-progress-map/project-map/page.js', 'awr-progress-map/project-map/interact.js'],
  },
  build: {
    commonjsOptions: {
      include: [/observation-contract/, /project-progress-overview/, /node_modules/],
    },
  },
  server: {
    fs: {
      // Permit the workspace root for local development.
      allow: [here, path.resolve(here, '..')],
    },
    proxy: {
      '/api': 'http://127.0.0.1:7390',
    },
  },
  test: {
    include: ['src/**/*.test.ts', 'src/**/*.test.tsx'],
    environment: 'jsdom',
    globals: true,
    setupFiles: ['./src/test-setup.ts'],
    css: false,
  },
});
