import { defineConfig } from '@playwright/test';

/** Real built-page acceptance against server.js --demo; Playwright owns Chromium. */
export default defineConfig({
  testDir: './e2e',
  testMatch: /workbench\.(?:metrics|visual)\.spec\.ts/,
  fullyParallel: false,
  workers: 1,
  reporter: 'list',
  timeout: 60_000,
  expect: { timeout: 20_000 },
  use: {
    baseURL: 'http://127.0.0.1:4175',
    browserName: 'chromium',
    headless: true,
    locale: 'zh-CN',
    timezoneId: 'UTC',
    deviceScaleFactor: 1,
    screenshot: 'only-on-failure',
  },
  projects: [
    { name: 'desktop-light', use: { viewport: { width: 1440, height: 900 }, colorScheme: 'light' } },
    { name: 'desktop-dark', use: { viewport: { width: 1440, height: 900 }, colorScheme: 'dark' } },
    { name: 'phone-light', use: { viewport: { width: 390, height: 844 }, colorScheme: 'light' } },
    { name: 'desktop-new-york', use: { viewport: { width: 1440, height: 900 }, colorScheme: 'light', timezoneId: 'America/New_York' } },
  ],
  webServer: {
    command: 'node server.js --demo --demo-variant full --port 4175',
    url: 'http://127.0.0.1:4175/workbench?scope=P-001',
    reuseExistingServer: false,
    timeout: 60_000,
  },
});
