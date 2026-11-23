'use strict';

const { test } = require('node:test');
const assert = require('node:assert/strict');
const http = require('node:http');
const fs = require('node:fs');
const path = require('node:path');
const { createTeamBridge, READ_OPS } = require('../server/team-bridge');

const FIXTURES_DIR = path.join(__dirname, 'fixtures');
const SAMPLES_DIR = path.resolve(__dirname, '../../../crates/awr-team-pg/tests/samples');
const LEGACY_FIXTURE_DIR = path.resolve(__dirname, '../../../tests/fixtures/workstreams/team-web-loop');

const UPDATES_LIST_SAMPLE = JSON.parse(fs.readFileSync(path.join(FIXTURES_DIR, 'updates-list.json'), 'utf8'));
const UPDATE_INSPECT_SAMPLE = JSON.parse(fs.readFileSync(path.join(FIXTURES_DIR, 'update-inspect.json'), 'utf8'));
const SAMPLE_PROJECT = UPDATES_LIST_SAMPLE.data.project_id;
const SAMPLE_UPDATE_ID = UPDATE_INSPECT_SAMPLE.data.update.update_id;

const clone = (value) => structuredClone(value);

async function startUpstream(handler) {
  const server = http.createServer(handler);
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  return {
    base: `http://127.0.0.1:${server.address().port}`,
    close: () => new Promise((resolve) => {
      server.close(resolve);
      server.closeAllConnections();
    }),
  };
}

function collectBody(request, callback) {
  const chunks = [];
  request.on('data', (chunk) => chunks.push(chunk));
  request.on('end', () => callback(JSON.parse(Buffer.concat(chunks).toString('utf8') || '{}')));
}

function jsonReply(response, body, status = 200) {
  response.writeHead(status, { 'content-type': 'application/json' });
  response.end(JSON.stringify(body));
}

function createBridgeHarness(opts) {
  const bridge = createTeamBridge({ port: 0, ...opts });
  const call = (method, target, { cookie = 'awr_web_session=synthetic-updates-reader', body = null } = {}) => {
    const url = new URL(target, 'http://127.0.0.1');
    const handler = bridge.routes[`${method} ${url.pathname}`];
    if (!handler) return Promise.resolve({ status: 404, ok: false, error: { code: 'NoRoute' } });
    const req = { headers: cookie ? { cookie } : {}, method };
    const res = { getHeader: () => undefined, setHeader: () => {} };
    return handler(url, body, req, res);
  };
  return { bridge, call };
}

test('verbatim fixtures match PPD-010/011 service samples byte for byte', () => {
  for (const name of ['updates-list.json', 'update-inspect.json']) {
    const fixtureBytes = fs.readFileSync(path.join(FIXTURES_DIR, name));
    const sampleBytes = fs.readFileSync(path.join(SAMPLES_DIR, name));
    assert.deepEqual(fixtureBytes, sampleBytes, `${name} must stay verbatim against crates/awr-team-pg/tests/samples`);
  }
});

test('updates bridge adds only read-only GET routes and refuses update.record writes', async () => {
  const { bridge, call } = createBridgeHarness({ teamUrl: 'http://127.0.0.1:1', demo: false });
  assert.equal(typeof bridge.routes['GET /api/team/updates'], 'function');
  assert.equal(typeof bridge.routes['GET /api/team/update'], 'function');
  for (const route of Object.keys(bridge.routes)) {
    if (route.includes('/updates') || route.endsWith('/update')) {
      assert.ok(route.startsWith('GET '), `update route must be read-only GET: ${route}`);
    }
  }
  for (const forbidden of [
    'POST /api/team/updates',
    'PUT /api/team/updates',
    'DELETE /api/team/updates',
    'POST /api/team/update',
    'POST /api/team/updates/record',
  ]) {
    assert.equal(bridge.routes[forbidden], undefined, forbidden);
  }
  assert.equal(READ_OPS.has('updates.list'), true);
  assert.equal(READ_OPS.has('update.inspect'), true);
  assert.equal(READ_OPS.has('update.record'), false);

  const writeAttempt = await call('POST', '/api/team/read', {
    body: { project: SAMPLE_PROJECT, query: { protocol_version: 1, op: 'update.record' } },
  });
  assert.equal(writeAttempt.ok, false);
  assert.equal(writeAttempt.error.code, 'Unsupported');
});

