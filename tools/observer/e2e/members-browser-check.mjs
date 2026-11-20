// Run against a production build and an existing demo/prototype server.
// Evidence stays outside Git; Playwright always uses its managed Chromium.
import assert from 'node:assert/strict';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { chromium } from '@playwright/test';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const appBase = process.env.APP_BASE ?? 'http://127.0.0.1:4592';
const protoBase = process.env.PROTO_BASE ?? 'http://127.0.0.1:4588/panorama.html';
const evidence = process.env.OBSERVER_EVIDENCE_DIR ?? path.join(root, '../../.local/ppo-056/browser');
const liveSamples = process.env.LIVE_SAMPLES_DIR;
await mkdir(evidence, { recursive: true });

const browser = await chromium.launch({ headless: true });
const report = { basis: 'Production build, real demo API, managed Chromium', configurations: [], interactions: [], liveSamples: [] };

function channels(value) {
  const rgb = value.match(/rgba?\(([^)]+)\)/);
  if (rgb) return rgb[1].split(/[ ,/]+/).filter(Boolean).slice(0, 3).map(Number);
  const modern = value.match(/color\(srgb\s+([\d.]+)\s+([\d.]+)\s+([\d.]+)/);
  if (modern) return modern.slice(1, 4).map((part) => Math.round(Number(part) * 255));
  return null;
}

async function contextFor(width, height, scheme, language = 'zh-CN') {
  const context = await browser.newContext({ viewport: { width, height }, colorScheme: scheme, locale: language });
  await context.addInitScript(({ scheme, language }) => {
    localStorage.setItem('awr-observer-theme', scheme);
    localStorage.setItem('awr-proto-theme', scheme);
    localStorage.setItem('i18nextLng', language);
  }, { scheme, language });
  return context;
}

async function settle(page, ready) {
  await page.locator(ready).first().waitFor();
  await page.evaluate(() => document.fonts.ready);
  await page.waitForTimeout(400);
}

try {
  for (const [name, width, height, scheme, expectedMuted] of [
    ['1440-light', 1440, 900, 'light', [107, 116, 135]],
    ['1440-dark', 1440, 900, 'dark', [122, 130, 150]],
    ['390-light', 390, 844, 'light', [107, 116, 135]],
  ]) {
    const context = await contextFor(width, height, scheme);
    const app = await context.newPage();
    const proto = await context.newPage();
    const errors = [];
    app.on('pageerror', (error) => errors.push(error.message));
    await app.goto(`${appBase}/members?scope=P-001`);
    await settle(app, '[data-testid="member-table"]');
    await proto.goto(`${protoBase}#p=members`);
    await settle(proto, '.block table.t');
    const appShot = path.join(evidence, `${name}-app.png`);
    const protoShot = path.join(evidence, `${name}-prototype.png`);
    await app.screenshot({ path: appShot, fullPage: true });
    await proto.screenshot({ path: protoShot, fullPage: true });
    const metric = await app.evaluate(() => {
      const selectors = ['.obs-page-subtitle', '.members-section .obs-section-count', '.members-table thead th', '.members-pnote'];
      return {
        text: document.body.innerText,
        width: document.documentElement.clientWidth,
        scrollWidth: document.documentElement.scrollWidth,
        colors: selectors.map((selector) => ({ selector, value: getComputedStyle(document.querySelector(selector)).color })),
        headers: [...document.querySelectorAll('.members-table thead th')].map((element) => element.textContent).filter(Boolean),
        rows: document.querySelectorAll('.members-table tbody tr.ant-table-row').length,
      };
    });
    assert.equal(metric.scrollWidth, metric.width, `${name}: horizontal overflow`);
    assert.deepEqual(errors, [], `${name}: browser errors`);
    assert.doesNotMatch(metric.text, /\b(?:undefined|null|NaN|personId|agentActive|connection_id|updated_at_ms)\b/);
    for (const color of metric.colors) assert.deepEqual(channels(color.value), expectedMuted, `${name}: ${color.selector}`);
    if (width === 390) assert.deepEqual(metric.headers, ['成员', '小组 · 职责', '现在在做', '等本人处理的']);
    report.configurations.push({ name, appShot, protoShot, ...metric, colors: metric.colors.map((color) => ({ ...color, channels: channels(color.value) })) });
    await context.close();
  }

  const context = await contextFor(1440, 900, 'light');
  const page = await context.newPage();
  await page.goto(`${appBase}/members?scope=P-001`);
  await settle(page, '[data-testid="member-table"]');
  const search = page.getByRole('textbox', { name: '搜索成员或部门' });
  await search.fill('平台小组');
  await page.locator('[data-testid="member-row-M-004"]').waitFor();
  assert.equal(await page.locator('.members-table tbody tr.ant-table-row').count(), 1);
  await search.fill('');
  await page.locator('.members-department-filter').click();
  await page.locator('.ant-select-item-option[title="应用小组"]').click();
  await page.waitForFunction(() => document.querySelectorAll('.members-table tbody tr.ant-table-row').length === 3);
  assert.equal(await page.locator('[data-testid="member-row-M-004"]').count(), 0);
  report.interactions.push('Search and the native department Select filter the same directory rows');
  await Promise.all([
    page.waitForResponse((response) => response.url().includes('/api/team/directory')),
    page.getByRole('button', { name: '刷新成员' }).click(),
  ]);
  report.interactions.push('Refresh rereads the project directory');
  await page.goto(`${appBase}/members?scope=P-001`);
  await settle(page, '[data-testid="member-table"]');
  await page.locator('[data-testid="members-summary-groups"]').click();
  await page.locator('[data-testid="members-detail"]').waitFor();
  assert.equal(await page.locator('[data-testid="members-detail-row"]').count(), 2);
  report.interactions.push('Group count and group detail rows agree');
  await page.goto(`${appBase}/members?scope=P-001`);
  await settle(page, '[data-testid="member-table"]');
  await page.locator('[data-testid="member-row-M-001"]').click();
  await page.waitForURL(/\/people\/M-001$/);
  report.interactions.push('A member row opens the designed person route');
  await context.close();

  if (liveSamples) {
    for (const filename of ['observer-projects.json', 'lin-projects.json']) {
      const body = await readFile(path.join(liveSamples, filename), 'utf8');
      const context = await contextFor(1440, 900, 'light');
      const page = await context.newPage();
      await page.route('**/api/team/projects', (route) => route.fulfill({ status: 200, contentType: 'application/json', body }));
      await page.goto(`${appBase}/members`);
      await settle(page, '[data-testid="page-members"]');
      const text = await page.locator('body').innerText();
      assert.doesNotMatch(text, /\b(?:undefined|null|NaN|personId|agentActive|connection_id|updated_at_ms)\b/);
      const screenshot = path.join(evidence, `verbatim-${filename.replace('.json', '')}.png`);
      await page.screenshot({ path: screenshot, fullPage: true });
      report.liveSamples.push({ filename, verbatim: true, screenshot, textCharacters: text.length });
      await context.close();
    }
  }

  await writeFile(path.join(evidence, 'members-browser-report.json'), `${JSON.stringify(report, null, 2)}\n`);
  console.log(JSON.stringify({ evidence, configurations: report.configurations.length, interactions: report.interactions, liveSamples: report.liveSamples }, null, 2));
} finally {
  await browser.close();
}
