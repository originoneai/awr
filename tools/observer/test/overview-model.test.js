'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const http = require('node:http');
const path = require('node:path');
const { createServer } = require('vite');
const { createTeamBridge } = require('../server/team-bridge');

const REAL_OVERVIEW = require('../src/test-fixtures/overview/overview-works-run09.json');
const REAL_REPORTS = require('../src/test-fixtures/work-state/progress-reports-run09.json');
const REAL_SNAPSHOTS = require('../src/test-fixtures/work-state/snapshots-run09.json');
const REAL_ACTIVITY = require('../src/test-fixtures/activity/service-audit-development-run09.json');
const COOKIE = 'awr_web_session=synthetic-overview-reader';
const AT = Date.now();
const STREAM = REAL_OVERVIEW.workstreams[0].id;
const WORK = REAL_OVERVIEW.works[0].key;
const REPORT = REAL_REPORTS['api-in-review-owned-by-other'];
const SNAPSHOT = REAL_SNAPSHOTS['api-in-review-owned-by-other'];

function field(value, at = AT) {
  return { value, as_of_unix_ms: value == null ? null : at, reason: null };
}

function realWorkListItem() {
  // Verbatim run09 values retained in the public extracts. The archive has no raw
  // work.list envelope (样本里没有); only its operation plumbing is synthetic.
  return {
    work_id: WORK,
    title: REAL_OVERVIEW.works[0].title,
    workstream_id: STREAM,
    summary: {
      version: 1,
      // Owner, outcome, acceptance, integration and external-wait fields are not
      // in the retained real list sample (样本里没有), so they are not invented.
      phase: field(REPORT.phase, REPORT.reported_at_unix_ms),
      runtime: field(SNAPSHOT.runtime.state),
      report: field({ origin: REPORT.origin, contract_matches_current: REPORT.contract_matches_current,
        stale: REPORT.stale }, REPORT.reported_at_unix_ms),
    },
  };
}

function envelope(state, query) {
  return {
    protocol_version: 1,
    scope_id: 'main',
    coordinator_epoch: 'synthetic-epoch',
    source_snapshot_id: 'synthetic-source',
    project_revision: state.revision,
    ...(query.workstream_id ? { workstream_id: query.workstream_id, authority_version: '1' } : {}),
    ...(query.op === 'decisions.list' ? { selection_basis: query.work_id ? 'work' : query.workstream_id ? 'explicit' : 'project' } : {}),
  };
}

function asOf(state, extra = {}) {
  return { at_unix_ms: AT, source_snapshot_id: 'synthetic-source', project_revision: state.revision, ...extra };
}

function emptyList(state, query) {
  const common = { items: [], total: 0, next_cursor: null, truncated: false, execution_authorized: false };
  if (query.op === 'decisions.list') return { ...common, work_id: null, decision_id: null, decision_state: 'all', as_of: asOf(state) };
  if (query.op === 'dependencies.list') return { ...common, work_id: null, count_scope: 'authorized_matches',
    count_limited: true, hidden_count: null, as_of: asOf(state) };
  if (query.op === 'handoff.list') return { ...common, work_id: null, recipient_person_id: null, handoff_state: 'all',
    count_scope: 'authorized_matches', count_limited: true, hidden_count: null, as_of: asOf(state, { handoffs_revision: 'a'.repeat(32) }) };
  if (query.op === 'review.rounds') return { ...common, work_id: query.work_id, as_of: asOf(state, { rounds_revision: 'b'.repeat(32) }) };
  throw new Error(`No empty-list shape for ${query.op}`);
}

function activityReply(state, { cursor = null, total = REAL_ACTIVITY.data.items.length, scope = 'project' } = {}) {
  return {
    ...envelope(state, { op: 'audit.development' }),
    data: {
      items: REAL_ACTIVITY.data.items,
      total,
      next_cursor: cursor,
      truncated: cursor !== null,
      development_source: REAL_ACTIVITY.data.development_source,
      chat_text_collected: false,
      tool_io_collected: false,
      request_retention: REAL_ACTIVITY.data.request_retention,
      scope,
      unknown_means: REAL_ACTIVITY.data.unknown_means,
      execution_authorized: false,
      as_of: asOf(state),
    },
  };
}