test('GET /api/team/updates and /api/team/update preserve verbatim PPD-010/011 replies and observation freshness', async (t) => {
  const calls = [];
  let pageTwo = clone(UPDATES_LIST_SAMPLE);
  pageTwo.data.items[0].update_id = '01M7TSKHVDC25RY7GZBC6K0ABK';
  pageTwo.data.items[0].period_key = '2026-W46';
  pageTwo.data.items[0].starts_on = '2026-11-09';
  pageTwo.data.items[0].ends_on = '2026-11-15';
  pageTwo.data.items[0].version = '1';
  pageTwo.data.items[0].supersedes_update_id = null;
  pageTwo.data.next_cursor = null;
  pageTwo.data.truncated = false;

  const upstream = await startUpstream((request, response) => {
    collectBody(request, (body) => {
      calls.push({ url: request.url, body, cookie: request.headers.cookie });
      if (body.op === 'updates.list') {
        jsonReply(response, body.cursor ? pageTwo : UPDATES_LIST_SAMPLE);
        return;
      }
      if (body.op === 'update.inspect') {
        jsonReply(response, UPDATE_INSPECT_SAMPLE);
        return;
      }
      jsonReply(response, { code: 'Unsupported' }, 400);
    });
  });
  t.after(() => upstream.close());

  let currentNow = UPDATES_LIST_SAMPLE.data.as_of.at_unix_ms + 1500;
  const { call } = createBridgeHarness({ teamUrl: upstream.base, demo: false, now: () => currentNow });

  const page1 = await call('GET', `/api/team/updates?project=${SAMPLE_PROJECT}&limit=1`);
  assert.equal(page1.ok, true, JSON.stringify(page1));
  assert.deepEqual(page1.result, UPDATES_LIST_SAMPLE);
  assert.deepEqual(page1.freshness, {
    report_received: { status: 'not_measured' },
    authoritative: {
      status: 'measured',
      stale_after_ms: 120000,
      age_ms: 1500,
      project_revision: '6',
      source_snapshot_id: UPDATES_LIST_SAMPLE.source_snapshot_id,
      coordinator_epoch: UPDATES_LIST_SAMPLE.coordinator_epoch,
      at_unix_ms: UPDATES_LIST_SAMPLE.data.as_of.at_unix_ms,
    },
    page_read: { status: 'measured', at_unix_ms: currentNow },
  });

  const cursor = encodeURIComponent(UPDATES_LIST_SAMPLE.data.next_cursor);
  const nextPage = await call('GET', `/api/team/updates?project=${SAMPLE_PROJECT}&limit=1&cursor=${cursor}`);
  assert.equal(nextPage.ok, true, JSON.stringify(nextPage));
  assert.deepEqual(nextPage.result, pageTwo);

  // Advance clock past OBSERVATION_STALE_AFTER_MS (120000ms) to verify stale currency transition.
  currentNow = UPDATE_INSPECT_SAMPLE.data.as_of.at_unix_ms + 120001;
  const inspected = await call('GET', `/api/team/update?project=${SAMPLE_PROJECT}&id=${SAMPLE_UPDATE_ID}`);
  assert.equal(inspected.ok, true, JSON.stringify(inspected));
  assert.deepEqual(inspected.result, UPDATE_INSPECT_SAMPLE);
  assert.equal(inspected.freshness.authoritative.status, 'stale');
  assert.equal(inspected.freshness.authoritative.age_ms, 120001);
  assert.equal(inspected.freshness.authoritative.stale_after_ms, 120000);
  assert.equal(inspected.freshness.authoritative.at_unix_ms, UPDATE_INSPECT_SAMPLE.data.as_of.at_unix_ms);

  assert.deepEqual(calls, [
    {
      url: `/v1/web/projects/${SAMPLE_PROJECT}/query`,
      cookie: 'awr_web_session=synthetic-updates-reader',
      body: { protocol_version: 1, op: 'updates.list', limit: 1 },
    },
    {
      url: `/v1/web/projects/${SAMPLE_PROJECT}/query`,
      cookie: 'awr_web_session=synthetic-updates-reader',
      body: { protocol_version: 1, op: 'updates.list', limit: 1, cursor: UPDATES_LIST_SAMPLE.data.next_cursor },
    },
    {
      url: `/v1/web/projects/${SAMPLE_PROJECT}/query`,
      cookie: 'awr_web_session=synthetic-updates-reader',
      body: { protocol_version: 1, op: 'update.inspect', update_id: SAMPLE_UPDATE_ID },
    },
  ]);
});

