// Real demo acceptance: run `npx vite build && node e2e/demo-organization-check.mjs`.
// Screenshots and the report are evidence only and stay in .local/.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdir, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { chromium } from '@playwright/test';
import { settle } from './geometry/shared.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

// Register each page module here: adding a new page check takes one module under
// e2e/geometry/<page>.mjs and one line in this list.
export const registeredPageModules = [
  'workbench',
  'project-plan',
  'project-acceptance',
  'project-knowledge',
  'activity',
  'inbox',
  'projects',
  'members',
];

export const fullRoutes = [
  { name: 'workbench', path: '/workbench?scope=P-001' },
  { name: 'projects', path: '/projects' },
  { name: 'project-overview', path: '/projects/P-001?tab=overview' },
  { name: 'project-plan', path: '/projects/P-001?tab=plan&work=T-101' },
  { name: 'project-dependencies', path: '/projects/P-001?tab=deps&work=T-103' },
  { name: 'project-acceptance', path: '/projects/P-001?tab=acceptance&work=T-101' },
  { name: 'project-knowledge', path: '/projects/P-001?tab=knowledge' },
  { name: 'project-confirmations', path: '/projects/P-001?tab=knowledge&knowledge=confirmations' },
  { name: 'notify-plan', path: '/projects/P-002?tab=plan&work=T-206' },
  { name: 'onboard-plan', path: '/projects/P-003?tab=plan&work=T-303' },
  { name: 'inbox', path: '/inbox?scope=P-001' },
  { name: 'members', path: '/members?scope=P-001' },
  { name: 'activity-development', path: '/activity?project=P-001&kind=development' },
  { name: 'activity-requests', path: '/activity?project=P-001&kind=requests' },
  { name: 'account', path: '/account?scope=P-001' },
  { name: 'agent-connect', path: '/agent-connect?scope=P-001' },
  { name: 'lessons-pending', path: '/lessons', pending: true },
  { name: 'settings', path: '/settings?scope=P-001' },
  { name: 'task-pending', path: '/tasks/T-101', pending: true },
  { name: 'goal-pending', path: '/goals/GOAL-001', pending: true },
  { name: 'document-pending', path: '/documents/DOC-001', pending: true },
  { name: 'update-pending', path: '/updates/U-001', pending: true },
  { name: 'person-pending', path: '/members/M-002', pending: true },
  { name: 'takeover-pending', path: '/takeover', pending: true },
];

export const variantRoutes = {
  empty: [
    { name: 'projects', path: '/projects' },
    // Without a selected project the page first asks for one organization directory; a
    // service without that read answers Unsupported by design and the chooser remains.
    { name: 'members-chooser', path: '/members', expectedErrors: ['Unsupported'] },
    { name: 'settings-empty', path: '/settings' },
  ],
  single: [
    { name: 'projects', path: '/projects' },
    { name: 'project-plan', path: '/projects/P-001?tab=plan&work=T-101' },
  ],
  restricted: [
    // In the restricted variant every composed overview read denies with Forbidden; the
    // shell's shared inbox indicators read one per visible project by design (AWR-PPO-030).
    { name: 'projects', path: '/projects', expectedErrors: ['Forbidden'] },
    { name: 'project-knowledge-denied', path: '/projects/P-001?tab=knowledge', expectedErrors: ['Forbidden'] },
    { name: 'onboard-denied', path: '/projects/P-003?tab=plan', expectedErrors: ['Forbidden'] },
    { name: 'settings-revoked', path: '/settings?scope=P-001', expectedErrors: ['Forbidden'] },
  ],
  partial: [
    { name: 'project-plan-partial', path: '/projects/P-001?tab=plan&work=T-101', expectedErrors: ['Unavailable'] },
    { name: 'activity-partial', path: '/activity?project=P-001&kind=development', expectedErrors: ['Unavailable'] },
    { name: 'settings-partial', path: '/settings?scope=P-001', expectedErrors: ['Unavailable'] },
  ],
};

export async function loadPageModules(moduleNames = registeredPageModules) {
  return Promise.all(moduleNames.map(async (id) => {
    const loaded = await import(`./geometry/${id}.mjs`);
    return loaded.default;
  }));
}

