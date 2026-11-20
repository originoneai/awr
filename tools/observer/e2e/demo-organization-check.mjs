// Real demo acceptance: run `npx vite build && node e2e/demo-organization-check.mjs`.
// Screenshots and the report are evidence only and stay in .local/.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { existsSync } from 'node:fs';
import { mkdir, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { chromium } from '@playwright/test';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const evidence = path.resolve(process.env.OBSERVER_EVIDENCE_DIR ?? path.join(root, '../../.local/ppo-063/browser'));
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
  await page.getByTestId('workbench-frame').waitFor();
  assert.equal(await page.locator('.obs-kpi').count(), 4, 'workbench has four KPI cells');
  assert.equal(await page.locator('.obs-workbench-instrument').count(), 1, 'workbench has one verdict instrument');
  assert.equal(await page.locator('.obs-detail-panel').count(), 1, 'workbench has one detail panel');
  assert.ok(await page.locator('.obs-chain-node-root').count() <= 1, 'one visible chain has at most one root');
  if (await page.locator('.obs-chain-node-root').count()) {
    assert.equal(await page.locator('.obs-chain-node-root .obs-lamp[data-lamp="blocked"]').count(), 1, 'chain root is red');
  }
  const geometry = await page.evaluate(() => {
    const title = getComputedStyle(document.querySelector('.obs-page-title'));
    const panel = getComputedStyle(document.querySelector('.obs-detail-panel'));
    const body = getComputedStyle(document.querySelector('.obs-workbench-body'));
    const rail = document.querySelector('.obs-todo-rail').getBoundingClientRect();
    return {
      titleSize: title.fontSize,
      titleLeading: title.lineHeight,
      panelRadius: panel.borderTopLeftRadius,
      bodyColumns: body.gridTemplateColumns.split(' ').length,
      railWidth: Math.round(rail.width),
      overflow: document.documentElement.scrollWidth > document.documentElement.clientWidth,
    };
  });
  assert.deepEqual([geometry.titleSize, geometry.titleLeading, geometry.panelRadius], ['22px', '28px', '16px']);
  assert.equal(geometry.overflow, false, 'workbench has no horizontal overflow');
  if (page.viewportSize().width >= 1240) {
    assert.equal(geometry.bodyColumns, 2);
    assert.equal(geometry.railWidth, 348);
  } else {
    assert.equal(geometry.bodyColumns, 1);
  }
  await page.locator('.obs-kpi').first().click();
  const detail = await page.getByTestId('workbench-metric-detail').waitFor();
  const detailCount = Number(await page.getByTestId('workbench-metric-detail').getAttribute('data-count'));
  assert.equal(await page.locator('#workbench-metric-detail, [data-testid="workbench-metric-detail"]').locator('.obs-row-item').count(), detailCount,
    'KPI detail rows equal the number');
  await page.getByRole('button', { name: /返回工作台|Back to workbench/ }).click();
  await page.getByRole('tab', { name: /我在等|Waiting on others/ }).click();
  await settle(page);
}