function defaultReply(query, state) {
  if (query.op === 'session') return { body: { session_id: 'synthetic-overview-reader', expires_at_ms: Date.now() + 3600000 } };
  if (query.op === 'directory.read') return { body: { data: {
    // No verbatim directory reply exists in the archive (样本里没有).
    project_id: 'portal', configured: false, version: '0', updated_at_ms: null,
    plan: { organization: null, departments: [], project_department_ids: [], members: [] }, members: [],
  } } };
  if (query.op === 'workstreams.list') return { body: {
    items: REAL_OVERVIEW.workstreams.map(({ id, external_key, title }) => ({ id, external_key, title })), total: 1, next_cursor: null,
  } };
  if (query.op === 'work.list') return { body: { ...envelope(state, query), data: {
    items: [realWorkListItem()], total: 1, next_cursor: null, truncated: false, execution_authorized: false, as_of: asOf(state),
  } } };
  if (query.op === 'audit.development') return { body: activityReply(state) };
  if (['decisions.list', 'dependencies.list', 'handoff.list', 'review.rounds'].includes(query.op)) {
    // These four raw list replies are not in the verbatim archive (样本里没有).
    // Empty contract-valid collections test composition without fabricating facts.
    return { body: { ...envelope(state, query), data: emptyList(state, query) } };
  }
  return { status: 500, body: { code: 'UnexpectedRead', message: 'Unexpected synthetic read' } };
}

async function modelFixture(t, respond = () => undefined, options = {}) {
  const state = { revision: '42', sessionRevoked: false, calls: [] };
  const upstream = http.createServer(async (req, res) => {
    const chunks = [];
    for await (const chunk of req) chunks.push(chunk);
    let query;
    if (req.url === '/v1/web/session') query = { op: 'session' };
    else if (req.url === '/v1/web/projects/portal/directory/read') query = { op: 'directory.read' };
    else query = JSON.parse(Buffer.concat(chunks).toString() || '{}');
    state.calls.push(query);
    if (query.op === 'session' && state.sessionRevoked) {
      res.writeHead(401, { 'content-type': 'application/json' });
      res.end(JSON.stringify({ code: 'SessionExpired', message: 'Synthetic session revoked' }));
      return;
    }
    const custom = await respond(query, state);
    const outcome = custom ?? defaultReply(query, state);
    res.writeHead(outcome.status ?? 200, { 'content-type': 'application/json' });
    res.end(JSON.stringify(outcome.body));
  });
  await new Promise((resolve) => upstream.listen(0, '127.0.0.1', resolve));
  t.after(() => new Promise((resolve) => { upstream.close(resolve); upstream.closeAllConnections(); }));
  const bridge = createTeamBridge({
    teamUrl: `http://127.0.0.1:${upstream.address().port}`, port: 0,
    readTimeoutMs: 100, overviewDeadlineMs: 1000, ...options,
  });
  const call = () => bridge.routes['GET /api/team/overview-model'](
    new URL('http://localhost/api/team/overview-model?project=portal'), null,
    { headers: { cookie: COOKIE } },
    { getHeader: () => undefined, setHeader: () => {} },
  );
  return { bridge, call, state };
}

const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

test('overview-model composes canonical facts from real extracts without inventing absent facts', async (t) => {
  const { call, state } = await modelFixture(t);
  const result = await call();
  assert.equal(result.ok, true, JSON.stringify(result));
  assert.equal(result.input.tasks.coverage, 'complete');
  assert.equal(result.input.tasks.items.length, 1);
  const task = result.input.tasks.items[0];
  assert.equal(task.id, WORK);
  assert.equal(task.title, REAL_OVERVIEW.works[0].title);
  assert.equal(task.report.completed, false);
  assert.equal(task.report.reportedAtUnixMs, REPORT.reported_at_unix_ms);
  assert.equal(task.agentName, REPORT.origin.actor_name);
  assert.equal(task.started, true);
  for (const fieldName of ['ownerPersonId', 'acceptance', 'integrated', 'externalWait', 'verifiedAtUnixMs']) {
    assert.equal(Object.hasOwn(task, fieldName), false, `${fieldName} was absent from the real extracts`);
  }
  assert.deepEqual(result.input.reviewRounds, { coverage: 'complete', items: [] });
  assert.equal(result.input.activity.items[0].id, REAL_ACTIVITY.data.items[0].id);
  assert.equal(result.read_at_unix_ms, result.input.options.nowUnixMs);
  assert.equal(result.freshness.page_read.at_unix_ms, result.read_at_unix_ms);
  assert.equal(result.freshness.authoritative.status, 'measured');
  assert.equal(result.freshness.authoritative.project_revision, '42');
  assert.equal(result.freshness.report_received.status, 'not_measured');
  assert.equal(Object.hasOwn(result.input, 'approvals'), false);
  const ops = new Set(state.calls.map((call) => call.op));
  assert.deepEqual(ops, new Set(['session', 'workstreams.list', 'work.list', 'decisions.list',
    'dependencies.list', 'handoff.list', 'audit.development', 'directory.read', 'review.rounds']));
  assert.equal(ops.has('work.snapshot'), false);
});