test('updates routes validate query selectors before upstream and never echo selector values', async (t) => {
  let upstreamCalls = 0;
  const upstream = await startUpstream((_request, response) => {
    upstreamCalls += 1;
    jsonReply(response, UPDATES_LIST_SAMPLE);
  });
  t.after(() => upstream.close());

  const { call } = createBridgeHarness({ teamUrl: upstream.base, demo: false });
  const sentinel = 'private-selector-sentinel';
  const invalidPaths = [
    '/api/team/updates',
    '/api/team/updates?project=',
    `/api/team/updates?project=${sentinel.repeat(6)}`,
    `/api/team/updates?project=demo&project=other`,
    `/api/team/updates?project=demo&work_id=${sentinel}`,
    `/api/team/updates?project=demo&workstream_id=${sentinel}`,
    `/api/team/updates?project=demo&session_id=${sentinel}`,
    `/api/team/updates?project=demo&update_id=${sentinel}`,
    `/api/team/updates?project=demo&cursor=`,
    `/api/team/updates?project=demo&cursor=${'x'.repeat(4097)}`,
    `/api/team/updates?project=demo&cursor=${encodeURIComponent('bad\ncursor')}`,
    '/api/team/updates?project=demo&limit=0',
    '/api/team/updates?project=demo&limit=01',
    '/api/team/updates?project=demo&limit=101',
    '/api/team/updates?project=demo&limit=1.5',
    '/api/team/updates?project=demo&max_context_bytes=0',
    '/api/team/updates?project=demo&max_context_bytes=262145',
    '/api/team/update',
    '/api/team/update?project=demo',
    '/api/team/update?id=u1',
    '/api/team/update?project=demo&id=',
    `/api/team/update?project=demo&id=${sentinel.repeat(6)}`,
    `/api/team/update?project=demo&id=${encodeURIComponent(`bad\x07${sentinel}`)}`,
    '/api/team/update?project=demo&id=u1&id=u2',
    `/api/team/update?project=demo&id=u1&cursor=${sentinel}`,
    '/api/team/update?project=demo&id=u1&limit=1',
    '/api/team/update?project=demo&id=u1&max_context_bytes=1024',
    `/api/team/update?project=demo&id=u1&extra=${sentinel}`,
  ];
  for (const target of invalidPaths) {
    const reply = await call('GET', target);
    assert.equal(reply.ok, false, target);
    assert.equal(reply.error.code, 'InvalidInput', target);
    assert.equal(reply.result, undefined, target);
    assert.equal(reply.freshness.authoritative.status, 'not_measured', target);
    assert.equal(JSON.stringify(reply).includes(sentinel), false, `must not echo selector for ${target}`);
  }
  assert.equal(upstreamCalls, 0, 'invalid selectors must never reach upstream');
});

