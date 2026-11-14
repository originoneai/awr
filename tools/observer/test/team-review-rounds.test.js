'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const http = require('node:http');
const { createTeamBridge } = require('../server/team-bridge');
const at = 1793534400000;
const digest = 'a'.repeat(64);
const fields = ['state', 'bundle_hash', 'contract_hash', 'contract_matches_current', 'artifact_digest',
  'reviewer_role', 'role_basis', 'reviewer_actor_id', 'reviewer_person_id', 'decision',
  'decided_at_unix_ms', 'reason_first_line'];

function readableRound(position = 1) {
  const item = { round_id: `review-${position}`, round_index: position * 2, round_position: position,
    readable: true, reason: null, state: 'approved', bundle_hash: digest, contract_hash: digest,
    contract_matches_current: true, artifact_digest: digest, reviewer_role: 'independent_reviewer',
    role_basis: 'recorded_approval_basis', reviewer_actor_id: 'reviewer', reviewer_person_id: null,
    decision: 'approve', decided_at_unix_ms: at, reason_first_line: 'Requirements verified', missing: {} };
  item.missing = Object.fromEntries(fields.map((field) => [field, item[field] === null ? 'not_recorded' : null]));
  return item;
}

function restrictedRound(position = 1) {
  return { round_id: `review-${position}`, round_index: position * 2, round_position: position,
    readable: false, reason: 'not_in_review_work_scope',
    ...Object.fromEntries(fields.map((field) => [field, null])),
    missing: Object.fromEntries(fields.map((field) => [field, 'not_in_review_work_scope'])) };
}

function reply(items, total = items.length, cursor = null) {
  return { protocol_version: 1, scope_id: 'main', source_snapshot_id: 'current-plan', project_revision: '12',
    coordinator_epoch: 'current', workstream_id: 'development', authority_version: '1',
    data: { work_id: 'work-one', items, total, next_cursor: cursor, truncated: cursor !== null,
      execution_authorized: false, as_of: { at_unix_ms: at, source_snapshot_id: 'current-plan',
        project_revision: '12', rounds_revision: 'b'.repeat(32) } } };
}

async function fixture(t, respond) {
  const calls = [];
  const upstream = http.createServer(async (req, res) => {
    const chunks = [];
    for await (const chunk of req) chunks.push(chunk);
    const query = JSON.parse(Buffer.concat(chunks).toString());
    calls.push({ query, path: req.url, cookie: req.headers.cookie });
    const { status = 200, body } = respond(query);
    res.writeHead(status, { 'content-type': 'application/json' });
    res.end(JSON.stringify(body));
  });
  await new Promise((resolve) => upstream.listen(0, '127.0.0.1', resolve));
  t.after(() => new Promise((resolve) => { upstream.close(resolve); upstream.closeAllConnections(); }));
  const bridge = createTeamBridge({ teamUrl: `http://127.0.0.1:${upstream.address().port}`, port: 0 });
  const read = (parameters) => bridge.routes['GET /api/team/reviews'](
    new URL(`http://localhost/api/team/reviews?${parameters}`), null,
    { headers: { cookie: 'awr_web_session=synthetic-review-reader' } },
    { getHeader: () => undefined, setHeader: () => {} },
  );
  return { read, calls };
}

const scope = 'project=portal&work_id=work-one&workstream_id=development';

test('review history retains ordered restricted placeholders and the current member session', async (t) => {
  const pages = [reply([readableRound()], 2, 'page-two'), reply([restrictedRound(2)], 2)];
  const { read, calls } = await fixture(t, (query) => ({ body: pages[query.cursor ? 1 : 0] }));
  for (const [index, cursor] of ['', '&cursor=page-two'].entries()) {
    const result = await read(`${scope}&limit=1&max_context_bytes=65536${cursor}`);
    assert.equal(result.ok, true, JSON.stringify(result));
    assert.deepEqual(result.result.data, pages[index].data);
    assert.equal(result.freshness.authoritative.status, 'measured');
    assert.deepEqual(calls[index], { path: '/v1/web/projects/portal/query', cookie: 'awr_web_session=synthetic-review-reader',
      query: { protocol_version: 1, op: 'review.rounds', work_id: 'work-one', workstream_id: 'development',
        limit: 1, max_context_bytes: 65536, ...(index ? { cursor: 'page-two' } : {}) } });
  }
});

