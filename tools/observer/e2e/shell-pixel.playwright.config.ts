import { defineConfig } from '@playwright/test';

// AWR-PPO-066: shell pixel parity against the prototype, measured on the demo server.
// Run from tools/observer: npx playwright test -c e2e/shell-pixel.playwright.config.ts
export default defineConfig({
  testDir: '.',
  testMatch: 'shell-pixel.spec.ts',
  fullyParallel: false,
  workers: 1,
  reporter: 'list',
  use: {
    browserName: 'chromium',
    headless: true,
    locale: 'zh-CN',
    timezoneId: 'UTC',
    deviceScaleFactor: 1,
    viewport: { width: 1440, height: 900 },
  },
  projects: [
    { name: 'desktop-light', use: { colorScheme: 'light' } },
    { name: 'desktop-dark', use: { colorScheme: 'dark' } },
  ],
});
