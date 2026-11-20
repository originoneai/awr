// Local AWR-PPO-057 acceptance only. Run after `npx vite build`.
// Required paths are supplied through OBSERVER_PROTOTYPE_PATH and OBSERVER_LIVE_SAMPLES_DIR.
// Screenshots, comparisons and reports stay in OBSERVER_EVIDENCE_DIR (never in Git).
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { chromium } from '@playwright/test';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const prototypePath = process.env.OBSERVER_PROTOTYPE_PATH;
const samplesDir = process.env.OBSERVER_LIVE_SAMPLES_DIR;
const evidence = path.resolve(process.env.OBSERVER_EVIDENCE_DIR ?? path.join(root, '../../.local/ppo-057/activity'));
assert.ok(prototypePath, 'OBSERVER_PROTOTYPE_PATH is required');
assert.ok(samplesDir, 'OBSERVER_LIVE_SAMPLES_DIR is required');

let server;
let browser;

async function startServer() {
  server = spawn(process.execPath, ['server.js', '--demo', '--demo-variant', 'full', '--port', '0'], {
    cwd: root,
    stdio: ['ignore', 'pipe', 'pipe'],
    env: { ...process.env, AWR_OBSERVER_DEMO_VARIANT: 'full' },
  });
  let output = '';
  return new Promise((resolve, reject) => {
    const timeout = setTimeout(() => reject(new Error(`Observer did not start: ${output}`)), 10_000);
    server.stdout.on('data', (chunk) => {
      output += String(chunk);
      const match = output.match(/URL\s+(http:\/\/127\.0\.0\.1:\d+)/);
      if (match) {
        clearTimeout(timeout);
        resolve(match[1]);
      }
    });
    server.stderr.on('data', (chunk) => { output += String(chunk); });
    server.once('exit', (code) => {
      clearTimeout(timeout);
      reject(new Error(`Observer exited (${code}): ${output}`));
    });
  });
}

async function stopServer() {
  if (!server || server.killed) return;
  const stopped = new Promise((resolve) => server.once('exit', resolve));
  server.kill('SIGTERM');
  await stopped;
  server = undefined;
}

async function settleActual(page) {
  await page.getByTestId('page-activity').waitFor();
  await page.waitForFunction(() => !document.querySelector('.activity-loading')
    && document.querySelectorAll('.ant-spin-spinning').length === 0, null, { timeout: 20_000 });
  await page.evaluate(() => document.fonts.ready);
  await page.waitForTimeout(250);
}

function contextOptions(viewport, mode, language = 'zh-CN') {
  return {
    viewport,
    colorScheme: mode,
    locale: language,
    reducedMotion: 'reduce',
  };
}

async function actualPage(baseUrl, viewport, mode, language = 'zh-CN') {
  const context = await browser.newContext(contextOptions(viewport, mode, language));
  await context.addInitScript(({ mode, language }) => {
    localStorage.setItem('awr-observer-theme', mode);
    localStorage.setItem('i18nextLng', language);
  }, { mode, language });
  const page = await context.newPage();
  await page.goto(`${baseUrl}/activity?project=P-001&kind=development`);
  await settleActual(page);
  return { context, page };
}

async function prototypePage(viewport, mode) {
  const context = await browser.newContext(contextOptions(viewport, mode));
  await context.addInitScript((mode) => {
    try { localStorage.setItem('awr-proto-theme', mode); } catch { /* file origins may deny storage */ }
  }, mode);
  const page = await context.newPage();
  await page.goto(`${pathToFileURL(prototypePath).href}#p=activity&tab=orders`);
  await page.locator('.block .t, .block .empty').waitFor();
  await page.evaluate(() => document.fonts.ready);
  await page.waitForTimeout(250);
  return { context, page };
}

const ACTUAL_SELECTORS = {
  header: '.obs-page-header', filters: '.activity-filters', segment: '.activity-segment',
  option: '.activity-segment button', section: '.activity-section',
  sectionHead: '.activity-section .ant-card-head', tableHead: '.activity-table thead th', note: '.activity-pnote',
};
const PROTOTYPE_SELECTORS = {
  header: '.wrap > .head', filters: '.wrap > .filters', segment: '.wrap > .filters .seg',
  option: '.wrap > .filters .seg button', section: '.wrap > .block',
  sectionHead: '.wrap > .block .block-head', tableHead: '.wrap > .block .t th', note: '.wrap > .block .pnote',
};

