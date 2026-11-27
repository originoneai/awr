'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const http = require('node:http');
const { createTeamBridge } = require('../server/team-bridge');
// Recorded from the production service router over HTTP MCP, using synthetic
// registered sources and an isolated PostgreSQL fixture. No demo-shaped replies.
const list = require('../../../crates/awr-server/tests/samples/goals-list.json');
const inspect = require('../../../crates/awr-server/tests/samples/goal-inspect.json');
const retired = require('../../../crates/awr-server/tests/samples/goal-retired-inspect.json');

async function setup(t) {
  let body = list, status = 200;
  const calls = [];
  const upstream = http.createServer(async (req, res) => {
    const chunks = [];
    for await (const chunk of req) chunks.push(chunk);
    calls.push({ path: req.url, cookie: req.headers.cookie, query: JSON.parse(Buffer.concat(chunks)) });
    res.writeHead(status, { 'content-type': 'application/json' });
    res.end(JSON.stringify(body));
  });
  await new Promise((resolve) => upstream.listen(0, '127.0.0.1', resolve));
  t.after(() => new Promise((resolve) => { upstream.close(resolve); upstream.closeAllConnections(); }));
  const bridge = createTeamBridge({ teamUrl: `http://127.0.0.1:${upstream.address().port}`, port: 0 });
  return {
    calls,
    response(value, code = 200) { body = value; status = code; },
    read(route = 'goals', params = '') {
      return bridge.routes[`GET /api/team/${route}`](
        new URL(`http://localhost/api/team/${route}?project=one${params}`), null,
        { headers: { cookie: 'awr_web_session=synthetic-goal-reader' } },
        { getHeader: () => undefined, setHeader: () => {} });
    },
  };
}

test('goal routes preserve verbatim service replies, attribution and the caller session', async (t) => {
  const s = await setup(t);
  assert.equal((await s.read()).ok, true);
  assert.deepEqual(s.calls.at(-1), { path: '/v1/web/projects/one/query',
    cookie: 'awr_web_session=synthetic-goal-reader', query: { protocol_version: 1, op: 'goals.list' } });
  s.response(inspect);
  const result = await s.read('goal', '&goal_id=ship');
  assert.equal(result.ok, true, JSON.stringify(result));
  assert.deepEqual(result.result, inspect);
  assert.equal(result.result.data.goal.hidden_task_count, 1);
  assert.equal(result.result.data.goal.status, 'confirmed');
  assert.deepEqual(s.calls.at(-1).query, { protocol_version: 1, op: 'goal.inspect', goal_id: 'ship' });
  assert.ok(result.freshness);
  s.response(retired);
  const retiredResult = await s.read('goal', '&goal_id=retired');
  assert.equal(retiredResult.ok, true, JSON.stringify(retiredResult));
  assert.equal(retiredResult.result.data.orphan_work[0].reason, 'cancelled_goal');
});

test('goal reads reject forged authority, unbounded inputs and duplicate selectors before forwarding', async (t) => {
  const s = await setup(t);
  for (const [route, params] of [
    ['goals', '&goal_id=ship'], ['goal', ''], ['goals', '&actor_id=forged'],
    ['goals', '&project=two'], ['goals', '&workstream_id=one&workstream_id=two'],
    ['goals', '&work_id=other'], ['goals', '&session_id=other'],
    ['goals', '&limit=0'], ['goals', '&limit=101'], ['goals', '&limit=1.5'],
    ['goal', '&goal_id=ship&max_context_bytes=262145'], ['goals', `&cursor=${'x'.repeat(4097)}`],
    ['goal', `&goal_id=${'x'.repeat(129)}`], ['goal', '&goal_id=bad%00id'],
    ['goal', '&goal_id=' + encodeURIComponent('目'.repeat(43))], ['goal', '&goal_id=bad%C2%85id'],
  ]) {
    const result = await s.read(route, params);
    assert.equal(result.ok, false, params);
    assert.equal(result.error.code, 'InvalidInput', params);
  }
  assert.equal(s.calls.length, 0);
});

