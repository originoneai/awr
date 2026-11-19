// Real demo acceptance: run `npx vite build && node e2e/demo-organization-check.mjs`.
// Screenshots and the report are evidence only and stay in .local/.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { existsSync } from 'node:fs';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { chromium } from '@playwright/test';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const evidence = path.resolve(process.env.OBSERVER_EVIDENCE_DIR ?? path.join(root, '../../.local/ppo-063/browser'));
const fixture = JSON.parse(await readFile(path.join(root, 'fixtures/demo-organization.json'), 'utf8'));
const workTitle = (workId) => fixture.tasks.find((task) => task.work_id === workId)?.title ?? workId;
const chrome = '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome';
const results = [];
const failures = [];
let server;
let browser;

async function startServer(variant) {
  server = spawn(process.execPath, ['server.js', '--demo', '--demo-variant', variant, '--port', '0'], {
    cwd: root,
    stdio: ['ignore', 'pipe', 'pipe'],
    env: { ...process.env, AWR_OBSERVER_DEMO_VARIANT: variant },
  });
  let output = '';
  return new Promise((resolve, reject) => {
    const timeout = setTimeout(() => reject(new Error(`Observer did not start for ${variant}: ${output}`)), 10_000);
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
      reject(new Error(`Observer exited (${code}) for ${variant}: ${output}`));
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

async function settle(page) {
  await page.waitForLoadState('domcontentloaded');
  await page.waitForFunction(() => document.querySelectorAll('.ant-spin-spinning').length === 0, null, { timeout: 20_000 });
  await page.waitForTimeout(250);
}

async function workbenchQueue(page) {
  await page.locator('.ant-tabs-tab').nth(1).click();
  await page.getByTestId('workbench-table').waitFor();
  await page.locator('.ant-table-row').first().click();
  await page.getByTestId('workbench-detail').waitFor();
  await settle(page);
}

async function planDetail(page) {
  await page.getByTestId('work-plan-page').waitFor();
  await page.getByTestId(`plan-status-${workTitle('T-101')}`).waitFor().catch(() => {});
  await settle(page);
}

async function acceptanceDetail(page) {
  await page.getByTestId('acceptance-evidence-chain').waitFor();
  await settle(page);
  const earlier = page.getByRole('button', { name: /第 1 次复核|Review 1/ });
  if (await earlier.count()) {
    await earlier.first().click();
    await page.getByTestId('acceptance-review-round-t-101-1').waitFor();
    const latest = page.getByRole('button', { name: /返回最近一次复核|Return to latest review/ });
    if (await latest.count()) await latest.click();
  }
  const artifact = page.locator('[data-testid^="artifact-expand-"]').first();
  if (await artifact.count()) {
    await artifact.click();
    await page.locator('[data-testid^="artifact-read-content-"]').first().waitFor();
    await page.locator('[data-testid^="artifact-digest-"]').first().waitFor();
  }
  await settle(page);
}

async function documentDetail(page) {
  await page.getByTestId('project-document-library').waitFor();
  const read = page.getByRole('button', { name: /读取全文|Read full/ });
  if (await read.count()) {
    await read.first().click();
    await page.getByTestId('library-document-text').waitFor();
  }
  await settle(page);
}

const fullRoutes = [
  { name: 'workbench', path: '/workbench?scope=orders', action: workbenchQueue },
  { name: 'projects', path: '/projects' },
  { name: 'project-overview', path: '/projects/orders?tab=overview' },
  { name: 'project-plan', path: '/projects/orders?tab=plan&work=T-101', action: planDetail },
  { name: 'project-dependencies', path: '/projects/orders?tab=deps&work=T-103' },
  { name: 'project-acceptance', path: '/projects/orders?tab=acceptance&work=T-101', action: acceptanceDetail },
  { name: 'project-knowledge', path: '/projects/orders?tab=knowledge', action: documentDetail },
  { name: 'project-confirmations', path: '/projects/orders?tab=knowledge&knowledge=confirmations' },
  { name: 'notify-plan', path: '/projects/notify?tab=plan&work=T-206' },
  { name: 'onboard-plan', path: '/projects/onboard?tab=plan&work=T-303' },
  { name: 'inbox', path: '/inbox?scope=orders' },
  { name: 'members', path: '/members?scope=orders' },
  { name: 'activity-development', path: '/activity?project=orders&kind=development' },
  { name: 'activity-requests', path: '/activity?project=orders&kind=requests' },
  { name: 'account', path: '/account?scope=orders' },
  { name: 'agent-connect', path: '/agent-connect?scope=orders' },
  { name: 'lessons-pending', path: '/lessons', pending: true },
  { name: 'settings-pending', path: '/settings', pending: true },
  { name: 'task-pending', path: '/tasks/T-101', pending: true },
  { name: 'goal-pending', path: '/goals/g1', pending: true },
  { name: 'document-pending', path: '/documents/d1', pending: true },
  { name: 'update-pending', path: '/updates/o47', pending: true },
  { name: 'person-pending', path: '/members/lin-yue', pending: true },
  { name: 'takeover-pending', path: '/takeover', pending: true },
];

const variantRoutes = {
  empty: [
    { name: 'projects', path: '/projects' },
    { name: 'members', path: '/members' },
  ],
  single: [
    { name: 'projects', path: '/projects' },
    { name: 'project-plan', path: '/projects/orders?tab=plan&work=T-101', action: planDetail },
  ],
  restricted: [
    { name: 'projects', path: '/projects' },
    { name: 'project-knowledge-denied', path: '/projects/orders?tab=knowledge', expectedErrors: ['Forbidden'] },
    { name: 'onboard-denied', path: '/projects/onboard?tab=plan', expectedErrors: ['Forbidden'] },
  ],
  partial: [
    { name: 'project-plan-partial', path: '/projects/orders?tab=plan&work=T-101', expectedErrors: ['Unavailable'] },
    { name: 'activity-partial', path: '/activity?project=orders&kind=development', expectedErrors: ['Unavailable'] },
  ],
};

async function firstLoadGeometry(page) {
  await page.locator('.shell-header .ant-breadcrumb > ol > li').first().waitFor();
  return page.evaluate(() => {
    const centers = [...document.querySelectorAll('.shell-header .ant-breadcrumb > ol > li')]
      .map((element) => {
        const rect = element.getBoundingClientRect();
        return rect.height > 0 ? rect.top + rect.height / 2 : null;
      })
      .filter((center) => center !== null);
    const switcherHeights = [...document.querySelectorAll('.shell-perspective-option')]
      .map((element) => element.getBoundingClientRect().height);
    const headerText = document.querySelector('.shell-header')?.textContent ?? '';
    return {
      centers,
      centerSpread: centers.length > 0 ? Math.max(...centers) - Math.min(...centers) : Number.POSITIVE_INFINITY,
      switcherHeights,
      hasHeaderLimitation: /小组职责与项目关系未知|小组与部门登记情况未知|尚未登记部门|尚未登记小组/.test(headerText),
    };
  });
}

async function visit(baseUrl, variant, route, viewport) {
  const context = await browser.newContext({
    viewport,
    colorScheme: 'light',
    locale: 'zh-CN',
  });
  await context.addInitScript(() => {
    localStorage.setItem('awr-observer-theme', 'light');
    localStorage.setItem('i18nextLng', 'zh-CN');
  });
  const page = await context.newPage();
  const pageErrors = [];
  const requestFailures = [];
  const apiErrors = [];
  page.on('pageerror', (error) => pageErrors.push(error.message));
  page.on('requestfailed', (request) => requestFailures.push(`${request.method()} ${request.url()} ${request.failure()?.errorText || ''}`));
  page.on('response', async (response) => {
    if (!response.url().includes('/api/team/')) return;
    if (response.status() >= 400) {
      apiErrors.push({ status: response.status(), url: response.url() });
      return;
    }
    try {
      const payload = await response.json();
      if (payload?.ok === false) apiErrors.push({ code: payload.error?.code, url: response.url() });
    } catch {
      // Static assets are not JSON; API responses always are, so a JSON parse
      // failure is itself a failed observation.
      apiErrors.push({ code: 'InvalidJson', url: response.url() });
    }
  });
  const actualPath = route.path.replace(/([?&](?:work|work_id)=)(T-\d{3})/g,
    (_match, prefix, workId) => `${prefix}${encodeURIComponent(workTitle(workId))}`);
  const target = `${baseUrl}${actualPath}`;
  try {
    await page.goto(target);
    const geometry = await firstLoadGeometry(page);
    if (route.action) await route.action(page);
    else await settle(page);
    await page.waitForTimeout(350);
    const body = await page.locator('body').innerText();
    const state = await page.evaluate(() => ({
      width: document.documentElement.clientWidth,
      scrollWidth: document.documentElement.scrollWidth,
      pending: document.querySelectorAll('[data-testid="state-not-open"], [data-testid$="-placeholder"]').length,
      title: document.title,
    }));
    const forbiddenText = [
      ...(body.match(/\b(?:undefined|null|NaN|Unsupported|BadGateway|ReadTimeout|protocol_version|workstream_id|contract_hash|source_snapshot_id|coordinator_epoch|execution_authorized|passed)\b/g) || []),
      ...(body.match(/\bT-\d{3}\b/g) || []),
      ...(body.match(/\bd[1-9]\b/g) || []),
      ...(body.match(/(?:^|\n)##+\s/g) || []),
    ];
    const expected = route.expectedErrors || [];
    const unexpectedApiErrors = apiErrors.filter((error) => !expected.includes(error.code));
    const screenshot = path.join(evidence, `${variant}-${route.name}-${viewport.width}x${viewport.height}.png`);
    await page.screenshot({ path: screenshot, fullPage: true });
    const record = {
      variant,
      route: actualPath,
      intendedPending: route.pending === true,
      pendingBlocks: state.pending,
      viewport: `${viewport.width}x${viewport.height}`,
      title: state.title,
      screenshot,
      bodyCharacters: body.length,
      horizontalOverflow: state.scrollWidth > state.width,
      firstLoadGeometry: geometry,
      pageErrors,
      requestFailures,
      apiErrors,
      forbiddenText,
    };
    results.push(record);
    if (!body.trim()) failures.push(`${variant} ${route.path} ${viewport.width}: empty page`);
    if (state.scrollWidth > state.width) failures.push(`${variant} ${route.path} ${viewport.width}: horizontal overflow ${state.scrollWidth}/${state.width}`);
    if (geometry.centerSpread > 1) failures.push(`${variant} ${route.path} ${viewport.width}: breadcrumb centres differ by ${geometry.centerSpread}px`);
    if (geometry.switcherHeights.some((height) => Math.abs(height - 28) > 0.01)) failures.push(`${variant} ${route.path} ${viewport.width}: switcher height is not 28px (${geometry.switcherHeights.join(', ')})`);
    if (geometry.hasHeaderLimitation) failures.push(`${variant} ${route.path} ${viewport.width}: limitation text leaked into the header`);
    if (pageErrors.length) failures.push(`${variant} ${route.path} ${viewport.width}: page errors ${pageErrors.join('; ')}`);
    if (requestFailures.length) failures.push(`${variant} ${route.path} ${viewport.width}: request failures ${requestFailures.join('; ')}`);
    if (unexpectedApiErrors.length) failures.push(`${variant} ${route.path} ${viewport.width}: API errors ${JSON.stringify(unexpectedApiErrors)}`);
    if (forbiddenText.length) failures.push(`${variant} ${route.path} ${viewport.width}: visible forbidden text ${forbiddenText.join(', ')}`);
    if (route.pending && state.pending === 0) failures.push(`${variant} ${route.path} ${viewport.width}: pending destination did not render its designed state`);
  } catch (error) {
    failures.push(`${variant} ${route.path} ${viewport.width}: ${error.stack || error}`);
    results.push({ variant, route: route.path, viewport: `${viewport.width}x${viewport.height}`, fatal: String(error) });
  } finally {
    await context.close();
  }
}

try {
  await mkdir(evidence, { recursive: true });
  browser = await chromium.launch({
    headless: true,
    ...(existsSync(chrome) ? { channel: 'chrome' } : {}),
  });
  for (const variant of ['full', 'empty', 'single', 'restricted', 'partial']) {
    const baseUrl = await startServer(variant);
    const routes = variant === 'full' ? fullRoutes : variantRoutes[variant];
    for (const viewport of [{ width: 1440, height: 900 }, { width: 390, height: 844 }]) {
      for (const route of routes) await visit(baseUrl, variant, route, viewport);
    }
    await stopServer();
  }
} finally {
  if (browser) await browser.close();
  await stopServer();
}

const report = {
  basis: 'Real node server.js --demo responses; no browser request stubs',
  viewports: ['1440x900 light', '390x844 light'],
  visited: results.length,
  failures,
  results,
};
await mkdir(evidence, { recursive: true });
await writeFile(path.join(evidence, 'report.json'), `${JSON.stringify(report, null, 2)}\n`);
console.log(JSON.stringify({ evidence, visited: results.length, failures }, null, 2));
assert.deepEqual(failures, []);
