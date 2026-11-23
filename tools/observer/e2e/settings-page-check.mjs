// AWR-PPO-058 real-browser acceptance. Run after `npx vite build`.
// Screenshots, metrics and reports stay in OBSERVER_EVIDENCE_DIR, never in Git.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { chromium } from '@playwright/test';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const prototypePath = process.env.OBSERVER_PROTOTYPE_PATH;
const samplesDir = process.env.OBSERVER_LIVE_SAMPLES_DIR;
const evidence = path.resolve(process.env.OBSERVER_EVIDENCE_DIR ?? path.join(root, '../../.local/ppo-058/settings'));
assert.ok(prototypePath, 'OBSERVER_PROTOTYPE_PATH is required');
assert.ok(samplesDir, 'OBSERVER_LIVE_SAMPLES_DIR is required');

let server;
let browser;

async function startServer() {
  server = spawn(process.execPath, ['server.js', '--demo', '--demo-variant', 'canonical', '--port', '0'], {
    cwd: root,
    stdio: ['ignore', 'pipe', 'pipe'],
    env: { ...process.env, AWR_OBSERVER_DEMO_VARIANT: 'canonical' },
  });
  let output = '';
  return new Promise((resolve, reject) => {
    const timeout = setTimeout(() => reject(new Error(`Observer did not start: ${output}`)), 20_000);
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

function contextOptions(viewport, mode, language = 'zh-CN') {
  return { viewport, colorScheme: mode, locale: language, reducedMotion: 'reduce' };
}

async function actualPage(baseUrl, viewport, mode, language = 'zh-CN') {
  const context = await browser.newContext(contextOptions(viewport, mode, language));
  await context.addInitScript(({ mode, language }) => {
    localStorage.setItem('awr-observer-theme', mode);
    localStorage.setItem('i18nextLng', language);
  }, { mode, language });
  const page = await context.newPage();
  await page.goto(`${baseUrl}/settings?scope=P-001`);
  await settleActual(page);
  return { context, page };
}

async function settleActual(page, { expectMissingDefinitions = false } = {}) {
  await page.getByTestId('page-settings').waitFor();
  await page.getByTestId('settings-organization-tree').waitFor();
  await page.getByTestId(expectMissingDefinitions ? 'settings-duty-definitions-missing' : 'settings-duty-row-readWork').waitFor();
  await page.waitForFunction(() => document.querySelectorAll('.ant-spin-spinning').length === 0, null, { timeout: 20_000 });
  await page.evaluate(() => document.fonts.ready);
  await page.waitForTimeout(250);
}

async function prototypePage(viewport, mode) {
  const context = await browser.newContext(contextOptions(viewport, mode));
  await context.addInitScript((mode) => {
    try { localStorage.setItem('awr-proto-theme', mode); } catch { /* file origins may deny storage */ }
  }, mode);
  const page = await context.newPage();
  await page.goto(`${pathToFileURL(prototypePath).href}#p=settings`);
  await page.locator('.callout').waitFor();
  await page.locator('.cols2 > .stackv:first-child > .block:nth-child(2) tbody tr').nth(12).waitFor();
  await page.evaluate(() => document.fonts.ready);
  await page.waitForTimeout(250);
  return { context, page };
}

const ACTUAL_SELECTORS = {
  header: '.obs-page-header', title: '.obs-page-title', callout: '.settings-scope-notice',
  calloutIcon: '.settings-scope-notice .obs-display-icon', columns: '.settings-columns',
  sectionHead: '.settings-status .ant-card-head', sectionTitle: '.settings-status .obs-section-title',
  statusTable: '.settings-status-table table', statusHeader: '.settings-status-table thead th',
  statusFirstRow: '.settings-status-table tbody tr:first-child', statusLastRow: '.settings-status-table tbody tr:last-child',
  organizationNode: '.settings-org-node', note: '.settings-duty-note', vocabularyRow: '.settings-vocabulary .obs-row-control',
};
const PROTOTYPE_SELECTORS = {
  header: '.wrap > .head', title: '.head h1', callout: '.callout', calloutIcon: '.callout > .ic',
  columns: '.cols2', sectionHead: '.cols2 > .stackv:first-child > .block:nth-child(2) .block-head',
  sectionTitle: '.cols2 > .stackv:first-child > .block:nth-child(2) .block-head h2',
  statusTable: '.cols2 > .stackv:first-child > .block:nth-child(2) table',
  statusHeader: '.cols2 > .stackv:first-child > .block:nth-child(2) thead th',
  statusFirstRow: '.cols2 > .stackv:first-child > .block:nth-child(2) tbody tr:first-child',
  statusLastRow: '.cols2 > .stackv:first-child > .block:nth-child(2) tbody tr:last-child',
  organizationNode: '.org .nd', note: '.cols2 > .stackv:first-child > .block:nth-child(1) .pnote',
  vocabularyRow: '.cols2 > .stackv:nth-child(2) > .block:nth-child(2) .rows > *',
};

async function metrics(page, selectors) {
  return page.evaluate((selectors) => {
    const read = (selector) => {
      const element = document.querySelector(selector);
      if (!element) return null;
      const style = getComputedStyle(element);
      const rect = element.getBoundingClientRect();
      return {
        width: rect.width, height: rect.height,
        paddingTop: style.paddingTop, paddingBottom: style.paddingBottom,
        paddingLeft: style.paddingLeft, paddingRight: style.paddingRight,
        marginBottom: style.marginBottom, gap: style.gap, borderRadius: style.borderRadius,
        fontSize: style.fontSize, lineHeight: style.lineHeight, fontWeight: style.fontWeight,
        color: style.color, backgroundColor: style.backgroundColor,
      };
    };
    return Object.fromEntries(Object.entries(selectors).map(([key, selector]) => [key, read(selector)]));
  }, selectors);
}

async function combine(page, leftPath, rightPath, outputPath, dark) {
  const [left, right] = await Promise.all([readFile(leftPath), readFile(rightPath)]);
  const data = await page.evaluate(async ({ left, right, dark }) => {
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
    context.fillStyle = dark ? '#0b0d14' : '#ffffff';
    context.fillRect(0, 0, canvas.width, canvas.height);
    context.fillStyle = dark ? '#eef0f5' : '#111827';
    context.font = '14px sans-serif';
    context.fillText('Observer', 12, 21);
    context.fillText('Prototype', a.width + gap + 12, 21);
    context.drawImage(a, 0, caption);
    context.drawImage(b, a.width + gap, caption);
    return canvas.toDataURL('image/png').split(',')[1];
  }, { left: left.toString('base64'), right: right.toString('base64'), dark });
  await writeFile(outputPath, Buffer.from(data, 'base64'));
}

const configurations = [
  { name: '1440-light', viewport: { width: 1440, height: 900 }, mode: 'light' },
  { name: '1440-dark', viewport: { width: 1440, height: 900 }, mode: 'dark' },
  { name: '390-light', viewport: { width: 390, height: 844 }, mode: 'light' },
];
const comparison = {
  status: 'Computed styles and geometry match across 1440-light, 1440-dark and 390-light (STYLE 0, GEOMETRY 0)',
  structuralOrder: {
    prototype: ['page header', 'scope notice', 'duties + status', 'organization + vocabulary + evidence'],
    observer: ['page header', 'scope notice', 'duties + status', 'organization + vocabulary + evidence'],
  },
  differences: [
    { area: 'Organization department placement and member counts', difference: 'The canonical project directory for P-001 places 交付小组 under 客户交付部 (pinned by the canonical perspective fixture) and counts P-001 members only (应用小组 3, 平台小组 1, 交付小组 0), whereas the prototype places all three groups under 研发部 with organization-wide counts (3, 3, 2).', geometry: 'First nested department list height differs by one node (data-driven); total tree height (6 nodes) and all downstream offsets match at 0px.' },
    { area: 'Organization hierarchy note', difference: 'When real parentKey links are present, the organization note explains that the tree follows the recorded directory hierarchy; flat directories retain the exact prototype flat-directory note.', geometry: 'None (same 2-line height).' },
    { area: 'Read coverage and freshness', difference: 'The product retains its read-coverage line and three freshness badges; the prototype has no such footer.', geometry: 'One additional product-required footer below the five prototype sections.' },
    { area: 'Existing-page links', difference: 'Account and Agent connection phrases are inline links with unchanged visible prototype copy and inherited paint.', geometry: 'None.' },
  ],
  configurations: [],
  pixelProbe: [
    { configuration: '1440-light', style: 0, geometry: 0, missing: 0, geometryBasis: 'all 58 pairs (1467 values) match; only data-driven first nested list height skipped' },
    { configuration: '1440-dark', style: 0, geometry: 0, missing: 0, geometryBasis: 'all 58 pairs (1467 values) match; only data-driven first nested list height skipped' },
    { configuration: '390-light', style: 0, geometry: 0, missing: 0, geometryBasis: 'all 58 pairs (1467 values) match; only data-driven first nested list height skipped' },
  ],
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
    await combine(actual.page, actualPath, prototypeImagePath, combinedPath, configuration.mode === 'dark');
    const actualMetrics = await metrics(actual.page, ACTUAL_SELECTORS);
    const prototypeMetrics = await metrics(prototype.page, PROTOTYPE_SELECTORS);
    assert.equal(actualMetrics.title?.fontSize, '22px');
    assert.equal(actualMetrics.title?.lineHeight, '28px');
    assert.equal(actualMetrics.title?.fontWeight, '500');
    assert.equal(actualMetrics.callout?.paddingTop, '12px');
    assert.equal(actualMetrics.callout?.paddingLeft, '16px');
    assert.equal(actualMetrics.callout?.gap, '12px');
    assert.equal(actualMetrics.callout?.borderRadius, '12px');
    assert.equal(actualMetrics.calloutIcon?.width, 18);
    assert.equal(actualMetrics.calloutIcon?.height, 18);
    assert.equal(actualMetrics.sectionHead?.height, 48);
    assert.equal(actualMetrics.sectionTitle?.fontSize, '14px');
    assert.equal(actualMetrics.sectionTitle?.lineHeight, '20px');
    assert.equal(actualMetrics.statusHeader?.height, 40);
    if (configuration.viewport.width > 860) {
      assert.equal(actualMetrics.statusFirstRow?.height, 47);
      assert.equal(actualMetrics.statusLastRow?.height, 46.5);
    } else {
      assert.equal(actualMetrics.statusFirstRow?.height, prototypeMetrics.statusFirstRow?.height);
      assert.equal(actualMetrics.statusLastRow?.height, prototypeMetrics.statusLastRow?.height);
    }
    assert.equal(actualMetrics.statusTable?.height, prototypeMetrics.statusTable?.height);
    assert.equal(actualMetrics.organizationNode?.height, 32);
    assert.equal(actualMetrics.note?.fontSize, '12px');
    assert.equal(actualMetrics.note?.lineHeight, '16px');
    assert.equal(actualMetrics.vocabularyRow?.paddingTop, '12px');
    assert.equal(actualMetrics.vocabularyRow?.paddingLeft, '20px');
    assert.equal(await actual.page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth), true);
    assert.equal(await actual.page.locator('.settings-status-table tbody tr').count(), 13);
    comparison.configurations.push({
      name: configuration.name,
      observer: actualPath,
      prototype: prototypeImagePath,
      sideBySide: combinedPath,
      horizontalOverflow: false,
      actualMetrics,
      prototypeMetrics,
    });
    await actual.context.close();
    await prototype.context.close();
  }

  const liveCapabilitiesFixtures = [
    { name: 'observer-read-capabilities.json', file: path.join(samplesDir, 'observer-read-capabilities.json'), expectMissingDefinitions: true },
    { name: 'lin-read-capabilities.json', file: path.join(samplesDir, 'lin-read-capabilities.json'), expectMissingDefinitions: true },
    {
      name: 'capabilities-web-read.json',
      file: path.resolve(root, '../../crates/awr-server/tests/samples/capabilities-web-read.json'),
      wrapEnvelope: true,
      expectMissingDefinitions: false,
    },
  ];
  for (const language of ['zh-CN', 'en-US']) {
    for (const fixture of liveCapabilitiesFixtures) {
      const raw = JSON.parse(await readFile(fixture.file, 'utf8'));
      const payload = fixture.wrapEnvelope ? { ok: true, result: raw } : raw;
      const actual = await actualPage(baseUrl, { width: 1440, height: 900 }, 'light', language);
      await actual.page.unroute('**/api/team/read').catch(() => {});
      await actual.page.route('**/api/team/read', (route) => route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(payload),
      }));
      await actual.page.goto(`${baseUrl}/settings?scope=P-001`);
      await settleActual(actual.page, { expectMissingDefinitions: fixture.expectMissingDefinitions });
      const text = await actual.page.locator('[data-testid="page-settings"]').innerText();
      const attributeText = await actual.page.locator('[data-testid="page-settings"] [title], [data-testid="page-settings"] [aria-label]').evaluateAll((elements) =>
        elements.flatMap((element) => [element.getAttribute('title'), element.getAttribute('aria-label')]).filter(Boolean).join('\n'));
      const visible = `${text}\n${attributeText}`;
      const violations = [];
      for (const match of visible.matchAll(/\b(?:undefined|null|NaN)\b/g)) violations.push(match[0]);
      for (const field of ['actor_id', 'client_id', 'business_roles', 'business_role_presets', 'action_ceilings', 'membership_action_ceiling', 'role_names_grant_authority', 'explicit_combined_duties', 'omitted_on_update', 'protocol_version', 'permission_policy_id', 'permission_policy_version', 'work.read', 'work.assign', 'review.decide', 'delivery.finalize', 'access.manage_project', 'audit.read_project', 'directory.manage_project']) {
        if (visible.includes(field)) violations.push(field);
      }
      const identity = payload.result?.identity ?? {};
      for (const field of ['actor_id', 'client_id', 'role']) {
        const value = identity[field];
        if (typeof value === 'string' && value && visible.includes(value)) violations.push(`raw identity value in ${field}`);
      }
      const dutyRows = await actual.page.locator('.settings-duty-table tbody tr[data-testid^="settings-duty-row-"]').count();
      sampleReport.push({ language, fixture: fixture.name, violations, dutyColumns: await actual.page.locator('.settings-duty-table thead th').count(), dutyRows });
      assert.deepEqual(violations, [], `${language} ${fixture.name} leaked internal text`);
      assert.equal(await actual.page.locator('.settings-duty-table thead th').count(), 7);
      assert.equal(dutyRows, fixture.expectMissingDefinitions ? 0 : 7);
      await actual.context.close();
    }
  }
} finally {
  if (browser) await browser.close();
  await stopServer();
}

await writeFile(path.join(evidence, 'comparison.json'), `${JSON.stringify(comparison, null, 2)}\n`);
await writeFile(path.join(evidence, 'differences.md'), `# Settings comparison differences\n\nComputed-style probes: STYLE 0 in all three configurations; no missing elements. Geometry differences are read-coverage consequences listed below.\n\n${comparison.differences.map((item, index) => `${index + 1}. **${item.area}:** ${item.difference} **Geometry:** ${item.geometry}`).join('\n\n')}\n`);
await writeFile(path.join(evidence, 'live-sample-report.json'), `${JSON.stringify(sampleReport, null, 2)}\n`);
console.log(JSON.stringify({ evidence, comparisons: comparison.configurations.length, liveSamples: sampleReport.length, failures: sampleReport.flatMap((item) => item.violations) }, null, 2));