export function buildPageRegistry(modules) {
  const names = new Set();
  const reportKeys = new Set();
  const canonicalKeys = new Set();
  const routeActions = new Map();
  const geometryModules = [];
  const canonicalModules = [];

  for (const mod of modules) {
    assert.ok(mod && typeof mod.name === 'string' && mod.name.length > 0, 'page module must declare a non-empty name');
    assert.ok(!names.has(mod.name), `duplicate page module name: ${mod.name}`);
    names.add(mod.name);

    if (mod.reportKey || mod.matches || mod.measure || mod.check) {
      assert.equal(typeof mod.reportKey, 'string', `${mod.name}: reportKey must be a string`);
      assert.ok(!reportKeys.has(mod.reportKey), `duplicate geometry reportKey: ${mod.reportKey}`);
      assert.equal(typeof mod.matches, 'function', `${mod.name}: matches must be a function`);
      assert.equal(typeof mod.measure, 'function', `${mod.name}: measure must be a function`);
      assert.equal(typeof mod.check, 'function', `${mod.name}: check must be a function`);
      reportKeys.add(mod.reportKey);
      geometryModules.push(mod);
    }

    if (mod.routeActions) {
      for (const [routeName, action] of Object.entries(mod.routeActions)) {
        assert.equal(typeof action, 'function', `${mod.name}: route action ${routeName} must be a function`);
        assert.ok(!routeActions.has(routeName), `duplicate route action for ${routeName}`);
        routeActions.set(routeName, action);
      }
    }

    if (mod.canonical) {
      assert.equal(typeof mod.canonical.reportKey, 'string', `${mod.name}: canonical.reportKey must be a string`);
      assert.ok(!canonicalKeys.has(mod.canonical.reportKey), `duplicate canonical reportKey: ${mod.canonical.reportKey}`);
      assert.equal(typeof mod.canonical.route?.name, 'string', `${mod.name}: canonical.route.name must be a string`);
      assert.equal(typeof mod.canonical.route?.path, 'string', `${mod.name}: canonical.route.path must be a string`);
      assert.equal(typeof mod.canonical.route?.run, 'function', `${mod.name}: canonical.route.run must be a function`);
      canonicalKeys.add(mod.canonical.reportKey);
      canonicalModules.push(mod);
    }
  }

  return { geometryModules, routeActions, canonicalModules };
}

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

export async function runDemoOrganizationCheck({
  evidenceDir = path.resolve(process.env.OBSERVER_EVIDENCE_DIR ?? path.join(root, '../../.local/ppo-057/browser')),
  moduleNames = registeredPageModules,
} = {}) {
  const registry = buildPageRegistry(await loadPageModules(moduleNames));
  const results = [];
  const failures = [];
  const canonicalRecords = Object.fromEntries(
    registry.canonicalModules.map((mod) => [mod.canonical.reportKey, []]),
  );
  const canonicalRoutes = registry.canonicalModules.map((mod) => ({
    name: mod.canonical.route.name,
    path: mod.canonical.route.path,
    action: async (page) => {
      await mod.canonical.route.run(page, (item) => {
        canonicalRecords[mod.canonical.reportKey].push(item);
      });
    },
  }));

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
      const action = route.action ?? registry.routeActions.get(route.name);
      if (action) await action(page);
      else await settle(page);
      await page.waitForTimeout(350);
      const body = await page.locator('body').innerText();
      const pageGeometryMetrics = {};
      const measuredModules = [];
      for (const mod of registry.geometryModules) {
        const metric = mod.matches(route) ? await mod.measure(page) : null;
        pageGeometryMetrics[mod.reportKey] = metric;
        if (metric) measuredModules.push({ mod, metric });
      }
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
      const screenshot = path.join(evidenceDir, `${variant}-${route.name}-${viewport.width}x${viewport.height}.png`);
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
        ...pageGeometryMetrics,
        pageErrors,
        requestFailures,
        apiErrors,
        forbiddenText,
        keyLeaks,
        documentedKeyLeaks: expectedKeyLeaks,
        keyLeakExemptionReason: route.keyLeakReason ?? null,
        unexpectedKeyLeaks,
        missingExpectedKeys,
      };
      results.push(record);
      const compact = viewport.width <= 860;
      for (const { mod, metric } of measuredModules) {
        for (const failure of mod.check(metric, compact)) {
          failures.push(`${variant} ${route.path} ${viewport.width}: ${failure}`);
        }
      }
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
    await mkdir(evidenceDir, { recursive: true });
    browser = await chromium.launch({ headless: true });
    for (const variant of ['full', 'empty', 'single', 'restricted', 'partial', 'canonical']) {
      const baseUrl = await startServer(variant);
      const routes = variant === 'full' ? fullRoutes
        : variant === 'canonical' ? canonicalRoutes
        : variantRoutes[variant];
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
    exempted: item.documentedKeyLeaks,
    exemptionReason: item.keyLeakExemptionReason,
    screenshot: item.screenshot,
  }));
  const report = {
    basis: 'Real node server.js --demo responses; no browser request stubs',
    viewports: ['1440x900 light', '390x844 light'],
    visited: results.length,
    failures,
    documentedKeyLeaks,
    ...canonicalRecords,
    results,
  };
  await mkdir(evidenceDir, { recursive: true });
  await writeFile(path.join(evidenceDir, 'report.json'), `${JSON.stringify(report, null, 2)}\n`);
  console.log(JSON.stringify({
    evidence: evidenceDir,
    visited: results.length,
    failures,
    documentedKeyLeaks,
    ...canonicalRecords,
  }, null, 2));
  assert.deepEqual(failures, []);
  return report;
}

const isEntrypoint = process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (isEntrypoint) {
  await runDemoOrganizationCheck();
}