test('updates.list contract rejects every missing field, extra field, wrong type and invariant violation', async (t) => {
  let upstreamReply;
  const upstream = await startUpstream((_request, response) => jsonReply(response, upstreamReply));
  t.after(() => upstream.close());

  const { call } = createBridgeHarness({ teamUrl: upstream.base, demo: false });
  const baseList = () => {
    const value = clone(UPDATES_LIST_SAMPLE);
    value.data.next_cursor = null;
    value.data.truncated = false;
    return value;
  };

  // Missing every single key at every level
  for (const key of Object.keys(baseList())) {
    upstreamReply = baseList();
    delete upstreamReply[key];
    const reply = await call('GET', `/api/team/updates?project=${SAMPLE_PROJECT}`);
    assert.equal(reply.ok, false, `missing envelope.${key}`);
    assert.equal(reply.error.code, 'BadGateway', `missing envelope.${key}`);
    assert.equal(reply.result, undefined);
    assert.equal(reply.freshness.authoritative.status, 'failed');
  }
  for (const key of Object.keys(baseList().data)) {
    upstreamReply = baseList();
    delete upstreamReply.data[key];
    const reply = await call('GET', `/api/team/updates?project=${SAMPLE_PROJECT}`);
    assert.equal(reply.ok, false, `missing data.${key}`);
    assert.equal(reply.error.code, 'BadGateway', `missing data.${key}`);
    assert.equal(reply.result, undefined);
  }
  for (const key of Object.keys(baseList().data.as_of)) {
    upstreamReply = baseList();
    delete upstreamReply.data.as_of[key];
    const reply = await call('GET', `/api/team/updates?project=${SAMPLE_PROJECT}`);
    assert.equal(reply.ok, false, `missing as_of.${key}`);
    assert.equal(reply.error.code, 'BadGateway', `missing as_of.${key}`);
  }
  for (const key of Object.keys(baseList().data.items[0])) {
    upstreamReply = baseList();
    delete upstreamReply.data.items[0][key];
    const reply = await call('GET', `/api/team/updates?project=${SAMPLE_PROJECT}`);
    assert.equal(reply.ok, false, `missing item.${key}`);
    assert.equal(reply.error.code, 'BadGateway', `missing item.${key}`);
  }
  for (const key of Object.keys(baseList().data.items[0].author)) {
    upstreamReply = baseList();
    delete upstreamReply.data.items[0].author[key];
    const reply = await call('GET', `/api/team/updates?project=${SAMPLE_PROJECT}`);
    assert.equal(reply.ok, false, `missing author.${key}`);
    assert.equal(reply.error.code, 'BadGateway', `missing author.${key}`);
  }

  // Extra fields at every level
  for (const [label, mutate] of [
    ['envelope extra', (r) => { r.extra = 'leak'; }],
    ['envelope workstream_id', (r) => { r.workstream_id = 'ws-1'; }],
    ['data total extra', (r) => { r.data.total = 1; }],
    ['data extra', (r) => { r.data.extra = 'leak'; }],
    ['as_of extra', (r) => { r.data.as_of.extra = 'leak'; }],
    ['item extra', (r) => { r.data.items[0].extra = 'leak'; }],
    ['author extra', (r) => { r.data.items[0].author.extra = 'leak'; }],
  ]) {
    upstreamReply = baseList();
    mutate(upstreamReply);
    const reply = await call('GET', `/api/team/updates?project=${SAMPLE_PROJECT}`);
    assert.equal(reply.ok, false, label);
    assert.equal(reply.error.code, 'BadGateway', label);
    assert.equal(reply.result, undefined, label);
    assert.equal(JSON.stringify(reply).includes('leak'), false, label);
  }

  // Wrong types and semantic invariants
  const mutations = [
    ['wrong protocol_version', (r) => { r.protocol_version = 2; }],
    ['wrong scope_id', (r) => { r.scope_id = 'other'; }],
    ['wrong selection_basis', (r) => { r.selection_basis = 'work'; }],
    ['numeric project_revision', (r) => { r.project_revision = 6; }],
    ['leading zero project_revision', (r) => { r.project_revision = '06'; }],
    ['wrong project_id', (r) => { r.data.project_id = 'other-project'; }],
    ['execution_authorized true', (r) => { r.data.execution_authorized = true; }],
    ['truncated mismatch false', (r) => { r.data.next_cursor = 'next-1'; r.data.truncated = false; }],
    ['truncated mismatch true', (r) => { r.data.next_cursor = null; r.data.truncated = true; }],
    ['short non-terminal page', (r) => { r.data.next_cursor = 'next-1'; r.data.truncated = true; }],
    ['as_of revision drift', (r) => { r.data.as_of.project_revision = '5'; }],
    ['as_of snapshot drift', (r) => { r.data.as_of.source_snapshot_id = 'other-snap'; }],
    ['as_of float timestamp', (r) => { r.data.as_of.at_unix_ms = 123.5; }],
    ['non-current item in list', (r) => { r.data.items[0].current = false; }],
    ['bad period_kind', (r) => { r.data.items[0].period_kind = 'quarter'; }],
    ['week kind with month key', (r) => { r.data.items[0].period_key = '2026-11'; }],
    ['week 54 out of bounds', (r) => { r.data.items[0].period_key = '2026-W54'; }],
    ['month kind with week key', (r) => { r.data.items[0].period_kind = 'month'; }],
    ['invalid calendar date', (r) => { r.data.items[0].starts_on = '2026-02-29'; }],
    ['ends_on before starts_on', (r) => { r.data.items[0].ends_on = '2026-11-15'; }],
    ['unknown status', (r) => { r.data.items[0].status = 'good'; }],
    ['blank summary', (r) => { r.data.items[0].summary = '   '; }],
    ['summary with control char', (r) => { r.data.items[0].summary = 'hello\nworld'; }],
    ['summary over 200 chars', (r) => { r.data.items[0].summary = 'a'.repeat(201); }],
    ['author null', (r) => { r.data.items[0].author = null; }],
    ['author string JSON', (r) => { r.data.items[0].author = '{"person_id":"a","display_name":"b"}'; }],
    ['blank author display_name', (r) => { r.data.items[0].author.display_name = '  '; }],
    ['numeric version', (r) => { r.data.items[0].version = 2; }],
    ['version 1 with supersedes', (r) => { r.data.items[0].version = '1'; }],
    ['version 2 without supersedes', (r) => { r.data.items[0].supersedes_update_id = null; }],
    ['self-superseding update', (r) => { r.data.items[0].supersedes_update_id = r.data.items[0].update_id; }],
    ['duplicate update_id', (r) => { r.data.items.push(clone(r.data.items[0])); }],
    ['ascending order instead of descending', (r) => {
      const newer = clone(r.data.items[0]);
      newer.update_id = '01M7TSKHZZZZZZZZZZZZZZZZZZ';
      newer.period_key = '2026-W48';
      newer.starts_on = '2026-11-23';
      newer.ends_on = '2026-11-29';
      r.data.items.push(newer);
    }],
  ];
  for (const [label, mutate] of mutations) {
    upstreamReply = baseList();
    mutate(upstreamReply);
    const reply = await call('GET', `/api/team/updates?project=${SAMPLE_PROJECT}`);
    assert.equal(reply.ok, false, label);
    assert.equal(reply.error.code, 'BadGateway', label);
    assert.equal(reply.result, undefined, label);
  }

  // Cursor loop refusal
  upstreamReply = clone(UPDATES_LIST_SAMPLE);
  const loopedCursor = encodeURIComponent(UPDATES_LIST_SAMPLE.data.next_cursor);
  const looped = await call('GET', `/api/team/updates?project=${SAMPLE_PROJECT}&limit=1&cursor=${loopedCursor}`);
  assert.equal(looped.ok, false);
  assert.equal(looped.error.code, 'BadGateway');
  assert.equal(looped.result, undefined);

  // Page exceeding requested limit refusal
  upstreamReply = baseList();
  const older = clone(upstreamReply.data.items[0]);
  older.update_id = '01M7TSKHVDC25RY7GZBC6K0ABK';
  older.period_key = '2026-W46';
  older.starts_on = '2026-11-09';
  older.ends_on = '2026-11-15';
  older.version = '1';
  older.supersedes_update_id = null;
  upstreamReply.data.items.push(older);
  const overLimit = await call('GET', `/api/team/updates?project=${SAMPLE_PROJECT}&limit=1`);
  assert.equal(overLimit.ok, false);
  assert.equal(overLimit.error.code, 'BadGateway');
  assert.equal(overLimit.result, undefined);

  // Valid month period_kind accepted, out-of-range month rejected
  upstreamReply = baseList();
  upstreamReply.data.items[0].period_kind = 'month';
  upstreamReply.data.items[0].period_key = '2026-11';
  upstreamReply.data.items[0].starts_on = '2026-11-01';
  upstreamReply.data.items[0].ends_on = '2026-11-30';
  const monthOk = await call('GET', `/api/team/updates?project=${SAMPLE_PROJECT}`);
  assert.equal(monthOk.ok, true, JSON.stringify(monthOk));

  upstreamReply = baseList();
  upstreamReply.data.items[0].period_kind = 'month';
  upstreamReply.data.items[0].period_key = '2026-13';
  const monthBad = await call('GET', `/api/team/updates?project=${SAMPLE_PROJECT}`);
  assert.equal(monthBad.ok, false);
  assert.equal(monthBad.error.code, 'BadGateway');
});

