'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const http = require('node:http');
const fs = require('node:fs');
const path = require('node:path');
const { createTeamBridge } = require('../server/team-bridge');
// Exact HTTP bytes emitted by the production router with isolated PostgreSQL.
const samples = Object.fromEntries(['review.queue', 'team.reviewers'].map((op) => [op,
  JSON.parse(fs.readFileSync(path.join(__dirname, 'fixtures/review-queue', `${op}.json`), 'utf8'))]));

async function fixture(t) {
  const state = { op: 'review.queue', body: null, status: 200, calls: [] };
  const upstream = http.createServer(async (req, res) => {
    const chunks = [];
    for await (const chunk of req) chunks.push(chunk);
    state.calls.push({ path: req.url, cookie: req.headers.cookie, query: JSON.parse(Buffer.concat(chunks)) });
    res.writeHead(state.status, { 'content-type': 'application/json' });
    res.end(JSON.stringify(state.body ?? samples[state.op]));
  });
  await new Promise((resolve) => upstream.listen(0, '127.0.0.1', resolve));
  t.after(() => new Promise((resolve) => { upstream.close(resolve); upstream.closeAllConnections(); }));
  const bridge = createTeamBridge({ teamUrl: `http://127.0.0.1:${upstream.address().port}`, port: 0 });
  const read = (op, suffix = '') => {
    state.op = op;
    const route = op === 'review.queue' ? 'review-queue' : 'reviewers';
    return bridge.routes[`GET /api/team/${route}`](new URL(`http://localhost/api/team/${route}?project=one${suffix}`), null,
      { headers: { cookie: 'awr_web_session=synthetic-queue-reader' } }, { getHeader: () => undefined, setHeader: () => {} });
  };
  return { state, read, bridge };
}

test('queue and reviewer routes preserve verbatim production replies and forward only the reader session', async (t) => {
  const { state, read } = await fixture(t);
  for (const op of Object.keys(samples)) {
    const result = await read(op);
    assert.equal(result.ok, true, JSON.stringify(result));
    assert.deepEqual(result.result, samples[op]);
    assert.deepEqual(state.calls.at(-1), { path: '/v1/web/projects/one/query', cookie: 'awr_web_session=synthetic-queue-reader',
      query: { protocol_version: 1, op } });
  }
  assert.deepEqual(samples['team.reviewers'].data.items, [{ name: 'Reviewer One', review_duty: 'reviewer', queue_count: 2 }]);
  assert.deepEqual(samples['review.queue'].data.items[0].requested_reviewer, { kind: 'role', role: 'reviewer' });
});

test('review reads reject extra authority, selectors and response privacy leaks', async (t) => {
  const { state, read, bridge } = await fixture(t);
  for (const [op, mutate] of [
    ['team.reviewers', (r) => { r.data.items[0].other_duties = ['administrator']; }],
    ['team.reviewers', (r) => { r.data.items[0].person_id = 'private'; }],
    ['review.queue', (r) => { r.data.items[0].requested_reviewer = null; }],
    ['review.queue', (r) => { r.data.groups[0].oldest_wait_ms = -1; }],
    ['review.queue', (r) => { r.data.execution_authorized = true; }],
    ['review.queue', (r) => { r.data.work_id = 'private'; }],
    ['review.queue', (r) => { r.data.as_of.project_revision = 'wrong'; }],
  ]) {
    state.body = structuredClone(samples[op]); mutate(state.body);
    const result = await read(op);
    assert.equal(result.ok, false, JSON.stringify(state.body));
    assert.equal(result.error.code, 'BadGateway');
    assert.equal(result.result, undefined);
  }
  for (const query of [
    { op: 'review.queue', session_id: 's' }, { op: 'team.reviewers', recipient_person_id: 'p' },
    { op: 'review.queue', actor_id: 'forged' }, { op: 'review.queue', limit: 101 },
  ]) {
    const result = await bridge.routes['POST /api/team/read'](new URL('http://localhost/api/team/read'),
      { project: 'one', query: { protocol_version: 1, ...query } }, { headers: {} }, { getHeader: () => undefined, setHeader: () => {} });
    assert.equal(result.ok, false);
    assert.equal(result.error.code, 'InvalidInput');
  }
  state.body = { code: 'Forbidden', message: 'access denied' }; state.status = 403;
  for (const op of Object.keys(samples)) {
    const result = await read(op);
    assert.equal(result.ok, false);
    assert.equal(result.error.code, 'Forbidden');
    assert.equal(result.result, undefined);
  }
});


test('a work-only queue selection accepts the service-resolved stream and rejects a mismatched work', async (t) => {
  const { state, read } = await fixture(t);
  state.body = structuredClone(samples['review.queue']);
  Object.assign(state.body.data, { work_id: 'a', workstream_id: 'resolved-stream', total: 1,
    items: state.body.data.items.filter((item) => item.work_id === 'a'),
    groups: state.body.data.groups.filter((group) => group.requested_reviewer.kind === 'role') });
  const result = await read('review.queue', '&work_id=a');
  assert.equal(result.ok, true, JSON.stringify(result));
  assert.deepEqual(state.calls.at(-1).query, { protocol_version: 1, op: 'review.queue', work_id: 'a' });
  state.body.data.items[0].work_id = 'other-work';
  const refused = await read('review.queue', '&work_id=a');
  assert.equal(refused.error.code, 'BadGateway');
  assert.equal(refused.result, undefined);
});
