'use strict';

const { test } = require('node:test');
const assert = require('node:assert/strict');
const http = require('http');
const fs = require('fs');
const path = require('path');
const { createTeamBridge } = require('../../observer/server/team-bridge');

const FIXTURE_DIR = path.resolve(__dirname, '../../../tests/fixtures/workstreams/team-web-loop');
const GUARD = { 'x-awr-inspector': '1', origin: 'http://127.0.0.1' };

function coordinationPage(items = [], next = null) {
  return { protocol_version: 1, scope_id: 'main', project_revision: '20', source_snapshot_id: 'source', coordinator_epoch: 'epoch',
    data: { identity: { actor_id: 'member', client_id: 'member-agent', role: 'reviewer' }, items, next_cursor: next,
      state_basis: 'current_persistent_facts', deduplicate_by: 'item_key', refresh_from_first_page_on_change: true, execution_authorized: false } };
}
function coordinationItem(op = 'review.inspect') {
  const query = { protocol_version: 1, op, work_id: 'WORK', workstream_id: 'stream',
    ...(op === 'review.inspect' ? { review_round_id: 'round' } : op === 'delivery.integration.inspect' ? { request_id: 'integration' } : {}) };
  return { item_key: 'semantic-key', work_id: 'WORK', workstream_id: 'stream', contract_hash: 'requirements',
    title: 'Review the integration', title_truncated: false, next_query: query,
    guidance: { code: 'review', when: 'Review is open', because: ['The current round needs a decision'],
      action: { op, query }, recheck_on: 'Requirements, review or permissions change' } };
}

function startBridge() {
  const bridge = createTeamBridge({ teamFixtureDir: FIXTURE_DIR, port: 0 });
  const server = http.createServer(async (req, res) => {
    const url = new URL(req.url, 'http://127.0.0.1');
    const key = `${req.method} ${url.pathname}`;
    const handler = bridge.routes[key];
    if (!handler) {
      res.writeHead(404, { 'content-type': 'application/json' });
      res.end('{}');
      return;
    }
    let body = null;
    if (req.method === 'POST') {
      const chunks = [];
      for await (const c of req) chunks.push(c);
      const raw = Buffer.concat(chunks).toString('utf8');
      body = raw ? JSON.parse(raw) : {};
    }
    try {
      const json = await handler(url, body, req, res);
      const cookie = res.getHeader('set-cookie');
      const headers = { 'content-type': 'application/json' };
      if (cookie) headers['set-cookie'] = cookie;
      res.writeHead(200, headers);
      res.end(JSON.stringify(json));
    } catch (err) {
      res.writeHead(500, { 'content-type': 'application/json' });
      res.end(JSON.stringify({ ok: false, error: { message: String(err.message || err) } }));
    }
  });
  return new Promise((resolve) => {
    server.listen(0, '127.0.0.1', () => {
      const { port } = server.address();
      resolve({
        base: `http://127.0.0.1:${port}`,
        close: () => new Promise((r) => server.close(r)),
        bridge,
      });
    });
  });
}

async function req(base, method, urlPath, { body, headers, cookie } = {}) {
  const res = await fetch(base + urlPath, {
    method,
    headers: {
      ...GUARD,
      ...(headers || {}),
      ...(cookie ? { cookie } : {}),
      ...(body ? { 'content-type': 'application/json' } : {}),
    },
    body: body ? JSON.stringify(body) : undefined,
  });
  const setCookie = typeof res.headers.getSetCookie === 'function' ? res.headers.getSetCookie() : [];
  const json = await res.json();
  return { status: res.status, json, setCookie };
}

test('team-web module double-submit guard', async () => {
  const teamWeb = require('../public/team-web.js');
  const calls = [];
  const api = teamWeb.createTeamWeb({
    i18n: { t: (k) => k },
    $: () => null,
    callApi: async () => ({ ok: true }),
  });
  let resolveGate;
  const gate = new Promise((r) => { resolveGate = r; });
  let started = 0;
  const run = api._guardDouble('act', async () => {
    started += 1;
    calls.push('start');
    await gate;
    calls.push('end');
  });
  const p1 = run();
  const p2 = run(); // should no-op while inflight
  resolveGate();
  await Promise.all([p1, p2]);
  assert.equal(started, 1);
});