async function metrics(page, selectors) {
  return page.evaluate((selectors) => {
    const read = (selector) => {
      const element = document.querySelector(selector);
      if (!element) return null;
      const style = getComputedStyle(element);
      return {
        height: element.getBoundingClientRect().height,
        paddingTop: style.paddingTop,
        paddingBottom: style.paddingBottom,
        paddingLeft: style.paddingLeft,
        paddingRight: style.paddingRight,
        marginBottom: style.marginBottom,
        gap: style.gap,
        borderRadius: style.borderRadius,
        fontSize: style.fontSize,
        lineHeight: style.lineHeight,
      };
    };
    return Object.fromEntries(Object.entries(selectors).map(([key, selector]) => [key, read(selector)]));
  }, selectors);
}

async function combine(page, leftPath, rightPath, outputPath) {
  const [left, right] = await Promise.all([readFile(leftPath), readFile(rightPath)]);
  const data = await page.evaluate(async ({ left, right }) => {
    const load = async (bytes) => {
      const image = new Image();
      image.src = `data:image/png;base64,${bytes}`;
      await image.decode();
      return image;
    };
    const [a, b] = await Promise.all([load(left), load(right)]);
    const caption = 32;
    const gap = 16;
    const canvas = document.createElement('canvas');
    canvas.width = a.width + b.width + gap;
    canvas.height = Math.max(a.height, b.height) + caption;
    const context = canvas.getContext('2d');
    context.fillStyle = '#ffffff';
    context.fillRect(0, 0, canvas.width, canvas.height);
    context.fillStyle = '#111827';
    context.font = '14px sans-serif';
    context.fillText('Observer', 12, 21);
    context.fillText('Prototype', a.width + gap + 12, 21);
    context.drawImage(a, 0, caption);
    context.drawImage(b, a.width + gap, caption);
    return canvas.toDataURL('image/png').split(',')[1];
  }, { left: left.toString('base64'), right: right.toString('base64') });
  await writeFile(outputPath, Buffer.from(data, 'base64'));
}

const configurations = [
  { name: '1440-light', viewport: { width: 1440, height: 900 }, mode: 'light' },
  { name: '1440-dark', viewport: { width: 1440, height: 900 }, mode: 'dark' },
  { name: '390-light', viewport: { width: 390, height: 844 }, mode: 'light' },
];

const comparison = {
  status: 'Pending supervisor review',
  structuralOrder: {
    prototype: ['page header', 'project and category filters', 'What happened section'],
    observer: ['page header', 'project and category filters', 'What happened section'],
    difference: 'No main-block order difference in the default data view',
  },
  acceptedBySupervisor: false,
  differences: [
    { area: 'Section heading', difference: 'The prototype-only data-source badge is absent in the product.', reason: 'Owner rule: demonstration source controls and internal operation text do not ship.' },
    { area: 'Section footer', difference: 'Scope, read coverage, freshness and optional earlier-history control remain visible below the required collection note.', reason: 'AWR-PPO-057 explicitly retains these existing observation contracts.' },
    { area: 'Rows and counts', difference: 'The synthetic demo organization returns different operations and times; older timestamps retain the shared full date-time formatter rather than the prototype sample abbreviations.', reason: 'Data-dependent content and the existing observation time contract; operation labels, titles and empty states use the prototype vocabulary.' },
    { area: 'People and Agents', difference: 'Unlinked actor records say Name not registered and Agent attribution is generic when no Agent name is recorded.', reason: 'The activity reply has identifiers only; the page never guesses a member or Agent name.' },
    { area: 'Mobile table', difference: 'The shared DataTable uses 16px outer cell padding below 860px, and ellipsized zero-width action text keeps bounded rows; the prototype uses 20px and wraps that zero-width text into very tall rows.', reason: 'Existing compact-table geometry and the requirement to truncate untrusted long text without breaking layout; the prototype wrapping defect is not reproduced.' },
    { area: 'Application shell', difference: 'The product uses its authorized perspective and has no prototype identity/source demonstration switches.', reason: 'Previously accepted application-shell and read-only product boundaries.' },
    { area: 'Detail interaction', difference: 'An event deep link retains the detail panel below the ledger; default comparison screenshots have no selected event.', reason: 'Retain the existing detail contract without adding a default block absent from the prototype.' },
  ],
  configurations: [],
};

