// Local acceptance only (not CI) for AWR-OBS-021: run `npx vite build && node e2e/mobile-viewport-check.mjs`.
// At 375 and 768 px every main page must keep the document within the viewport and expose the
// navigation through the header entry: the drawer opens, holds every navigation item, navigates,
// closes on Escape and returns focus to the entry. Uses the server's demo fixtures.
// Set OBSERVER_URL to reuse a running built app, or CHROME_PATH to select a browser executable.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { existsSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { chromium } from '@playwright/test';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
let serverProcess;

async function observerUrl() {
  if (process.env.OBSERVER_URL) return process.env.OBSERVER_URL;
  serverProcess = spawn(process.execPath, ['server.js', '--demo', '--port', '0'], {
    cwd: root,
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  let output = '';
  return new Promise((resolve, reject) => {
    const timeout = setTimeout(() => reject(new Error('Observer server did not start')), 10_000);
    serverProcess.stdout.on('data', (chunk) => {
      output += String(chunk);
      const match = output.match(/URL\s+(http:\/\/127\.0\.0\.1:\d+)/);
      if (match) {
        clearTimeout(timeout);
        resolve(match[1]);
      }
    });
    serverProcess.stderr.on('data', (chunk) => {
      output += String(chunk);
    });
    serverProcess.once('exit', (code) => {
      clearTimeout(timeout);
      reject(new Error(`Observer server exited (${code}): ${output}`));
    });
  });
}

function chromePath() {
  const candidates = [
    process.env.CHROME_PATH,
    '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
    '/Applications/Chromium.app/Contents/MacOS/Chromium',
    '/usr/bin/google-chrome',
    '/usr/bin/chromium',
  ].filter(Boolean);
  return candidates.find((candidate) => existsSync(candidate));
}

// The demo fixtures serve no activity read (the page stays in its loading state), so activity is
// covered by tools/browser-check.sh against the real-shaped mock instead of here; the drawer
// behaviour under test is shared by the shell on every page.
const PAGES = [
  ['overview', '/projects/demo?tab=overview', '[data-testid="page-project"]'],
  ['plan', '/projects/demo?tab=plan', '[data-testid="page-project"]'],
  ['knowledge', '/projects/demo?tab=knowledge', '[data-testid="page-knowledge"]'],
  ['account', '/account', '[data-testid="page-account"]'],
];

const WIDTHS = [375, 768];

let browser;
try {
  const baseUrl = await observerUrl();
  const executablePath = chromePath();
  browser = await chromium.launch({ headless: true, ...(executablePath ? { executablePath } : {}) });
  for (const width of WIDTHS) {
    const context = await browser.newContext({ viewport: { width, height: width < 600 ? 812 : 900 } });
    await context.addInitScript(() => localStorage.setItem('i18nextLng', 'zh-CN'));
    for (const [name, pagePath, selector] of PAGES) {
      const page = await context.newPage();
      await page.goto(`${baseUrl}${pagePath}`);
      await page.waitForSelector(selector, { timeout: 20_000 });
      await page.waitForTimeout(500);

      const entry = page.getByRole('button', { name: '打开导航' });
      assert.ok(await entry.isVisible(), `${name} ${width}: the navigation entry must be visible`);
      assert.ok(await entry.boundingBox().then((box) => box.width >= 40 && box.height >= 40),
        `${name} ${width}: the navigation entry must keep a 40px tap area`);

      const metrics = await page.evaluate(() => ({
        scrollWidth: document.documentElement.scrollWidth,
        clientWidth: document.documentElement.clientWidth,
      }));
      assert.ok(metrics.scrollWidth <= metrics.clientWidth,
        `${name} ${width}: the page overflows horizontally: ${JSON.stringify(metrics)}`);

      // Open, choose a destination: the drawer closes and the route changes.
      await entry.click();
      const drawer = page.getByRole('dialog');
      await drawer.waitFor();
      const itemLabels = await drawer.getByRole('menuitem').allTextContents();
      assert.ok(itemLabels.length >= 9, `${name} ${width}: the drawer must hold every navigation item, saw ${JSON.stringify(itemLabels)}`);
      await drawer.getByRole('menuitem', { name: '活动与审计' }).click();
      await page.waitForFunction(() => !document.querySelector('.ant-drawer-open'));
      assert.ok(new URL(page.url()).pathname === '/activity',
        `${name} ${width}: choosing a destination must navigate, got ${page.url()}`);

      // Reopen, close with Escape: focus returns to the entry (the dialog restores it once the
      // closing motion has finished, so wait for the focus itself rather than a fixed delay).
      await entry.click();
      await drawer.waitFor();
      await page.keyboard.press('Escape');
      await page.waitForFunction(() => !document.querySelector('.ant-drawer-open'));
      await page.waitForFunction(
        () => document.activeElement?.getAttribute('data-testid') === 'nav-toggle'
          || document.activeElement?.getAttribute('aria-label') === '打开导航',
        null,
        { timeout: 3000 },
      );
      console.log(`mobile-viewport-check: ${name} ${width}px doc ${metrics.scrollWidth}/${metrics.clientWidth} nav ok`);
    }
    await context.close();
  }
} finally {
  if (browser) await browser.close();
  if (serverProcess) serverProcess.kill('SIGTERM');
}