test('unresolved execution guidance renders the current viewer authority tri-state in both locales', () => {
  const { install } = require('./fixtures/dom-stub');
  const { createTeamWeb } = require('../public/team-web');
  const catalogs = {
    en: require('../public/locales/en'),
    'zh-CN': require('../public/locales/zh-CN'),
  };
  const keys = {
    true: 'feedback.reconciliation_authority_yes',
    false: 'feedback.reconciliation_authority_no',
    missing: 'feedback.reconciliation_authority_unknown',
  };
  for (const catalog of Object.values(catalogs)) {
    const { document } = install();
    const ui = createTeamWeb({ i18n: { t: key => catalog[key] || key }, $: id => document.getElementById(id) });
    const work = { key: 'WORK', title: 'Work', workstream_id: 'stream', detail_loaded: true,
      status: 'in_progress', recovery_blocked: false, depends_on: [], acceptance: [],
      execution: { state: 'unknown', recovery_blocked: false } };
    Object.assign(ui.state, { session: { session_id: 'session' }, projectKey: 'project', selected: 'WORK', works: [work] });
    for (const [label, value] of [['true', true], ['false', false], ['missing', undefined]]) {
      if (value === undefined) delete work.execution.reconciliation_authority;
      else work.execution.reconciliation_authority = value;
      ui.render();
      const text = document.getElementById('teamDetail').textContent;
      assert.ok(text.includes(catalog[keys[label]]), `${label} authority is shown`);
      for (const other of Object.values(keys).filter(key => key !== keys[label]))
        assert.ok(!text.includes(catalog[other]), `${label} does not collapse into ${other}`);
    }
    work.execution.state = 'succeeded';
    work.execution.reconciliation_authority = true;
    ui.render();
    assert.ok(!document.getElementById('teamDetail').textContent.includes(catalog[keys.true]),
      'authority is hidden when the outcome is settled and recovery is not blocked');
    work.recovery_blocked = true;
    ui.render();
    assert.ok(!document.getElementById('teamDetail').textContent.includes(catalog[keys.true]),
      'a work-level block does not attribute recovery to a settled execution');
    work.execution.state = 'unknown';
    work.execution.recovery_cause = 'unattributed_or_other_execution';
    ui.render();
    assert.ok(!document.getElementById('teamDetail').textContent.includes(catalog[keys.true]),
      'a block attributed elsewhere does not direct recovery to this execution');
    work.execution.recovery_cause = 'attributed_execution_report';
    work.execution.controlled_confirmation_available = true;
    work.execution.reconciliation_authority = false;
    ui.render();
    assert.ok(!document.getElementById('teamDetail').textContent.includes(catalog[keys.false]),
      'controlled confirmation is not contradicted by reconciliation advice');
  }
});

test('reconciliation coverage copy is localized and shown only for uncertain recovery', () => {
  const { reconciliationCoverageMessageKey } = require('../public/team-web.js');
  const en = require('../public/locales/en.js');
  const zh = require('../public/locales/zh-CN.js');
  for (const status of ['covered', 'insufficient', 'none', 'unsupported', 'unknown']) {
    const key = `feedback.reconciliation_coverage_${status}`;
    assert.equal(reconciliationCoverageMessageKey({ execution: { state: 'unknown',
      reconciliation_coverage: { status } } }), key);
    assert.ok(en[key]); assert.ok(zh[key]);
    assert.ok(!/actor_id|client_id|secret/i.test(en[key] + zh[key]));
  }
  assert.equal(reconciliationCoverageMessageKey({ attention: 'recovery_required', execution: {
    state: 'succeeded', recovery_blocked: true, reconciliation_coverage: { status: 'none' },
  } }), 'feedback.reconciliation_coverage_none');
  assert.equal(reconciliationCoverageMessageKey({ execution: { state: 'succeeded',
    reconciliation_coverage: { status: 'none' } } }), null);
  assert.equal(reconciliationCoverageMessageKey({ execution: { state: 'unknown',
    reconciliation_coverage: { status: 'unexpected' } } }), 'feedback.reconciliation_coverage_unknown');
  assert.equal(reconciliationCoverageMessageKey({ attention: 'recovery_required', execution: null,
    reconciliation_coverage: { status: 'none' } }), 'feedback.reconciliation_coverage_none');
  assert.ok(en['ui.network_reconciliation_coverage']);
  assert.ok(zh['ui.network_reconciliation_coverage']);
});

