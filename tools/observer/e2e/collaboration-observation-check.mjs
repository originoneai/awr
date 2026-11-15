// Synthetic browser regression for product-only responsibility and dependency views.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdir } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { chromium } from '@playwright/test';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const evidence = path.resolve(process.env.OBSERVER_EVIDENCE_DIR ?? path.join(root, '../../.local/observer-collaboration-check'));
const id = suffix => `01J${'0'.repeat(22)}${suffix}`;
const work = id('1'), provider = id('2'), stream = id('3');
const digest = 'b'.repeat(64), at = Date.now();
const version = { receipt_id: id('4'), contract_hash: digest, source_snapshot_id: id('5') };
const freshness = { page_read: { status: 'measured', at_unix_ms: at },
  authoritative: { status: 'measured', at_unix_ms: at }, report_received: { status: 'not_measured' } };
const snapshot = { work_id: work, context_complete: true, contract_hash: digest, required_specs: [],
  visible_contract: { acceptance: ['Retain the latest customer filters'], goals: ['Deliver the customer portal'] },
  observation: { runtime: { state: 'claimed' }, observed_at_unix_ms: at,
    responsibility: { owner: id('6'), owner_name: 'Lin Chen', pending: { person_id: id('7') }, pending_name: 'Shen Lan',
      current_executor: { person_id: id('8'), agent_id: id('9') }, executor_person_name: 'Zhou Ning', executor_agent_name: 'Delivery assistant' },
    claim: { id: id('A'), lease_live: true },
    progress: { phase: 'implementing', summary: 'The customer list keeps the latest selected filters',
      origin: { actor_name: 'Review assistant' }, reported_at_unix_ms: at, stale: false },
  } };
const overview = { ok: true, project: 'demo', works: [{ key: work, workstream_id: stream,
  title: 'Build the customer portal', status: 'claimed', owner_person: null, agent: null,
  next_step: null, detail_loaded: false, contract_hash: digest }],
  workstreams: [{ id: stream, title: 'Customer portal' }],
  coverage: { scope: { status: 'succeeded', enumerated: 1, truncated: false, next_cursor: null, read_at_unix_ms: at },
    streams: [], works_enumerated: 1, works_total: 1, succeeded: 1, failed: 0, truncated: false }, freshness };
const handoff = { handoff_id: id('B'), kind: 'execution', effective_status: 'expired', pending: false, expired: true,
  from_display_name: 'Lin Chen', to_display_name: 'Shen Lan', updated_at_unix_ms: at, expires_at_unix_ms: at - 1000 };

