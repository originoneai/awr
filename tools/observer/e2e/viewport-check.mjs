// Local acceptance only (not CI): run `npx vite build && node e2e/viewport-check.mjs`.
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

const freshness = {
  report_received: { status: 'not_measured' },
  authoritative: { status: 'not_measured' },
  page_read: { status: 'measured', at_unix_ms: 1761840000000 },
};
const items = [
  {
    navigation: 'resume',
    work_id: 'W1',
    workstream_id: 'WS1',
    state: 'claimed',
    title: 'Build the filter API',
    responsibility: { relation: 'owned_by_me', owner: 'lin', version: '1' },
    next_query: { op: 'work.observe', protocol_version: 1, work_id: 'W1', workstream_id: 'WS1' },
  },
  {
    navigation: 'prepare',
    work_id: 'W2',
    workstream_id: 'WS2',
    state: null,
    title: 'Prepare the support page',
    responsibility: { relation: 'pool', owner: null, version: '1' },
    next_query: { op: 'work.prepare', protocol_version: 1, work_id: 'W2', workstream_id: 'WS2' },
  },
];
const workNext = {
  ok: true,
  result: {
    data: {
      identity: { actor_id: 'lin', role: 'developer', business_roles: ['developer'] },
      items,
      resume: [],
      next_cursor: null,
      next_action: null,
      recheck_on: [],
      execution_authorized: false,
    },
    project_revision: '1',
    protocol_version: 1,
  },
  query: { protocol_version: 1, op: 'work.next' },
  freshness,
};

function workSnapshot(workId) {
  return {
    ok: true,
    result: {
      data: {
        work_id: workId,
        observation: {
          last_activity_at_unix_ms: 1761839900000,
          observed_at_unix_ms: 1761839999000,
          progress: null,
          reporting: { version: 1, client_reported_at_unix_ms: 1761839800000 },
        },
        visible_contract: {
          goals: ['Keep support filters useful'],
          acceptance: ['Do not mutate input records'],
          required_dependencies: [],
        },
        guidance: { action: { op: 'session.checkpoint', note: 'Record the verified outcome.' } },
        next_step: null,
      },
      scope_id: 'main',
      source_snapshot_id: 'snapshot-1',
      coordinator_epoch: 'epoch-1',
      project_revision: '1',
      protocol_version: 1,
    },
    query: { protocol_version: 1, op: 'work.snapshot', work_id: workId },
    freshness,
  };
}

let browser;
try {
  const baseUrl = await observerUrl();
  const executablePath = chromePath();
  browser = await chromium.launch({ headless: true, ...(executablePath ? { executablePath } : {}) });
  const page = await browser.newPage({
    viewport: { width: 1341, height: 838 },
    deviceScaleFactor: 1.183,
  });
  await page.route('**/api/team/projects', (route) => route.fulfill({
    status: 200,
    contentType: 'application/json',
    body: JSON.stringify({
      ok: true,
      projects: [{ key: 'demo', title: 'Team mainline', role: 'project_admin' }],
    }),
  }));
  await page.route('**/api/team/read', (route) => {
    const query = route.request().postDataJSON().query;
    const payload = query.op === 'work.next'
      ? workNext
      : query.op === 'work.snapshot'
        ? workSnapshot(query.work_id)
        : { ok: false, error: { code: 'UnexpectedRead', message: query.op } };
    return route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(payload) });
  });
  await page.goto(`${baseUrl}/workbench?scope=demo`);
  await page.getByRole('link', { name: 'Build the filter API' }).waitFor();
  await page.waitForFunction(() => {
    const value = document.querySelector('[data-testid="last-report-W1"]')?.textContent;
    return value && value !== '加载中…';
  });
  const table = page.locator('.ant-table-content').first();
  await table.waitFor();
  const desktop = await table.evaluate((element) => ({
    clientWidth: element.clientWidth,
    scrollWidth: element.scrollWidth,
  }));
  assert.ok(
    desktop.scrollWidth <= desktop.clientWidth,
    `1341px table must fit without internal scrolling: ${JSON.stringify(desktop)}`,
  );

  await page.setViewportSize({ width: 375, height: 812 });
  await page.waitForFunction(() => document.querySelector('.ant-layout-sider-zero-width')
    && document.documentElement.scrollWidth <= 375);
  const narrow = await page.evaluate(() => ({
    clientWidth: document.documentElement.clientWidth,
    scrollWidth: document.documentElement.scrollWidth,
  }));
  assert.ok(narrow.scrollWidth <= 375, `375px page must not overflow: ${JSON.stringify(narrow)}`);
  console.log(`viewport-check: 1341px table ${desktop.scrollWidth}/${desktop.clientWidth}; 375px document ${narrow.scrollWidth}/${narrow.clientWidth}`);
} finally {
  if (browser) await browser.close();
  if (serverProcess) serverProcess.kill('SIGTERM');
}