test('overview-model treats the real self-scoped activity reply as restricted, not project-complete', async (t) => {
  const { call } = await modelFixture(t, (query, state) => query.op === 'audit.development'
    ? { body: activityReply(state, { scope: REAL_ACTIVITY.data.scope }) } : undefined);
  const result = await call();
  assert.equal(result.ok, true, JSON.stringify(result));
  assert.equal(REAL_ACTIVITY.data.scope, 'self');
  assert.equal(result.input.activity.coverage, 'partial');
  assert.equal(result.input.activity.reason, 'permission-restricted');
  assert.equal(result.input.activity.items.length, 1);
  assert.deepEqual(result.coverage.activity.missing, [{ op: 'audit.development', reason: 'permission-restricted' }]);
  assert.equal(result.freshness.authoritative.status, 'not_measured');
});

test('overview-model re-evaluates authoritative staleness on a cache hit', async (t) => {
  let clock = AT + 100000;
  const { call, state } = await modelFixture(t, () => undefined, { now: () => clock });
  const first = await call();
  assert.equal(first.freshness.authoritative.status, 'measured');
  assert.equal(first.freshness.authoritative.age_ms, 100000);
  const reads = state.calls.filter((query) => query.op !== 'session').length;
  clock += 30000;
  const cached = await call();
  assert.equal(cached.freshness.authoritative.status, 'stale');
  assert.equal(cached.freshness.authoritative.stale_after_ms, 120000);
  assert.equal(cached.freshness.authoritative.age_ms, 130000);
  assert.equal(cached.read_at_unix_ms, first.read_at_unix_ms);
  assert.equal(state.calls.filter((query) => query.op !== 'session').length, reads);
});

test('overview-model keeps confirmed activity when a later page times out', async (t) => {
  const { call } = await modelFixture(t, async (query) => {
    if (query.op !== 'audit.development') return undefined;
    if (query.cursor) { await delay(180); return undefined; }
    return { body: activityReply({ revision: '42' }, { cursor: 'activity-page-two', total: 2 }) };
  });
  const result = await call();
  assert.equal(result.ok, true, JSON.stringify(result));
  assert.equal(result.input.tasks.coverage, 'complete');
  assert.equal(result.input.activity.coverage, 'partial');
  assert.equal(result.input.activity.reason, 'read-failed');
  assert.equal(result.input.activity.items.length, 1);
  assert.equal(result.input.activity.items[0].id, REAL_ACTIVITY.data.items[0].id);
  assert.deepEqual(result.coverage.activity.missing, [{ op: 'audit.development', reason: 'read-failed', error_code: 'ReadTimeout' }]);
  assert.equal(result.freshness.authoritative.status, 'failed');
  assert.equal(result.freshness.authoritative.error_code, 'ReadTimeout');
});

test('overview-model marks an unfinished cursor sequence truncated and retains its pages', async (t) => {
  const { call } = await modelFixture(t, (query, state) => {
    if (query.op !== 'audit.development') return undefined;
    const page = query.cursor ? Number(query.cursor.replace('activity-', '')) : 1;
    return { body: activityReply(state, { cursor: `activity-${page + 1}`, total: 1000 }) };
  });
  const result = await call();
  assert.equal(result.ok, true, JSON.stringify(result));
  assert.equal(result.input.activity.coverage, 'partial');
  assert.equal(result.input.activity.reason, 'truncated');
  assert.ok(result.input.activity.items.length > 0);
  assert.ok(result.input.activity.items.length < 1000);
  assert.equal(result.coverage.activity.missing[0].reason, 'truncated');
  assert.equal(typeof result.coverage.activity.missing[0].next_cursor, 'string');
});

