import { defineConfig } from '@playwright/test';

const executablePath = process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH;

export default defineConfig({
  testDir: '.',
  testMatch: 'catalog.visual.spec.ts',
  fullyParallel: false,
  workers: 1,
  reporter: 'list',
  snapshotPathTemplate: '../__screenshots__/{projectName}/{arg}{ext}',
  expect: {
    toHaveScreenshot: {
      animations: 'disabled',
      caret: 'hide',
      scale: 'css',
      maxDiffPixelRatio: 0,
    },
  },
  use: {
    baseURL: 'http://127.0.0.1:4174',
    browserName: 'chromium',
    headless: true,
    locale: 'zh-CN',
    timezoneId: 'UTC',
    deviceScaleFactor: 1,
    launchOptions: executablePath ? { executablePath } : {},
  },
  projects: [
    {
      name: 'desktop-light',
      use: { viewport: { width: 1440, height: 900 }, colorScheme: 'light' },
    },
    {
      name: 'desktop-dark',
      use: { viewport: { width: 1440, height: 900 }, colorScheme: 'dark' },
    },
    {
      name: 'phone-light',
      use: { viewport: { width: 390, height: 844 }, colorScheme: 'light' },
    },
  ],
  webServer: {
    command: 'npx vite --host 127.0.0.1 --port 4174',
    url: 'http://127.0.0.1:4174/src/components/display/catalog/index.html?component=section',
    reuseExistingServer: !process.env.CI,
    timeout: 30_000,
  },
});
