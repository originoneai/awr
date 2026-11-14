// Browser fixture verification of product language and responsive layout, not business acceptance.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdir } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { chromium } from '@playwright/test';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const evidence = path.resolve(process.env.OBSERVER_EVIDENCE_DIR ?? path.join(root, '../../.local/observer-plan-check'));
const at = 1793534400000;
const digest = 'b'.repeat(64);
const internal = '01M7DRXDZWVP66RCRGK190BCXF';
const freshness = { page_read: { status: 'measured', at_unix_ms: at },
  authoritative: { status: 'measured', at_unix_ms: at }, report_received: { status: 'not_measured' } };
const overview = { ok: true, project: 'demo', works: [],
  workstreams: [{ id: 'development', title: 'Customer portal', external_key: 'portal' }],
  coverage: { scope: { status: 'succeeded', enumerated: 0, truncated: false, next_cursor: null, read_at_unix_ms: at },
    streams: [], works_enumerated: 0, works_total: 0, succeeded: 0, failed: 0, truncated: false }, freshness };
const planning = { items: [{ candidate_id: internal, candidate_digest: digest, draft_revision: 2,
  state: 'approved', author: { display_name: 'Maya' }, updated_at_unix_ms: at,
  baseline: { matches_current: true }, approval: { value: { display_name: 'Kai' } },
  publication: { value: null }, activation: { value: null },
  current_diff: { counts: { added: 0, changed: 1, removed: 0, unchanged: 0 }, items: [{
    work_id: internal, title: 'Clarify customer sign-in requirements', kind: 'changed',
    fields: [{ field: 'scope_paths', before: { value: '/private/original' }, after: { value: '/private/implementation' } }],
  }] },
}], total: 1, next_cursor: null, as_of: { at_unix_ms: at } };
const sources = { items: [
  { source_position: 1, path: 'docs/GOALS.md', status: 'consistent', reason: null, expected_sha256: digest, current_sha256: digest },
  { source_position: 2, path: null, status: 'forbidden', reason: 'required_spec_forbidden', expected_sha256: null, current_sha256: null },
], total: 2, next_cursor: null, as_of: { at_unix_ms: at },
collection: { complete: false, required_total: 2, checked_at_unix_ms: at } };

let browser;
const server = spawn(process.execPath, ['server.js', '--demo', '--port', '0'], { cwd: root, stdio: ['ignore', 'pipe', 'pipe'] });
try {
  const baseUrl = await new Promise((resolve, reject) => {
    let text = '';
    const timeout = setTimeout(() => reject(new Error('Observer did not start')), 10000);
    server.stdout.on('data', (chunk) => {
      text += chunk;
      const match = text.match(/URL\s+(http:\/\/127\.0\.0\.1:\d+)/);
      if (match) { clearTimeout(timeout); resolve(match[1]); }
    });
    server.once('exit', (code) => { clearTimeout(timeout); reject(new Error(`Observer exited: ${code}`)); });
  });
  await mkdir(evidence, { recursive: true });
  browser = await chromium.launch({ headless: true, channel: 'chrome' });
  const results = [];
  for (const language of ['zh-CN', 'en-US']) {
    for (const width of [1365, 390]) {
      const context = await browser.newContext({ viewport: { width, height: 900 } });
      await context.addInitScript((value) => localStorage.setItem('i18nextLng', value), language);
      const page = await context.newPage();
      const errors = [];
      page.on('pageerror', (error) => errors.push(error.message));
      await page.route('**/api/team/**', async (route) => {
        const request = route.request();
        const url = new URL(request.url());
        let payload;
        if (url.pathname.endsWith('/projects')) payload = { ok: true, projects: [{ key: 'demo', title: 'Customer portal', role: 'project_admin' }] };
        else if (url.pathname.endsWith('/overview')) payload = overview;
        else if (url.pathname.endsWith('/read')) {
          const query = request.postDataJSON().query;
          assert.ok(['planning.candidates', 'sources.inspect'].includes(query.op), `Unexpected read: ${query.op}`);
          payload = { ok: true, result: { data: query.op === 'planning.candidates' ? planning : sources }, freshness };
        } else throw new Error(`Unexpected route: ${url.pathname}`);
        await route.fulfill({ contentType: 'application/json', body: JSON.stringify(payload) });
      });
      await page.goto(`${baseUrl}/projects/demo?tab=plan`);
      const changes = page.getByTestId('plan-changes');
      const materials = page.getByTestId('plan-materials');
      await changes.getByText('Clarify customer sign-in requirements').waitFor();
      await materials.getByText(language === 'zh-CN' ? '部分资料需要处理' : 'Some documents need attention').waitFor();
      await materials.getByRole('button', { name: language === 'zh-CN' ? '刷新' : 'Refresh', exact: true }).click();
      await page.waitForFunction(() => !document.querySelector('.ant-spin-spinning'));
      const metrics = await page.evaluate(() => ({ width: window.innerWidth, document: document.documentElement.scrollWidth,
        text: document.body.innerText + Array.from(document.querySelectorAll('[aria-label], [title]')).map((el) => el.getAttribute('aria-label') || el.getAttribute('title')).join(' ') }));
      assert.ok(metrics.document <= width, `Horizontal overflow at ${width}: ${metrics.document}`);
      assert.ok(!metrics.text.includes(digest) && !metrics.text.includes(internal));
      assert.doesNotMatch(metrics.text, /scope_paths|source_snapshot_id|required_spec_forbidden|\/private\/implementation|技术细节|Technical details/);
      assert.equal(await page.getByRole('button', { name: /Approve|Publish|Claim|批准|认领|发布/ }).count(), 0);
      assert.deepEqual(errors, []);
      await page.evaluate(() => window.scrollTo(0, 0));
      await page.waitForFunction(() => window.scrollY === 0);
      const screenshot = path.join(evidence, `obs004-${language}-${width}.png`);
      await page.screenshot({ path: screenshot, fullPage: true });
      results.push({ language, viewport: width, document: metrics.document, screenshot });
      await context.close();
    }
  }
  console.log(JSON.stringify({ basis: 'synthetic browser fixtures', results }));
} finally {
  if (browser) await browser.close();
  server.kill('SIGTERM');
}