let browser;
const server = spawn(process.execPath, ['server.js', '--demo', '--port', '0'], { cwd: root, stdio: ['ignore', 'pipe', 'pipe'] });
try {
  const baseUrl = await new Promise((resolve, reject) => {
    let output = '';
    const timeout = setTimeout(() => reject(new Error('Observer did not start')), 10000);
    server.stdout.on('data', chunk => {
      output += chunk;
      const match = output.match(/URL\s+(http:\/\/127\.0\.0\.1:\d+)/);
      if (match) { clearTimeout(timeout); resolve(match[1]); }
    });
    server.once('exit', code => { clearTimeout(timeout); reject(new Error(`Observer exited ${code}`)); });
  });
  await mkdir(evidence, { recursive: true });
  browser = await chromium.launch({ headless: true, channel: 'chrome' });
  const results = [];
  for (const language of ['zh-CN', 'en-US']) {
    for (const width of [1365, 390]) {
      const context = await browser.newContext({ viewport: { width, height: 900 } });
      await context.addInitScript(value => localStorage.setItem('i18nextLng', value), language);
      const page = await context.newPage(), errors = [], reads = [];
      let changed = false, denied = false;
      page.on('pageerror', error => errors.push(error.message));
      await page.route('**/api/team/**', async route => {
        const request = route.request(), url = new URL(request.url());
        let payload;
        if (url.pathname.endsWith('/projects')) payload = { ok: true, projects: [{ key: 'demo', title: 'Customer portal', role: 'project_admin' }] };
        else if (url.pathname.endsWith('/overview')) payload = overview;
        else if (url.pathname.endsWith('/activity')) payload = { ok: true, data: { items: [], next_cursor: null, scope: 'self' } };
        else if (url.pathname.endsWith('/graph')) payload = { ok: true, project: 'demo', items: [], workstreams: [], read_at_unix_ms: at, as_of: null };
        else if (url.pathname.endsWith('/read')) {
          const query = request.postDataJSON().query;
          reads.push(query);
          let data;
          if (query.op === 'work.snapshot') data = snapshot;
          else if (query.op === 'handoff.list') data = { items: [handoff], total: 1, next_cursor: null, as_of: { at_unix_ms: at } };
          else if (query.op === 'planning.candidates') data = { items: [], next_cursor: null, as_of: { at_unix_ms: at } };
          else if (query.op === 'sources.inspect') data = { items: [], total: 0, next_cursor: null,
            collection: { complete: true, required_total: 0, checked_at_unix_ms: at }, as_of: { at_unix_ms: at } };
          else if (query.op === 'dependencies.list') {
            const current = changed ? { ...version, receipt_id: id('C') } : version;
            data = { items: [{ work_id: work, work_title: 'Build the customer portal', provider_work_id: provider,
              provider_title: 'Deliver the customer API', dependency_position: 1, version_policy: 'current_contract',
              review_assurance: { cross_workstream: { review_assurance: 'team_independent', version_policy: 'current_contract' } },
              required_version: current, available_version: current, adopted_version: current, consumed_version: version,
              adoption_valid: true, validity: changed ? 'upstream_changed' : 'valid', reason: changed ? 'execution_inputs_changed' : null,
              missing: {} }], total: 1, next_cursor: null, as_of: { at_unix_ms: at } };
          } else throw new Error(`Unexpected read ${query.op}`);
          payload = denied && query.op === 'dependencies.list'
            ? { ok: false, error: { code: 'Forbidden', message: 'private-upstream-diagnostic' } }
            : { ok: true, result: { data }, freshness };
        } else throw new Error(`Unexpected route ${url.pathname}`);
        await route.fulfill({ contentType: 'application/json', body: JSON.stringify(payload) });
      });

      async function checkProductSurface() {
        const metrics = await page.evaluate(() => ({ document: document.documentElement.scrollWidth,
          text: document.body.innerText + Array.from(document.querySelectorAll('[title],[aria-label]'))
            .map(node => `${node.getAttribute('title') ?? ''} ${node.getAttribute('aria-label') ?? ''}`).join(' ') }));
        assert.ok(metrics.document <= width, `Horizontal overflow at ${width} to ${metrics.document}`);
        for (const raw of [work, provider, stream, digest, version.receipt_id, 'contract_hash', 'current_contract', 'lease_live', 'private-upstream-diagnostic']) {
          assert.ok(!metrics.text.includes(raw), `Visible internal value ${raw}`);
        }
        assert.equal(await page.locator('details,[data-testid="technical-details"]').count(), 0);
        assert.deepEqual(errors, []);
        return metrics.document;
      }

      await page.goto(`${baseUrl}/projects/demo?tab=deps`);
      await page.getByRole('radio', { name: language === 'zh-CN' ? '依赖清单' : 'Dependencies', exact: true }).locator('..').click();
      const detail = page.getByTestId('dependency-detail');
      await detail.getByText(language === 'zh-CN' ? '与已采用成果一致' : 'Matches the adopted result', { exact: true }).waitFor();
      const dependencyShot = path.join(evidence, `obs005-dependencies-${language}-${width}.png`);
      await checkProductSurface();
      await page.evaluate(() => window.scrollTo(0, 0));
      await page.waitForFunction(() => window.scrollY === 0);
      await page.screenshot({ path: dependencyShot, fullPage: true, animations: 'disabled' });
      changed = true;
      await page.getByTestId('dependency-observations').getByRole('button', { name: language === 'zh-CN' ? '刷新记录' : 'Refresh records', exact: true }).click();
      await detail.getByText(language === 'zh-CN' ? '与当前采用的成果不同' : 'Differs from the currently adopted result', { exact: true }).waitFor();
      denied = true;
      await page.getByTestId('dependency-observations').getByRole('button', { name: language === 'zh-CN' ? '刷新记录' : 'Refresh records', exact: true }).click();
      await page.getByTestId('state-forbidden').waitFor();
      assert.equal(await detail.count(), 0);
      await page.getByTestId('project-title').waitFor();
      await checkProductSurface();

      await page.goto(`${baseUrl}/projects/demo?tab=plan&work=${work}`);
      await page.getByTestId('plan-detail').getByRole('tab', { name: language === 'zh-CN' ? '分工与交接' : 'People & handoffs', exact: true }).click();
      const people = page.getByTestId('work-responsibility');
      await people.getByText('Review assistant', { exact: true }).waitFor();
      await people.getByText(language === 'zh-CN' ? '执行交接 · 已过期' : 'Execution handoff · Expired', { exact: true }).waitFor();
      const responsibilityShot = path.join(evidence, `obs005-responsibility-${language}-${width}.png`);
      const documentWidth = await checkProductSurface();
      await page.evaluate(() => window.scrollTo(0, 0));
      await page.waitForFunction(() => window.scrollY === 0);
      await page.screenshot({ path: responsibilityShot, fullPage: true, animations: 'disabled' });
      assert.ok(reads.every(query => ['dependencies.list', 'work.snapshot', 'handoff.list', 'planning.candidates', 'sources.inspect'].includes(query.op)));
      results.push({ language, viewport: width, document_width: documentWidth, dependency_screenshot: dependencyShot,
        responsibility_screenshot: responsibilityShot, version_change_observed: true, denial_stays_local: true, read_count: reads.length });
      await context.close();
    }
  }
  console.log(JSON.stringify({ basis: 'synthetic browser fixtures', results }));
} finally {
  if (browser) await browser.close();
  server.kill('SIGTERM');
}
