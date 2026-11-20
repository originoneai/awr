import { defineConfig } from '@playwright/test';

export default defineConfig({
  testDir: '.',
  testMatch: 'brand-logo.spec.ts',
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