test('blocked work without an execution displays recovery coverage', () => {
  const { install } = require('./fixtures/dom-stub');
  const { createTeamWeb } = require('../public/team-web');
  const catalog = require('../public/locales/en');
  const { document } = install();
  const ui = createTeamWeb({ i18n: { t: key => catalog[key] || key }, $: id => document.getElementById(id) });
  const work = { key: 'WORK', title: 'Work', workstream_id: 'stream', detail_loaded: true,
    status: 'in_progress', attention: 'recovery_required', depends_on: [], acceptance: [],
    execution: null, reconciliation_coverage: { status: 'none' } };
  Object.assign(ui.state, { session: { session_id: 'session' }, projectKey: 'project', selected: 'WORK', works: [work] });
  ui.render();
  assert.ok(document.getElementById('teamDetail').textContent.includes(catalog['feedback.reconciliation_coverage_none']));
});

function startMockUpstream(handler) {
  const server = http.createServer((req, res) => handler(req, res));
  return new Promise((resolve) => {
    server.listen(0, '127.0.0.1', () => {
      const { port } = server.address();
      resolve({
        base: `http://127.0.0.1:${port}`,
        close: () => new Promise((r) => server.close(r)),
        port,
      });
    });
  });
}

function startLiveBridge(teamUrl, options = {}) {
  const bridge = createTeamBridge({ teamUrl, teamFixtureDir: FIXTURE_DIR, port: 0, ...options });
  const server = http.createServer(async (req, res) => {
    const url = new URL(req.url, 'http://127.0.0.1');
    const key = `${req.method} ${url.pathname}`;
    const handler = bridge.routes[key];
    if (!handler) {
      res.writeHead(404, { 'content-type': 'application/json' });
      res.end('{}');
      return;
    }
    let body = null;
    if (req.method === 'POST') {
      const chunks = [];
      for await (const c of req) chunks.push(c);
      const raw = Buffer.concat(chunks).toString('utf8');
      body = raw ? JSON.parse(raw) : {};
    }
    try {
      const json = await handler(url, body, req, res);
      const cookie = res.getHeader('set-cookie');
      const headers = { 'content-type': 'application/json' };
      if (cookie) headers['set-cookie'] = cookie;
      res.writeHead(200, headers);
      res.end(JSON.stringify(json));
    } catch (err) {
      res.writeHead(500, { 'content-type': 'application/json' });
      res.end(JSON.stringify({ ok: false, error: { message: String(err.message || err) } }));
    }
  });
  return new Promise((resolve) => {
    server.listen(0, '127.0.0.1', () => {
      const { port } = server.address();
      resolve({
        base: `http://127.0.0.1:${port}`,
        close: () => new Promise((r) => server.close(r)),
        bridge,
      });
    });
  });
}


function atomicWorkFixture() {
  const runtime = { state: 'in_progress', work_version: '7', last_fence: '3', recovery_blocked: false, selected_completion_id: null };
  return { project_revision: '20', source_snapshot_id: 'source-snapshot', coordinator_epoch: 'epoch', workstream_id: 'stream', data: {
    work_id: 'WORK', contract_hash: 'current', runtime: { ...runtime }, context_complete: true,
    visible_contract: { acceptance: ['Check the result'], required_dependencies: [] },
    snapshot: { version: 1, consistency: 'repeatable_read', project_revision: '20', source_snapshot_id: 'source-snapshot',
      coordinator_epoch: 'epoch', queried_at_unix_ms: 1234 },
    observation: { work_id: 'WORK', contract_hash: 'current', observed_at_unix_ms: 1234, runtime: { ...runtime },
      session: { id: 'session', actor_name: 'Member', client_id: 'client' },
      progress: { phase: 'delivered', summary: 'Implementation submitted', reported_at_unix_ms: 1000,
        reported_contract_hash: 'current', recorded_project_revision: '19', client_observed_at_unix_ms: null, provenance: 'caller_declared' },
      execution: { state: 'succeeded', terminal_reported: true, artifact_verified: false, effects_settled: true,
        settlement_basis: 'caller_asserted', settlement_scope: 'admitted_workspace_paths' },
      guidance: { code: 'inspect_delivery', action: { note: 'Inspect the current candidate' } }, pr_deliveries: [],
      collaboration: { candidate: { digest: 'neutral-binding', current: true, selection_version: '2', required_checks: ['search'] },
        verification: [{ check: 'search', run_id: 'check-run', outcome: 'passed' }], review: { id: 'round', state: 'open' },
        integration: { id: 'original-request', state: 'unknown' }, publication: { id: 'publication', phase: 'pending' },
        facts_truncated: false, acceptance_inferred: false, source_synchronized: false },
    },
  } };
}