test('overview-model applies authoritative ordering when responses settle out of order', async (t) => {
  let releaseOld;
  let oldStarted;
  const oldGate = new Promise((resolve) => { releaseOld = resolve; });
  const started = new Promise((resolve) => { oldStarted = resolve; });
  const { call, state } = await modelFixture(t, async (query) => {
    if (query.op === 'work.list' && state.revision === '41') {
      oldStarted();
      await oldGate;
    }
    return undefined;
  }, { readTimeoutMs: 1000, overviewDeadlineMs: 5000 });
  state.revision = '41';
  const oldRequest = call();
  await started;
  state.revision = '42';
  const newer = await call();
  assert.equal(newer.freshness.authoritative.project_revision, '42');
  state.revision = '41';
  releaseOld();
  const older = await oldRequest;
  assert.equal(older.freshness.authoritative.project_revision, '42');
  assert.equal(older.read_at_unix_ms, newer.read_at_unix_ms);
  state.revision = '42';
  const held = await call();
  assert.equal(held.freshness.authoritative.project_revision, '42');
  assert.equal(held.read_at_unix_ms, newer.read_at_unix_ms);
});

test('overview-model revocation clears the cached model and later reads return no restricted input', async (t) => {
  const { bridge, call, state } = await modelFixture(t);
  const first = await call();
  assert.equal(first.ok, true);
  assert.equal(bridge.cacheSize(), 1);
  state.sessionRevoked = true;
  const revoked = await call();
  assert.equal(revoked.ok, false);
  assert.equal(revoked.error.code, 'SessionExpired');
  assert.equal(revoked.input, undefined);
  assert.equal(bridge.cacheSize(), 0);
  const before = state.calls.filter((query) => query.op !== 'session').length;
  const again = await call();
  assert.equal(again.ok, false);
  assert.equal(state.calls.filter((query) => query.op !== 'session').length, before);
});

test('an in-flight composition cannot repopulate restricted cache after revocation', async (t) => {
  let release;
  let started;
  const gate = new Promise((resolve) => { release = resolve; });
  const waiting = new Promise((resolve) => { started = resolve; });
  const { bridge, call, state } = await modelFixture(t, async (query) => {
    if (query.op === 'work.list') { started(); await gate; }
    return undefined;
  }, { readTimeoutMs: 1000, overviewDeadlineMs: 5000 });
  const pending = call();
  await waiting;
  state.sessionRevoked = true;
  const revoked = await call();
  assert.equal(revoked.error.code, 'SessionExpired');
  release();
  const late = await pending;
  assert.equal(late.ok, false);
  assert.equal(late.error.code, 'Unauthenticated');
  assert.equal(late.input, undefined);
  assert.equal(bridge.cacheSize(), 0);
});

test('the demo composition feeds the real deriveOverview input surface directly', async (t) => {
  const bridge = createTeamBridge({ port: 0 });
  const result = await bridge.routes['GET /api/team/overview-model'](
    new URL('http://localhost/api/team/overview-model?project=orders'), null,
    { headers: { cookie: COOKIE } }, { getHeader: () => undefined, setHeader: () => {} },
  );
  assert.equal(result.ok, true, JSON.stringify(result));
  assert.ok(result.input.tasks.items.length > 0);
  assert.ok(result.input.reviewRounds.items.length > 0);
  assert.ok(result.input.dependencies.items.length > 0);
  assert.ok(result.input.decisions.items.length > 0);
  assert.ok(result.input.handoffs.items.length > 0);
  assert.ok(result.input.members.items.length > 0);
  const vite = await createServer({ configFile: false, root: path.resolve(__dirname, '..'),
    server: { middlewareMode: true }, appType: 'custom', logLevel: 'silent' });
  t.after(() => vite.close());
  const { deriveOverview } = await vite.ssrLoadModule('/src/core/overview/index.ts');
  const derived = deriveOverview(result.input);
  assert.equal(derived.tasks.length, result.input.tasks.items.length);
  assert.equal(derived.coverage.parts.tasks.state, result.input.tasks.coverage);
});