async function planDetail(page) {
  await page.getByTestId('work-plan-page').waitFor();
  await page.getByTestId('plan-status-T-101').waitFor().catch(() => {});
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

// ActivityPage currently labels links with the work key rather than the title.
// Keep this exact, route-scoped evidence of the existing page-task gap. Any new
// key on these pages, a missing expected key, or a key anywhere else fails.
const ACTIVITY_WORK_KEY_LEAKS = ['T-103', 'T-112', 'T-105', 'T-102', 'T-104', 'T-108', 'T-107', 'T-101'];

const fullRoutes = [
  { name: 'workbench', path: '/workbench?scope=P-001', action: workbenchQueue },
  { name: 'projects', path: '/projects' },
  { name: 'project-overview', path: '/projects/P-001?tab=overview' },
  { name: 'project-plan', path: '/projects/P-001?tab=plan&work=T-101', action: planDetail },
  { name: 'project-dependencies', path: '/projects/P-001?tab=deps&work=T-103' },
  { name: 'project-acceptance', path: '/projects/P-001?tab=acceptance&work=T-101', action: acceptanceDetail },
  { name: 'project-knowledge', path: '/projects/P-001?tab=knowledge', action: documentDetail },
  { name: 'project-confirmations', path: '/projects/P-001?tab=knowledge&knowledge=confirmations' },
  { name: 'notify-plan', path: '/projects/P-002?tab=plan&work=T-206' },
  { name: 'onboard-plan', path: '/projects/P-003?tab=plan&work=T-303' },
  { name: 'inbox', path: '/inbox?scope=P-001' },
  { name: 'members', path: '/members?scope=P-001' },
  { name: 'activity-development', path: '/activity?project=P-001&kind=development', expectedKeyLeaks: ACTIVITY_WORK_KEY_LEAKS },
  { name: 'activity-requests', path: '/activity?project=P-001&kind=requests', expectedKeyLeaks: ACTIVITY_WORK_KEY_LEAKS },
  { name: 'account', path: '/account?scope=P-001' },
  { name: 'agent-connect', path: '/agent-connect?scope=P-001' },
  { name: 'lessons-pending', path: '/lessons', pending: true },
  { name: 'settings-pending', path: '/settings', pending: true },
  { name: 'task-pending', path: '/tasks/T-101', pending: true },
  { name: 'goal-pending', path: '/goals/GOAL-001', pending: true },
  { name: 'document-pending', path: '/documents/DOC-001', pending: true },
  { name: 'update-pending', path: '/updates/U-001', pending: true },
  { name: 'person-pending', path: '/members/M-002', pending: true },
  { name: 'takeover-pending', path: '/takeover', pending: true },
];

const variantRoutes = {
  empty: [
    { name: 'projects', path: '/projects' },
    { name: 'members', path: '/members' },
  ],
  single: [
    { name: 'projects', path: '/projects' },
    { name: 'project-plan', path: '/projects/P-001?tab=plan&work=T-101', action: planDetail },
  ],
  restricted: [
    { name: 'projects', path: '/projects' },
    { name: 'project-knowledge-denied', path: '/projects/P-001?tab=knowledge', expectedErrors: ['Forbidden'] },
    { name: 'onboard-denied', path: '/projects/P-003?tab=plan', expectedErrors: ['Forbidden'] },
  ],
  partial: [
    { name: 'project-plan-partial', path: '/projects/P-001?tab=plan&work=T-101', expectedErrors: ['Unavailable'] },
    { name: 'activity-partial', path: '/activity?project=P-001&kind=development', expectedErrors: ['Unavailable'] },
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
  const actualPath = route.path;
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
    const keyLeaks = body.match(/\b(?:T|P|WS|M|Q|DOC|GOAL|U|A|L|H|PC|PW)-\d{3}\b/g) || [];
    const expectedKeyLeaks = route.expectedKeyLeaks || [];
    const unexpectedKeyLeaks = keyLeaks.filter((key) => !expectedKeyLeaks.includes(key));
    const missingExpectedKeys = expectedKeyLeaks.filter((key) => !keyLeaks.includes(key));
    const forbiddenText = [
      ...(body.match(/\b(?:undefined|null|NaN|Unsupported|BadGateway|ReadTimeout|protocol_version|workstream_id|contract_hash|source_snapshot_id|coordinator_epoch|execution_authorized|passed)\b/g) || []),
      ...(body.match(/(?:^|\n)##+\s/g) || []),
      ...unexpectedKeyLeaks,
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
      keyLeaks,
      documentedKeyLeaks: expectedKeyLeaks,
      unexpectedKeyLeaks,
      missingExpectedKeys,
    };
    results.push(record);
    if (missingExpectedKeys.length) failures.push(`${variant} ${route.path} ${viewport.width}: documented key leaks disappeared; remove the stale exemption ${missingExpectedKeys.join(', ')}`);
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

const documentedKeyLeaks = results.filter((item) => item.keyLeaks?.length).map((item) => ({
  variant: item.variant,
  route: item.route,
  viewport: item.viewport,
  keys: item.keyLeaks,
  screenshot: item.screenshot,
}));
const report = {
  basis: 'Real node server.js --demo responses; no browser request stubs',
  viewports: ['1440x900 light', '390x844 light'],
  visited: results.length,
  failures,
  documentedKeyLeaks,
  results,
};
await mkdir(evidence, { recursive: true });
await writeFile(path.join(evidence, 'report.json'), `${JSON.stringify(report, null, 2)}\n`);
console.log(JSON.stringify({ evidence, visited: results.length, failures, documentedKeyLeaks }, null, 2));
assert.deepEqual(failures, []);