test('update.inspect contract rejects missing/extra/bad-type fields and accepts valid schema-64 null and schema-65 snapshots', async (t) => {
  let upstreamReply;
  const upstream = await startUpstream((_request, response) => jsonReply(response, upstreamReply));
  t.after(() => upstream.close());

  const { call } = createBridgeHarness({ teamUrl: upstream.base, demo: false });

  // Schema-64 row without snapshot: fact_snapshot_ref=null, fact_snapshot=null, fact_snapshot_coverage=null
  upstreamReply = clone(UPDATE_INSPECT_SAMPLE);
  upstreamReply.data.update.fact_snapshot_ref = null;
  upstreamReply.data.fact_snapshot = null;
  upstreamReply.data.fact_snapshot_coverage = null;
  const legacyOk = await call('GET', `/api/team/update?project=${SAMPLE_PROJECT}&id=${SAMPLE_UPDATE_ID}`);
  assert.equal(legacyOk.ok, true, JSON.stringify(legacyOk));
  assert.equal(legacyOk.result.data.fact_snapshot, null);
  assert.equal(legacyOk.result.data.fact_snapshot_coverage, null);

  // Valid schema-65 snapshot with non-empty waits, duplicate decisions_made work_ids, and hidden works
  upstreamReply = clone(UPDATE_INSPECT_SAMPLE);
  upstreamReply.data.fact_snapshot.decisions_made = { count: 2, work_ids: ['a', 'a'] };
  upstreamReply.data.fact_snapshot.waits_opened = {
    count: 3,
    kinds: [
      { kind: 'decision', count: 2, work_ids: ['a', 'a'] },
      { kind: 'external', count: 1, work_ids: ['b'] },
    ],
  };
  upstreamReply.data.fact_snapshot_coverage = { complete: false, hidden_work_count: 2 };
  const richOk = await call('GET', `/api/team/update?project=${SAMPLE_PROJECT}&id=${SAMPLE_UPDATE_ID}`);
  assert.equal(richOk.ok, true, JSON.stringify(richOk));

  // Missing every key in data, fact_snapshot, fact_snapshot_coverage
  for (const key of Object.keys(UPDATE_INSPECT_SAMPLE.data)) {
    upstreamReply = clone(UPDATE_INSPECT_SAMPLE);
    delete upstreamReply.data[key];
    const reply = await call('GET', `/api/team/update?project=${SAMPLE_PROJECT}&id=${SAMPLE_UPDATE_ID}`);
    assert.equal(reply.ok, false, `missing data.${key}`);
    assert.equal(reply.error.code, 'BadGateway', `missing data.${key}`);
    assert.equal(reply.result, undefined);
  }
  for (const key of Object.keys(UPDATE_INSPECT_SAMPLE.data.fact_snapshot)) {
    upstreamReply = clone(UPDATE_INSPECT_SAMPLE);
    delete upstreamReply.data.fact_snapshot[key];
    const reply = await call('GET', `/api/team/update?project=${SAMPLE_PROJECT}&id=${SAMPLE_UPDATE_ID}`);
    assert.equal(reply.ok, false, `missing fact_snapshot.${key}`);
    assert.equal(reply.error.code, 'BadGateway', `missing fact_snapshot.${key}`);
  }
  for (const key of Object.keys(UPDATE_INSPECT_SAMPLE.data.fact_snapshot_coverage)) {
    upstreamReply = clone(UPDATE_INSPECT_SAMPLE);
    delete upstreamReply.data.fact_snapshot_coverage[key];
    const reply = await call('GET', `/api/team/update?project=${SAMPLE_PROJECT}&id=${SAMPLE_UPDATE_ID}`);
    assert.equal(reply.ok, false, `missing fact_snapshot_coverage.${key}`);
    assert.equal(reply.error.code, 'BadGateway', `missing fact_snapshot_coverage.${key}`);
  }

  // Extra fields and type/invariant violations
  const mutations = [
    ['extra envelope field', (r) => { r.extra = true; }],
    ['extra data field', (r) => { r.data.extra = true; }],
    ['extra update field', (r) => { r.data.update.extra = true; }],
    ['extra fact_snapshot field', (r) => { r.data.fact_snapshot.extra = { count: 0, work_ids: [] }; }],
    ['extra metric field', (r) => { r.data.fact_snapshot.verified.extra = 1; }],
    ['extra wait metric field', (r) => { r.data.fact_snapshot.waits_opened.extra = 1; }],
    ['extra wait kind field', (r) => {
      r.data.fact_snapshot.waits_opened = { count: 1, kinds: [{ kind: 'review', count: 1, work_ids: ['a'], extra: 1 }] };
    }],
    ['extra coverage field', (r) => { r.data.fact_snapshot_coverage.extra = 1; }],
    ['wrong project_id', (r) => { r.data.project_id = 'wrong-project'; }],
    ['wrong update_id echo', (r) => { r.data.update.update_id = 'other-update'; }],
    ['execution_authorized true', (r) => { r.data.execution_authorized = true; }],
    ['null ref with non-null snapshot', (r) => { r.data.update.fact_snapshot_ref = null; }],
    ['non-null ref with null snapshot', (r) => { r.data.fact_snapshot = null; }],
    ['non-null ref with null coverage', (r) => { r.data.fact_snapshot_coverage = null; }],
    ['verified count mismatch', (r) => { r.data.fact_snapshot.verified.count = 2; }],
    ['verified duplicate work_id', (r) => { r.data.fact_snapshot.verified = { count: 2, work_ids: ['a', 'a'] }; }],
    ['verified unsorted work_ids', (r) => { r.data.fact_snapshot.verified = { count: 2, work_ids: ['b', 'a'] }; }],
    ['wait total mismatch', (r) => {
      r.data.fact_snapshot.waits_opened = { count: 2, kinds: [{ kind: 'review', count: 1, work_ids: ['a'] }] };
    }],
    ['wait kind zero count entry', (r) => {
      r.data.fact_snapshot.waits_opened = { count: 0, kinds: [{ kind: 'review', count: 0, work_ids: [] }] };
    }],
    ['wait duplicate kind', (r) => {
      r.data.fact_snapshot.waits_opened = {
        count: 2,
        kinds: [
          { kind: 'review', count: 1, work_ids: ['a'] },
          { kind: 'review', count: 1, work_ids: ['b'] },
        ],
      };
    }],
    ['unknown wait kind', (r) => {
      r.data.fact_snapshot.waits_opened = { count: 1, kinds: [{ kind: 'blocker', count: 1, work_ids: ['a'] }] };
    }],
    ['coverage complete true with hidden works', (r) => {
      r.data.fact_snapshot_coverage = { complete: true, hidden_work_count: 1 };
    }],
    ['coverage complete false with zero hidden', (r) => {
      r.data.fact_snapshot_coverage = { complete: false, hidden_work_count: 0 };
    }],
    ['coverage negative hidden count', (r) => {
      r.data.fact_snapshot_coverage = { complete: false, hidden_work_count: -1 };
    }],
  ];
  for (const [label, mutate] of mutations) {
    upstreamReply = clone(UPDATE_INSPECT_SAMPLE);
    mutate(upstreamReply);
    const reply = await call('GET', `/api/team/update?project=${SAMPLE_PROJECT}&id=${SAMPLE_UPDATE_ID}`);
    assert.equal(reply.ok, false, label);
    assert.equal(reply.error.code, 'BadGateway', label);
    assert.equal(reply.result, undefined, label);
    assert.equal(reply.freshness.authoritative.status, 'failed', label);
  }
});

