// Synthetic browser regression for review navigation, current acceptance and product language.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { spawn } from 'node:child_process';
import { mkdir } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { chromium } from '@playwright/test';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const evidence = path.resolve(process.env.OBSERVER_EVIDENCE_DIR ?? path.join(root, '../../.local/observer-outcome-check'));
const id = (suffix) => `01J${'0'.repeat(22)}${suffix}`;
const work = id('1'), latest = id('2'), earlier = id('3'), evidenceId = id('4');
const execution = id('5'), claim = id('6'), artifact = id('7'), completion = id('8');
const contract = 'b'.repeat(64);
const at = Date.now();
const report = 'Customer filters remain selected when an earlier request finishes last. The updated behavior passed review.';
const digest = createHash('sha256').update(report).digest('hex');
const freshness = { page_read: { status: 'measured', at_unix_ms: at },
  authoritative: { status: 'measured', at_unix_ms: at }, report_received: { status: 'not_measured' } };
const snapshot = {
  work_id: work, contract_hash: contract,
  visible_contract: { acceptance: ['Keep the latest customer filter selection', 'Preserve the selected result during overlapping requests'] },
  observation: { runtime: { state: 'completed', selected_completion_id: completion },
    collaboration: { delivery_required: false, candidate: null,
      review: { id: latest, state: 'approved' }, integration: null,
      source_state: { observation: 'observed', source_synchronized: true },
      verification: [{ check: 'Race condition handling', outcome: 'passed' }], facts_truncated: false } },
};
const reviews = Object.fromEntries([earlier, latest].map(round => [round, {
  work_id: work, round_id: round, round_index: round === latest ? 2 : 1,
  state: round === latest ? 'approved' : 'rejected', contract_hash: round === latest ? contract : 'a'.repeat(64),
  evidence_id: round === latest ? evidenceId : id('9'), execution_id: round === latest ? execution : id('A'),
  evidence_provenance: { author_self_report: true, review_contract_matches_current: round === latest,
    execution_consistency: { independent_verification: false } },
  submission: { delivery_required: false, candidate_current: false, artifacts: [] },
  decisions: [{ decision: round === latest ? 'approve' : 'reject', human_approval: false,
    reason: round === latest ? `The selected result stays visible after delivery.finalize ${work}.`
      : 'An older response replaced the current selection. Keep the latest selected result.' }],
  findings: { findings: [] },
}]));
const overview = { ok: true, project: 'demo', works: [{ key: work, title: 'Preserve selected customer filters',
  status: 'completed', workstream_id: 'development', owner_person: null, agent: null, next_step: null,
  detail_loaded: false, contract_hash: contract }], workstreams: [{ id: 'development', title: 'Customer portal' }],
  coverage: { scope: { status: 'succeeded', enumerated: 1, truncated: false, next_cursor: null, read_at_unix_ms: at },
    streams: [], works_enumerated: 1, works_total: 1, succeeded: 1, failed: 0, truncated: false }, freshness };

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
      const page = await context.newPage();
      const errors = [], reads = [];
      let historyFailure = false;
      page.on('pageerror', error => errors.push(error.message));
      await page.route('**/api/team/**', async route => {
        const request = route.request(), url = new URL(request.url());
        let payload;
        if (url.pathname.endsWith('/projects')) payload = { ok: true, projects: [{ key: 'demo', title: 'Customer portal', role: 'project_admin' }] };
        else if (url.pathname.endsWith('/overview')) payload = overview;
        else if (url.pathname.endsWith('/activity')) {
          assert.equal(url.searchParams.get('work_id'), work);
          payload = { ok: true, data: { scope: 'self', next_cursor: null, items: [{ id: 'private-event',
            action: 'session.checkpoint', actor_id: id('B'), work_id: work, created_at_unix_ms: at,
            result: 'unknown' }], chat_text_collected: false, tool_io_collected: false } };
        } else if (url.pathname.endsWith('/read')) {
          const query = request.postDataJSON().query;
          reads.push(query);
          let data;
          if (query.op === 'work.snapshot') data = snapshot;
          else if (query.op === 'review.inspect') data = { review: reviews[query.review_round_id] };
          else if (query.op === 'review.rounds') {
            data = { work_id: work, items: [earlier, latest].map((round, index) => ({
              round_id: round, round_index: index + 1, round_position: index + 1, readable: true,
              state: round === latest ? 'approved' : 'rejected', contract_matches_current: round === latest,
              reviewer_role: 'agent_reviewer', decided_at_unix_ms: at,
              reason_first_line: reviews[round].decisions[0].reason,
            })), total: 2, next_cursor: null, as_of: { at_unix_ms: at } };
          } else if (query.op === 'completion.inspect') data = { selected_completion_id: completion, runtime_state: 'completed',
            completion: { receipt_id: completion, contract_hash: contract, evidence_id: evidenceId, execution_id: execution,
              approved_by: { review_round_id: latest, independence_kind: 'agent_review' } } };
          else if (query.op === 'delivery.inspect') data = { delivery: { awr_acceptance: { complete: true, runtime_state: 'completed', selected_completion_id: completion } } };
          else if (query.op === 'evidence.inspect') data = { evidence: { evidence_id: query.evidence_id,
            artifact_id: artifact, artifact_digest: digest } };
          else if (query.op === 'execution.inspect') data = { execution_id: query.execution_id, claim_id: claim };
          else if (query.op === 'claim.inspect') data = { claim_id: claim };
          else if (query.op === 'delivery.source.status') data = { source_synchronized: true, cursor_current: true, projection_current: true };
          else if (query.op === 'artifact.content') data = { artifact_id: artifact, work_id: work, text: report, sha256: digest, byte_length: Buffer.byteLength(report) };
          else if (['delivery.neutral.inspect', 'delivery.submission.describe', 'delivery.exports'].includes(query.op)) data = { work_id: work, items: [] };
          else throw new Error(`Unexpected read ${query.op}`);
          payload = query.op === 'review.rounds' && historyFailure
            ? { ok: false, error: { code: 'ReadTimeout', message: 'private-upstream-diagnostic' } }
            : { ok: true, result: { data }, freshness };
        } else throw new Error(`Unexpected route ${url.pathname}`);
        await route.fulfill({ contentType: 'application/json', body: JSON.stringify(payload) });
      });
      await page.goto(`${baseUrl}/projects/demo?tab=acceptance&work=${work}`);
      const chain = page.getByTestId('acceptance-evidence-chain');
      const completeText = language === 'zh-CN' ? '业务验收已完成' : 'Complete';
      await chain.getByText(completeText, { exact: true }).waitFor();
      await page.getByTestId('outcome-review-history').getByRole('button', { name: language === 'zh-CN' ? '第 1 次复核' : 'Review 1', exact: true }).click();
      await page.getByTestId(`acceptance-review-${earlier}`).waitFor();
      await chain.getByText(completeText, { exact: true }).waitFor();
      assert.equal(await page.getByTestId('acceptance-completion-conflict').count(), 0);
      const historyShot = path.join(evidence, `obs006-history-${language}-${width}.png`);
      await page.evaluate(() => window.scrollTo(0, 0));
      await page.waitForFunction(() => window.scrollY === 0);
      await page.screenshot({ path: historyShot, fullPage: true });
      await page.getByRole('button', { name: language === 'zh-CN' ? '返回最近一次复核' : 'Return to latest review', exact: true }).click();
      assert.equal(reads.filter(query => query.op === 'artifact.content').length, 0);
      await page.getByTestId(`artifact-expand-${artifact}`).click();
      await page.getByTestId(`artifact-read-content-${artifact}`).getByText(report, { exact: true }).waitFor();
      await page.getByTestId(`artifact-digest-${artifact}`).getByText(language === 'zh-CN' ? '内容已核对' : 'Content checked', { exact: true }).waitFor();
      historyFailure = true;
      await page.getByRole('button', { name: language === 'zh-CN' ? '刷新复核历程' : 'Refresh review history', exact: true }).click();
      await page.getByText(language === 'zh-CN' ? '复核历程暂时读取不到' : 'Review history cannot be loaded right now', { exact: true }).waitFor();
      await chain.getByText(completeText, { exact: true }).waitFor();
      const metrics = await page.evaluate(() => ({ width: window.innerWidth, document: document.documentElement.scrollWidth,
        text: document.body.innerText + Array.from(document.querySelectorAll('[aria-label], [title]')).map(el => `${el.getAttribute('aria-label') ?? ''} ${el.getAttribute('title') ?? ''}`).join(' ') }));
      assert.ok(metrics.document <= width, `Horizontal overflow at ${width} to ${metrics.document}`);
      for (const raw of [work, latest, earlier, contract, digest, execution, claim, completion, 'delivery.finalize', 'session.checkpoint', 'ReadTimeout', 'private-upstream-diagnostic']) assert.ok(!metrics.text.includes(raw), `Visible internal value ${raw}`);
      assert.deepEqual(errors, []);
      const screenshot = path.join(evidence, `obs006-${language}-${width}.png`);
      await page.evaluate(() => window.scrollTo(0, 0));
      await page.waitForFunction(() => window.scrollY === 0);
      await page.screenshot({ path: screenshot, fullPage: true });
      results.push({ language, viewport: width, document: metrics.document, history_screenshot: historyShot, screenshot, reads: reads.length });
      await context.close();
    }
  }
  console.log(JSON.stringify({ basis: 'synthetic browser fixtures', results }));
} finally {
  if (browser) await browser.close();
  server.kill('SIGTERM');
}