test('goal response contract rejects malformed or leaking replies before releasing them', async (t) => {
  const s = await setup(t);
  for (const mutate of [
    (r) => { r.data.execution_authorized = true; },
    (r) => { r.data.read_only = false; },
    (r) => { r.data.as_of.project_revision = '0'; },
    (r) => { r.data.as_of.at_unix_ms = -1; },
    (r) => { r.data.as_of.source_snapshot_id = 'other'; },
    (r) => { r.data.goal.goal_id = 'other'; },
    (r) => { r.data.goal.hidden_task_count = -1; },
    (r) => { r.data.goal.hidden_task_count = '1'; },
    (r) => { r.data.goal.success_criteria = 'one'; },
    (r) => { r.data.goal.status = null; },
    (r) => { r.data.goal.uncovered = null; },
    (r) => { r.data.goal.tasks[0].title = null; },
    (r) => { r.data.goal.tasks[0].report = 'must-not-leak'; },
    (r) => { r.data.goal.source_body = 'must-not-leak'; },
    (r) => { r.data.credential = 'must-not-leak'; },
    (r) => { r.credential = 'must-not-leak'; },
    (r) => { r.project_status = { report: 'must-not-leak' }; },
    (r) => { delete r.project_status; },
    (r) => { r.data.total = 0; },
    (r) => { r.data.truncated = true; },
    (r) => { r.data.next_cursor = ''; },
    (r) => { r.data.goal.tasks.push(r.data.goal.tasks[0]); },
    (r) => { r.data.orphan_work = [{ kind: 'orphan_work', goal_id: 'ship', task: r.data.goal.tasks[0], reason: 'invented' }]; },
    (r) => { r.workstream_id = 'other'; },
    (r) => { r.scope_id = 'other'; },
  ]) {
    const body = structuredClone(inspect); mutate(body); s.response(body);
    const result = await s.read('goal', '&goal_id=ship');
    assert.equal(result.ok, false, mutate.toString());
    assert.equal(result.error.code, 'BadGateway', mutate.toString());
    assert.equal(result.result, undefined);
    assert.equal(JSON.stringify(result).includes('must-not-leak'), false);
  }
  s.response(list);
  assert.equal((await s.read('goals', '&limit=1')).error.code, 'BadGateway');
  s.response(inspect);
  assert.equal((await s.read('goal', '&goal_id=ship&workstream_id=other')).error.code, 'BadGateway');
  assert.equal((await s.read('goal', '&goal_id=ship&max_context_bytes=1')).error.code, 'BadGateway');
});

test('goal routes retain bounded pages and reauthorize after a successful read', async (t) => {
  const s = await setup(t);
  const body = structuredClone(inspect);
  body.data.goal.tasks = body.data.goal.tasks.slice(0, 1);
  body.data.next_cursor = 'recorded-page-continuation'; body.data.truncated = true;
  s.response(body);
  const first = await s.read('goal', '&goal_id=ship&limit=1&max_context_bytes=65536');
  assert.equal(first.ok, true, JSON.stringify(first));
  for (const code of ['Forbidden', 'CursorExpired']) {
    s.response({ code, message: 'Read refused' }, 403);
    const denied = await s.read('goal', '&goal_id=ship&limit=1&cursor=recorded-page-continuation');
    assert.equal(denied.ok, false);
    assert.equal(denied.error.code, code);
    assert.equal(denied.result, undefined);
  }
  assert.equal(s.calls.length, 3);
});

test('goal collection validates previews, orphan records and deterministic ordering', async (t) => {
  const s = await setup(t);
  for (const mutate of [
    (r) => { r.data.items[0] = null; },
    (r) => { r.data.items[0].source_body = 'must-not-leak'; },
    (r) => { r.data.items[0].task_total = -1; },
    (r) => { r.data.items[0].tasks_next_cursor = 'unexpected'; },
    (r) => { r.data.items[0].departments = [{ name: 'invented' }]; },
    (r) => { r.data.items.push(r.data.items[0]); r.data.total++; },
    (r) => { r.data.items.reverse(); },
    (r) => { r.data.items.find((v) => v.kind === 'orphan_work').reason = 'invented'; },
    (r) => { r.data.items.find((v) => v.kind === 'orphan_work').task.report = 'must-not-leak'; },
    (r) => { r.data.items.find((v) => v.kind === 'goal' && v.tasks.length > 1).tasks.reverse(); },
  ]) {
    const body = structuredClone(list); mutate(body); s.response(body);
    const result = await s.read();
    assert.equal(result.ok, false, mutate.toString());
    assert.equal(result.error.code, 'BadGateway');
    assert.equal(result.result, undefined);
    assert.equal(JSON.stringify(result).includes('must-not-leak'), false);
  }
});


test('goal detail rejects duplicate orphan records from a recorded service reply', async (t) => {
  const s = await setup(t);
  const body = structuredClone(retired);
  body.data.orphan_work.push(structuredClone(body.data.orphan_work[0]));
  s.response(body);
  const result = await s.read('goal', '&goal_id=retired');
  assert.equal(result.ok, false);
  assert.equal(result.error.code, 'BadGateway');
  assert.equal(result.result, undefined);
});