test('updates routes enforce revocation without retaining data, clear session cache, and propagate cursor/timeout failures', async (t) => {
  let behavior = { status: 200, body: UPDATES_LIST_SAMPLE };
  let stall = false;
  const upstream = await startUpstream((request, response) => {
    if (stall) return;
    if (request.url === '/v1/web/session') {
      jsonReply(response, { ok: true, session_id: 'synthetic-updates-reader', expires_at_ms: Date.now() + 60000 });
      return;
    }
    collectBody(request, (body) => {
      if (body.op === 'workstreams.list') {
        jsonReply(response, { items: [], total: 0, next_cursor: null });
        return;
      }
      if (body.op === 'capabilities') {
        jsonReply(response, { protocol: 'awr-team-workstream', protocol_version: 1, identity: { actor_id: 'a', role: 'observer' } });
        return;
      }
      jsonReply(response, behavior.body, behavior.status);
    });
  });
  t.after(() => upstream.close());

  const { bridge, call } = createBridgeHarness({ teamUrl: upstream.base, demo: false, readTimeoutMs: 120 });

  for (const [status, code] of [
    [401, 'Unauthenticated'],
    [401, 'SessionExpired'],
    [403, 'Forbidden'],
  ]) {
    // Seed session overview cache, then trigger auth failure on /api/team/updates and verify cache is cleared.
    const overview = await call('GET', `/api/team/overview?project=${SAMPLE_PROJECT}`);
    assert.equal(overview.ok, true, code);
    assert.ok(bridge.cacheSize() > 0, `expected session cache populated before ${code}`);

    behavior = { status, body: { code, message: 'denied' } };
    const listReply = await call('GET', `/api/team/updates?project=${SAMPLE_PROJECT}`);
    assert.equal(listReply.ok, false, code);
    assert.equal(listReply.error.code, code, code);
    assert.equal(listReply.result, undefined, code);
    assert.equal(listReply.freshness.authoritative.status, 'failed', code);
    assert.equal(listReply.freshness.authoritative.error_code, code, code);
    assert.equal(bridge.cacheSize(), 0, `session cache must be cleared immediately on ${code}`);

    const inspectReply = await call('GET', `/api/team/update?project=${SAMPLE_PROJECT}&id=${SAMPLE_UPDATE_ID}`);
    assert.equal(inspectReply.ok, false, code);
    assert.equal(inspectReply.error.code, code, code);
    assert.equal(inspectReply.result, undefined, code);
  }

  // Non-auth CursorExpired preserves error code cleanly
  behavior = { status: 409, body: { code: 'CursorExpired', message: 'cursor expired' } };
  const expiredCursor = await call('GET', `/api/team/updates?project=${SAMPLE_PROJECT}`);
  assert.equal(expiredCursor.ok, false);
  assert.equal(expiredCursor.error.code, 'CursorExpired');
  assert.equal(expiredCursor.result, undefined);
  assert.equal(expiredCursor.freshness.authoritative.status, 'failed');

  // Direct POST /api/team/read refuses unsupported selectors on update.inspect
  const badBudget = await call('POST', '/api/team/read', {
    body: {
      project: SAMPLE_PROJECT,
      query: { protocol_version: 1, op: 'update.inspect', update_id: SAMPLE_UPDATE_ID, max_context_bytes: 1024 },
    },
  });
  assert.equal(badBudget.ok, false);
  assert.equal(badBudget.error.code, 'InvalidInput');

  stall = true;
  const timedOut = await call('GET', `/api/team/updates?project=${SAMPLE_PROJECT}`);
  assert.equal(timedOut.ok, false);
  assert.equal(timedOut.error.code, 'ReadTimeout');
  assert.equal(timedOut.result, undefined);
  assert.equal(timedOut.freshness.authoritative.status, 'failed');
});

