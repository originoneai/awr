import { spawn, type ChildProcess } from 'node:child_process';
import { mkdir, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { expect, test } from '@playwright/test';

// This specification is run from tools/observer with the sibling Playwright config.
const root = process.cwd();
const evidence = path.resolve(process.env.OBSERVER_EVIDENCE_DIR ?? path.join(root, '../../.local/ppo-002b/browser'));
let server: ChildProcess | undefined;
let baseUrl = '';

test.beforeAll(async () => {
  await mkdir(evidence, { recursive: true });
  server = spawn(process.execPath, ['server.js', '--demo', '--demo-variant', 'full', '--port', '0'], {
    cwd: root,
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  let output = '';
  baseUrl = await new Promise<string>((resolve, reject) => {
    const timeout = setTimeout(() => reject(new Error(`Observer did not start: ${output}`)), 10_000);
    server?.stdout?.on('data', (chunk) => {
      output += String(chunk);
      const match = output.match(/URL\s+(http:\/\/127\.0\.0\.1:\d+)/);
      if (match) {
        clearTimeout(timeout);
        resolve(match[1]);
      }
    });
    server?.stderr?.on('data', (chunk) => { output += String(chunk); });
    server?.once('exit', (code) => {
      clearTimeout(timeout);
      reject(new Error(`Observer exited (${code}): ${output}`));
    });
  });
});

test.afterAll(async () => {
  if (!server || server.killed) return;
  const stopped = new Promise((resolve) => server?.once('exit', resolve));
  server.kill('SIGTERM');
  await stopped;
});

test.beforeEach(async ({ page }, testInfo) => {
  const mode = testInfo.project.name === 'desktop-dark' ? 'dark' : 'light';
  await page.addInitScript((themeMode) => {
    window.localStorage.setItem('awr-observer-theme', themeMode);
    window.localStorage.setItem('i18nextLng', 'zh-CN');
  }, mode);
});

test('owner brand mark matches the prototype brand geometry', async ({ page }, testInfo) => {
  const mode = testInfo.project.name === 'desktop-dark' ? 'dark' : 'light';
  await page.goto(`${baseUrl}/workbench?scope=P-001`);
  const brand = page.locator('.shell-sider').getByTestId('awr-brand');
  await brand.waitFor();
  const color = brand.getByTestId('brand-mark-color');
  const white = brand.getByTestId('brand-mark-white');
  const visible = mode === 'light' ? color : white;
  const hidden = mode === 'light' ? white : color;
  // A flex item's inline-block display is blockified in Chromium; visibility is the metric here.
  await expect(visible).toBeVisible();
  await expect(hidden).toHaveCSS('display', 'none');
  await visible.evaluate((image: HTMLImageElement) => image.decode());
  await page.evaluate(() => document.fonts.ready);

  // Prototype .brand: height 32px, align-items center, gap 10px. Prototype .brand b:
  // 17px/600. The owner's replacement mark is 24px wide (24 × 225 / 288 = 18.75px high).
  const metrics = await brand.evaluate((element) => {
    const images = [...element.querySelectorAll('img')];
    const mark = images.find((image) => getComputedStyle(image).display !== 'none');
    const wordmark = element.querySelector('[data-testid="brand-wordmark"]');
    if (!mark || !wordmark) throw new Error('The visible brand mark or wordmark is missing');
    const markBox = mark.getBoundingClientRect();
    const wordmarkBox = wordmark.getBoundingClientRect();
    const wordmarkStyles = getComputedStyle(wordmark);
    return {
      markWidth: markBox.width,
      markHeight: markBox.height,
      markToWordmarkGap: wordmarkBox.left - markBox.right,
      verticalCenterDelta: Math.abs(
        (markBox.top + markBox.height / 2) - (wordmarkBox.top + wordmarkBox.height / 2),
      ),
      wordmarkFontSize: wordmarkStyles.fontSize,
      wordmarkFontWeight: wordmarkStyles.fontWeight,
      currentSrc: (mark as HTMLImageElement).currentSrc,
      naturalWidth: (mark as HTMLImageElement).naturalWidth,
    };
  });
  expect(metrics.markWidth).toBe(24);
  expect(metrics.markHeight).toBe(18.75);
  expect(metrics.markToWordmarkGap).toBe(10);
  expect(metrics.verticalCenterDelta).toBeLessThanOrEqual(0.5);
  expect(metrics.wordmarkFontSize).toBe('17px');
  expect(metrics.wordmarkFontWeight).toBe('600');
  expect(metrics.currentSrc).toBe(`${baseUrl}/awr-logo-${mode === 'light' ? 'color' : 'white'}-72.png`);
  expect(metrics.naturalWidth).toBe(72);

  await page.waitForFunction(() => document.querySelectorAll('.ant-spin-spinning').length === 0);
  await page.screenshot({
    path: path.join(evidence, `shell-${mode}-1440x900.png`),
    animations: 'disabled',
  });
  await writeFile(path.join(evidence, `brand-metrics-${mode}.json`), `${JSON.stringify(metrics, null, 2)}\n`);
  console.log(`brand-logo ${mode}: ${JSON.stringify(metrics)}`);
});