const sampleReport = [];

try {
  await mkdir(evidence, { recursive: true });
  const baseUrl = await startServer();
  browser = await chromium.launch({ headless: true });
  for (const configuration of configurations) {
    const actual = await actualPage(baseUrl, configuration.viewport, configuration.mode);
    const prototype = await prototypePage(configuration.viewport, configuration.mode);
    const actualPath = path.join(evidence, `${configuration.name}-observer.png`);
    const prototypeImagePath = path.join(evidence, `${configuration.name}-prototype.png`);
    const combinedPath = path.join(evidence, `${configuration.name}-side-by-side.png`);
    await actual.page.screenshot({ path: actualPath, fullPage: true });
    await prototype.page.screenshot({ path: prototypeImagePath, fullPage: true });
    await combine(actual.page, actualPath, prototypeImagePath, combinedPath);
    const actualMetrics = await metrics(actual.page, ACTUAL_SELECTORS);
    const prototypeMetrics = await metrics(prototype.page, PROTOTYPE_SELECTORS);
    assert.equal(actualMetrics.option?.height, 28);
    assert.equal(actualMetrics.sectionHead?.height, 48);
    assert.equal(actualMetrics.tableHead?.height, 40);
    assert.equal(actualMetrics.note?.fontSize, '12px');
    assert.equal(actualMetrics.note?.lineHeight, '16px');
    comparison.configurations.push({
      name: configuration.name,
      observer: actualPath,
      prototype: prototypeImagePath,
      sideBySide: combinedPath,
      actualMetrics,
      prototypeMetrics,
    });
    await actual.context.close();
    await prototype.context.close();
  }

  for (const language of ['zh-CN', 'en-US']) {
    for (const kind of ['development', 'requests']) {
      const fixture = kind === 'development' ? 'observer-activity-development.json' : 'observer-activity-requests.json';
      const payload = JSON.parse(await readFile(path.join(samplesDir, fixture), 'utf8'));
      const actual = await actualPage(baseUrl, { width: 1440, height: 900 }, 'light', language);
      await actual.page.unroute('**/api/team/activity?*').catch(() => {});
      await actual.page.route('**/api/team/activity?*', (route) => route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(payload),
      }));
      await actual.page.goto(`${baseUrl}/activity?project=P-001&kind=${kind}`);
      await settleActual(actual.page);
      const text = await actual.page.locator('body').innerText();
      const attributeText = await actual.page.locator('[title], [aria-label]').evaluateAll((elements) =>
        elements.flatMap((element) => [element.getAttribute('title'), element.getAttribute('aria-label')]).filter(Boolean).join('\n'));
      const visible = `${text}\n${attributeText}`;
      const violations = [];
      for (const match of visible.matchAll(/\b(?:undefined|null|NaN)\b/g)) violations.push(match[0]);
      for (const field of ['actor_id', 'client_id', 'credential_id', 'work_id', 'created_at_unix_ms', 'unknown_means', 'development_source', 'protocol_version']) {
        if (visible.includes(field)) violations.push(field);
      }
      for (const item of payload.data.items) {
        for (const field of ['id', 'actor_id', 'client_id', 'credential_id', 'work_id', 'action']) {
          const value = item[field];
          if (typeof value === 'string' && value && visible.includes(value)) violations.push(`raw activity value in ${field}`);
        }
      }
      sampleReport.push({ language, kind, fixture, violations });
      assert.deepEqual(violations, [], `${language} ${kind} leaked internal text`);
      await actual.context.close();
    }
  }
} finally {
  if (browser) await browser.close();
  await stopServer();
}

await writeFile(path.join(evidence, 'comparison.json'), `${JSON.stringify(comparison, null, 2)}\n`);
await writeFile(path.join(evidence, 'differences.md'), `# Activity comparison differences\n\nStatus: pending supervisor review.\n\n${comparison.differences.map((item, index) => `${index + 1}. **${item.area}:** ${item.difference} **Reason:** ${item.reason}`).join('\n')}\n`);
await writeFile(path.join(evidence, 'live-sample-report.json'), `${JSON.stringify(sampleReport, null, 2)}\n`);
console.log(JSON.stringify({ evidence, comparisons: comparison.configurations.length, liveSamples: sampleReport.length, failures: sampleReport.flatMap((item) => item.violations) }, null, 2));