test('demo mode serves synthetic period updates through the exact contract and legacy fixture dir stays unsupported', async () => {
  const fixedNow = 1795122000000;
  for (const demoVariant of ['full', 'canonical']) {
    const { call } = createBridgeHarness({ demo: true, demoVariant, now: () => fixedNow });
    const list = await call('GET', '/api/team/updates?project=P-001');
    assert.equal(list.ok, true, `${demoVariant}: ${JSON.stringify(list)}`);
    assert.equal(list.result.data.items.length, 3);
    assert.equal(list.result.data.truncated, false);
    assert.equal(list.result.data.next_cursor, null);
    assert.equal('total' in list.result.data, false, 'demo updates.list must match real PPD-010 keys with no extra total');
    assert.equal(list.freshness.authoritative.status, 'measured');
    assert.equal(list.freshness.authoritative.at_unix_ms, fixedNow);

    // Pagination works end-to-end
    const firstPage = await call('GET', '/api/team/updates?project=P-001&limit=1');
    assert.equal(firstPage.ok, true);
    assert.equal(firstPage.result.data.items.length, 1);
    assert.equal(firstPage.result.data.truncated, true);
    assert.ok(firstPage.result.data.next_cursor);
    const secondPage = await call('GET', `/api/team/updates?project=P-001&limit=1&cursor=${encodeURIComponent(firstPage.result.data.next_cursor)}`);
    assert.equal(secondPage.ok, true);
    assert.equal(secondPage.result.data.items.length, 1);
    assert.notEqual(secondPage.result.data.items[0].update_id, firstPage.result.data.items[0].update_id);

    // Every synthetic update passes update.inspect contract with PPD-011 fact snapshot
    for (const item of list.result.data.items) {
      const detail = await call('GET', `/api/team/update?project=P-001&id=${encodeURIComponent(item.update_id)}`);
      assert.equal(detail.ok, true, `${demoVariant} ${item.update_id}: ${JSON.stringify(detail)}`);
      assert.equal(detail.result.data.update.update_id, item.update_id);
      assert.ok(detail.result.data.fact_snapshot);
      assert.deepEqual(detail.result.data.fact_snapshot_coverage, { complete: true, hidden_work_count: 0 });
    }
    const u1 = await call('GET', '/api/team/update?project=P-001&id=U-001');
    assert.equal(u1.result.data.fact_snapshot.verified.count, 2);
    assert.equal(u1.result.data.fact_snapshot.waits_opened.count, 6);
    assert.equal(u1.result.data.fact_snapshot.review_returned.count, 1);
    assert.equal(u1.result.data.fact_snapshot.delivered_unverified.count, 1);

    // Foreign or unknown update returns Forbidden
    const foreign = await call('GET', '/api/team/update?project=P-001&id=U-004');
    assert.equal(foreign.ok, false);
    assert.equal(foreign.error.code, 'Forbidden');
  }

  // Restricted variant refuses P-003
  const restricted = createBridgeHarness({ demo: true, demoVariant: 'restricted' });
  assert.equal((await restricted.call('GET', '/api/team/updates?project=P-003')).error.code, 'Forbidden');

  // Empty variant refuses unknown project
  const empty = createBridgeHarness({ demo: true, demoVariant: 'empty' });
  assert.equal((await empty.call('GET', '/api/team/updates?project=P-001')).error.code, 'Forbidden');

  // Legacy fixture directory without demo organization does not fabricate update reads
  const legacy = createBridgeHarness({ teamFixtureDir: LEGACY_FIXTURE_DIR, demo: true });
  assert.equal((await legacy.call('GET', '/api/team/updates?project=demo')).error.code, 'Unsupported');
  assert.equal((await legacy.call('GET', '/api/team/update?project=demo&id=u1')).error.code, 'Unsupported');
});