test('review history refuses unattributed metadata, contradictory decisions and hidden disclosures', async (t) => {
  let body;
  const { read } = await fixture(t, () => ({ body }));
  const mutations = [
    ['missing work', (r) => { delete r.data.work_id; }],
    ['different work', (r) => { r.data.work_id = 'must-not-leak'; }],
    ['missing workstream', (r) => { delete r.workstream_id; }],
    ['different workstream', (r) => { r.workstream_id = 'must-not-leak'; }],
    ['missing authority', (r) => { delete r.authority_version; }],
    ['wrong as-of', (r) => { r.data.as_of.project_revision = '9'; }],
    ['missing total', (r) => { delete r.data.total; }],
    ['unknown state', (r) => { r.data.items[0].state = 'must-not-leak'; }],
    ['unbound role', (r) => { r.data.items[0].role_basis = null; }],
    ['missing decision time', (r) => { r.data.items[0].decided_at_unix_ms = null; }],
    ['full multiline reason', (r) => { r.data.items[0].reason_first_line = 'first\nmust-not-leak'; }],
    ['masked body', (r) => { r.data.items[0].readable = false; r.data.items[0].reason = 'not_in_review_work_scope'; }],
  ];
  for (const [name, mutate] of mutations) {
    body = reply([readableRound()]);
    mutate(body);
    const result = await read(scope);
    assert.equal(result.ok, false, name);
    assert.equal(result.error.code, 'BadGateway', name);
    assert.equal(result.result, undefined, name);
    assert.equal(JSON.stringify(result).includes('must-not-leak'), false, name);
  }
  for (const field of fields) {
    body = reply([restrictedRound()]);
    body.data.items[0][field] = 'must-not-leak';
    const result = await read(scope);
    assert.equal(result.ok, false, field);
    assert.equal(result.result, undefined, field);
  }
});

test('review history enforces complete positional pages, including nonconsecutive historical rounds', async (t) => {
  let body;
  const { read } = await fixture(t, () => ({ body }));
  for (const [name, value, params] of [
    ['missing prefix', reply([readableRound(2)], 2), ''],
    ['missing tail', reply([readableRound()], 2), ''],
    ['gap', reply([readableRound(), readableRound(3)], 3), ''],
    ['short nonterminal page', reply([readableRound()], 3, 'next'), '&limit=2'],
    ['duplicate round', reply([readableRound(), { ...readableRound(2), round_id: 'review-1' }]), ''],
    ['reversed historical index', reply([readableRound(), { ...readableRound(2), round_index: 1 }]), ''],
    ['missing page', reply([], 2), ''],
  ]) {
    body = value;
    const result = await read(scope + params);
    assert.equal(result.ok, false, name);
    assert.equal(result.error.code, 'BadGateway', name);
  }
  for (const value of [reply([]), reply([readableRound(), restrictedRound(2)])]) {
    body = value;
    assert.equal((await read(scope)).ok, true);
  }
});

test('review history preserves unknown historical roles and exact failure semantics', async (t) => {
  let body = reply([readableRound()]), status = 200;
  body.data.items[0].reviewer_role = null;
  body.data.items[0].role_basis = null;
  body.data.items[0].missing.reviewer_role = 'not_recorded';
  body.data.items[0].missing.role_basis = 'not_recorded';
  const { read, calls } = await fixture(t, () => ({ body, status }));
  assert.equal((await read(scope)).ok, true);
  for (const parameters of ['project=portal', `${scope}&actor_id=forged`, `${scope}&limit=1.5`,
    `${scope}&limit=101`, `${scope}&max_context_bytes=262145`, `${scope}&work_id=other`, `${scope}&op=work.complete`]) {
    assert.equal((await read(parameters)).error.code, 'InvalidInput', parameters);
  }
  assert.equal(calls.length, 1);
  for (const code of ['Forbidden', 'CursorExpired', 'ResponseTooLarge']) {
    body = { code, message: 'Read refused' };
    status = 403;
    const result = await read(scope);
    assert.equal(result.ok, false);
    assert.equal(result.error.code, code);
    assert.equal(result.result, undefined);
  }
});
