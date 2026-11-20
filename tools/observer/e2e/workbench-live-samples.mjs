// Verbatim live-sample replay. No screenshot or response body is written to the evidence report.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { chromium } from '@playwright/test';

const root = process.cwd();
const sampleDir = process.env.AWR_LIVE_SAMPLES_DIR;
if (!sampleDir) throw new Error('Set AWR_LIVE_SAMPLES_DIR to the read-only live-samples directory');
const evidence = path.resolve(process.env.OBSERVER_EVIDENCE_DIR ?? path.join(root, '../../.local/ppo-020/browser'));
const snapshot = JSON.parse(await readFile(path.join(sampleDir, 'service-work-snapshot-api-in-review.json'), 'utf8'));
const review = JSON.parse(await readFile(path.join(sampleDir, 'service-review-inspect-api-open.json'), 'utf8'));
const workId = snapshot.data.work_id;
const round = review.data.review;
const reportedAt = snapshot.data.observation.progress.reported_at_unix_ms;
const personId = round.author_person_id;
const now = reportedAt + 2 * 86_400_000;
const complete = (items) => ({ coverage: 'complete', items });
const input = {
  tasks: complete([{
    id: workId,
    title: null,
    ownerPersonId: personId,
    agentName: null,
    projectId: 'P-001',
    acceptance: undefined,
    report: { completed: false, reportedAtUnixMs: reportedAt },
    submittedResultAtUnixMs: null,
    integrated: false,
    started: true,
    agentActive: false,
    externalWait: null,
    commitments: undefined,
  }]),
  dependencies: complete([]),
  decisions: complete([]),
  reviewRounds: complete([{
    id: round.round_id,
    taskId: workId,
    roundIndex: round.round_index,
    state: round.state,
    current: true,
    independent: null,
    authorPersonId: personId,
    reviewerPersonId: null,
    authorActorId: null,
    reviewerActorId: null,
    openedAtUnixMs: null,
  }]),
  handoffs: complete([]),
  approvals: complete([]),
  members: complete([{ personId, displayName: null }]),
  activity: complete([]),
  options: {
    nowUnixMs: now,
    stalledAfterDays: 1,
    views: {
      me: { personId, projectId: 'P-001' },
      lead: { personId, projectId: 'P-001' },
      currentWeek: { startUnixMs: 0, endUnixMs: now },
      previousWeek: { startUnixMs: 0, endUnixMs: now },
    },
  },
};
const response = {
  ok: true,
  project: 'P-001',
  read_at_unix_ms: now,
  input,
  coverage: Object.fromEntries(Object.keys(input).filter((key) => !['options'].includes(key)).map((key) => [key, { status: 'complete', missing: [] }])),
  freshness: {
    report_received: { status: 'not_measured' },
    authoritative: { status: 'measured', at_unix_ms: now },
    page_read: { status: 'measured', at_unix_ms: now },
  },
};

let server;
let browser;
async function startServer() {
  server = spawn(process.execPath, ['server.js', '--demo', '--demo-variant', 'full', '--port', '0'], {
    cwd: root,
    stdio: ['ignore', 'pipe', 'pipe'],
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

const results = [];
try {
  await mkdir(evidence, { recursive: true });
  const baseUrl = await startServer();
  browser = await chromium.launch({ headless: true });
  for (const language of ['zh-CN', 'en-US']) {
    const context = await browser.newContext({ viewport: { width: 1440, height: 900 }, locale: language });
    await context.addInitScript((lang) => {
      localStorage.setItem('awr-observer-theme', 'light');
      localStorage.setItem('i18nextLng', lang);
    }, language);
    const page = await context.newPage();
    await page.route('**/api/team/projects', (route) => route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({ ok: true, projects: [{ key: 'P-001', title: 'Verbatim sample project' }] }),
    }));
    await page.route('**/api/team/overview-model?*', (route) => route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify(response),
    }));
    await page.goto(`${baseUrl}/workbench?scope=P-001&perspective=me`);
    await page.locator('[data-testid="workbench-frame"]').waitFor();
    const text = await page.locator('[data-testid="workbench-frame"]').innerText();
    const forbidden = text.match(/\b(?:undefined|null|NaN|protocol_version|workstream_id|work_id|round_id|contract_hash|source_snapshot_id|coordinator_epoch|execution_authorized|author_person_id|reviewerPersonId|read_at_unix_ms|coverage)\b/g) ?? [];
    const internalIds = text.match(/issue-filter-api|01M6[A-Z0-9]+/g) ?? [];
    results.push({ language, frameCharacters: text.length, forbidden, internalIds, rendered: text.length > 100 });
    assert.deepEqual(forbidden, [], `${language} leaked an empty artifact or raw field`);
    assert.deepEqual(internalIds, [], `${language} leaked an internal identifier`);
    await context.close();
  }
} finally {
  if (browser) await browser.close();
  if (server && !server.killed) {
    const stopped = new Promise((resolve) => server.once('exit', resolve));
    server.kill('SIGTERM');
    await stopped;
  }
}
const report = {
  basis: 'Verbatim service-work-snapshot-api-in-review and service-review-inspect-api-open replies',
  screenshots: false,
  results,
};
await writeFile(path.join(evidence, 'workbench-live-samples.json'), `${JSON.stringify(report, null, 2)}\n`);
console.log(JSON.stringify(report, null, 2));