for (const revoked of [false, true]) test(`the complete workspace refresh ${revoked ? 'clears changed' : 'retains unchanged'} protected member state`, async () => {
  const { install } = require('./fixtures/dom-stub'); install();
  const { createTeamWeb } = require('../public/team-web');
  const i18n = require('../public/i18n'); i18n.setLocale('en');
  const node = id => document.getElementById(id);
  const button = (id, text) => node(id).find(n => n.tagName === 'BUTTON' && n.textContent === text);
  const identity = { actor_id: 'manager', client_id: 'manager-client', role: 'project_admin', can_manage_members: true, can_read_project_audit: true };
  const view = { ok: true, works: [], identity, mcp_url: 'https://team.example/v1/projects/example/mcp' };
  let waitForProjects = null, applyPayload = null;
  const previousFetch = global.fetch;
  global.fetch = async (url, options) => {
    let body;
    if (url.includes('/projects?')) body = waitForProjects ? await waitForProjects : { ok: true, projects: [{ key: 'example', title: 'Example' }], session: { session_id: 'web-session' } };
    else if (url.includes('/overview?')) body = view;
    else {
      const { operation, payload } = JSON.parse(options.body);
      if (operation === 'inspect') body = { ok: true, data: { items: [{ actor_id: 'member', kind: 'human', display_name: 'Member', role: 'developer',
        independent_review: false, agent_review: false, clients: [{ client_id: 'member-agent', grants: [{ workstream_id: 'stream', authority_version: '1', read: true, write: true, manage: false, active: true }], credentials: [] }] }], workstreams: [], next_cursor: null } };
      else if (operation === 'preview') body = { ok: true, data: { applied: false, state_digest: 'before', plan_digest: 'plan', desired: payload.plan } };
      else {
        applyPayload = payload;
        body = { ok: true, data: { replayed: false, receipt: { protocol: 'awr-project-admin-access-v1', request_id: payload.request_id,
          admin_actor_id: identity.actor_id, admin_client_id: identity.client_id, subject_actor_id: payload.plan.subject.id,
          subject_client_id: payload.plan.subject_client_id, before_digest: payload.expected_state, plan_digest: payload.expected_plan,
          after_digest: 'after', project_revision: '2', desired: payload.plan } } };
      }
    }
    return { ok: true, json: async () => body };
  };
  try {
    const ui = createTeamWeb({ i18n, $: node }); await ui.refresh();
    await button('teamConsoleTabs', 'Members').click(); await button('teamConsolePanel', 'Generate connection instructions').click();
    const text = node('teamConsolePanel').find(n => n.tagName === 'TEXTAREA').value;
    let resolve; waitForProjects = new Promise(r => { resolve = r; });
    const refreshing = ui.refresh();
    assert.equal(node('teamConsolePanel').hidden, true); assert.equal(node('teamConsolePanel').find(n => n.tagName === 'TEXTAREA'), null);
    if (revoked) view.identity = { ...identity, can_read_project_audit: false };
    resolve({ ok: true, projects: [{ key: 'example' }], session: { session_id: 'web-session' } }); await refreshing;
    const area = node('teamConsolePanel').find(n => n.tagName === 'TEXTAREA');
    if (revoked) assert.equal(area, null); else assert.equal(area.value, text);
    assert.ok(applyPayload); assert.ok(!JSON.stringify(ui.state).includes('awr1.'));
  } finally { global.fetch = previousFetch; }
});

test('i18n keys for team web exist in en and zh-CN', () => {
  const enText = fs.readFileSync(path.join(__dirname, '../public/locales/en.js'), 'utf8');
  const zhText = fs.readFileSync(path.join(__dirname, '../public/locales/zh-CN.js'), 'utf8');
  for (const key of [
    'ui.team_web',
    'ui.my_projects',
    'ui.blocker_detail',
    'ui.accept_responsibility',
    'ui.hidden_dep_hint_p0',
  ]) {
    assert.ok(enText.includes('"' + key + '"'), key + ' en');
    assert.ok(zhText.includes('"' + key + '"'), key + ' zh');
  }
});
