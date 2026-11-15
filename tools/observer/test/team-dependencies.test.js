'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const http = require('node:http');
const { createTeamBridge } = require('../server/team-bridge');

const version = (receipt = 'accepted-output', hash = 'a') => ({ receipt_id: receipt,
  contract_hash: hash.repeat(64), source_snapshot_id: 'published-plan' });

function dependency(position = 1) {
  return { work_id: 'customer-portal', work_title: 'Customer portal', dependency_position: position,
    provider_work_id: `service-${position}`, provider_title: 'Account service', version_policy: 'current_contract',
    review_assurance: null, required_version: { ...version(), receipt_id: null }, available_version: version(),
    adopted_version: null, consumed_version: null, adoption_valid: null, validity: 'valid', reason: null,
    missing: { provider_title: null, required_version: null, available_version: null,
      adopted_version: 'not_recorded', consumed_version: 'not_recorded' } };
}

function reply(items = [dependency()], { work = 'customer-portal', total = items.length, cursor = null } = {}) {
  return { protocol_version: 1, scope_id: 'main', coordinator_epoch: 'current', source_snapshot_id: 'published-plan',
    project_revision: '12', workstream_id: 'delivery', authority_version: '1', data: {
      work_id: work, items, total, next_cursor: cursor, truncated: cursor !== null, count_scope: 'authorized_matches',
      count_limited: true, hidden_count: null, execution_authorized: false, as_of: {
        at_unix_ms: 1793534400000, source_snapshot_id: 'published-plan', project_revision: '12' } } };
}

async function fixture(t, respond) {
  const calls = [];
  const upstream = http.createServer(async (req, res) => {
    const chunks = [];
    for await (const chunk of req) chunks.push(chunk);
    const query = JSON.parse(Buffer.concat(chunks).toString());
    calls.push({ query, cookie: req.headers.cookie, path: req.url });
    const { status = 200, body } = respond(query);
    res.writeHead(status, { 'content-type': 'application/json' }); res.end(JSON.stringify(body));
  });
  await new Promise((resolve) => upstream.listen(0, '127.0.0.1', resolve));
  t.after(() => new Promise((resolve) => { upstream.close(resolve); upstream.closeAllConnections(); }));
  const bridge = createTeamBridge({ teamUrl: `http://127.0.0.1:${upstream.address().port}`, port: 0 });
  const request = { headers: { cookie: 'awr_web_session=synthetic-dependency-reader' } };
  const response = { getHeader: () => undefined, setHeader: () => {} };
  return { calls,
    read: (parameters) => bridge.routes['GET /api/team/dependencies'](
      new URL(`http://localhost/api/team/dependencies?${parameters}`), null, request, response),
    inspect: () => bridge.routes['POST /api/team/read'](null, { project: 'portal', query: {
      protocol_version: 1, op: 'delivery.inspect', work_id: 'customer-portal', workstream_id: 'delivery' } }, request, response) };
}

const scope = 'project=portal&work_id=customer-portal&workstream_id=delivery';

test('dependency observations forward the current identity and preserve bounded authoritative pages', async (t) => {
  const pages = [reply([dependency()], { total: 2, cursor: 'next-page' }), reply([dependency(2)], { total: 2 })];
  const { read, calls } = await fixture(t, (q) => ({ body: pages[q.cursor ? 1 : 0] }));
  for (const [index, cursor] of ['', '&cursor=next-page'].entries()) {
    const result = await read(`${scope}&limit=1&max_context_bytes=65536${cursor}`);
    assert.equal(result.ok, true, JSON.stringify(result));
    assert.deepEqual(result.result.data, pages[index].data);
    assert.deepEqual(calls[index], { query: { protocol_version: 1, op: 'dependencies.list',
      work_id: 'customer-portal', workstream_id: 'delivery', limit: 1, max_context_bytes: 65536,
      ...(index ? { cursor: 'next-page' } : {}) }, cookie: 'awr_web_session=synthetic-dependency-reader',
    path: '/v1/web/projects/portal/query' });
  }
});

test('dependency mapping matches delivery inspection without borrowing an available version as an adopted one', async (t) => {
  const item = dependency();
  const inspected = { ...item }; delete inspected.dependency_position; delete inspected.work_title;
  const response = reply();
  const { read, inspect } = await fixture(t, (q) => ({ body: q.op === 'delivery.inspect'
    ? { ...response, data: { delivery: { work_id: 'customer-portal', dependencies: [inspected] } } } : response }));
  const listed = await read(scope), delivery = await inspect();
  assert.equal(listed.ok, true); assert.equal(delivery.ok, true);
  const projected = { ...listed.result.data.items[0] }; delete projected.dependency_position; delete projected.work_title;
  assert.deepEqual(projected, delivery.result.data.delivery.dependencies[0]);
  assert.equal(projected.adopted_version, null); assert.equal(projected.consumed_version, null);
});

test('fixed historical dependency observations preserve their original source version', async (t) => {
  const item = dependency();
  item.version_policy = 'fixed_delivery';
  item.review_assurance = { cross_workstream: { review_assurance: 'simulated_member_independent', version_policy: 'fixed_delivery' } };
  item.required_version = version('historical-output', 'b');
  item.required_version.source_snapshot_id = 'historical-plan';
  for (const key of ['available_version', 'adopted_version', 'consumed_version']) {
    item[key] = { ...item.required_version }; item.missing[key] = null;
  }
  item.adoption_valid = true;
  const { read } = await fixture(t, () => ({ body: reply([item]) }));
  const result = await read(scope); assert.equal(result.ok, true, JSON.stringify(result));
  assert.equal(result.result.data.items[0].adopted_version.source_snapshot_id, 'historical-plan');
  assert.equal(result.result.source_snapshot_id, 'published-plan');
});

test('restricted dependency versions remain reasoned nulls and unavailable edges stay visible', async (t) => {
  const item = dependency();
  for (const key of ['provider_title', 'required_version', 'available_version', 'adopted_version', 'consumed_version']) {
    item[key] = null; item.missing[key] = 'permission_restricted';
  }
  item.validity = 'unavailable'; item.reason = 'permission_restricted';
  const { read } = await fixture(t, () => ({ body: reply([item]) }));
  const result = await read(scope); assert.equal(result.ok, true, JSON.stringify(result));
  assert.deepEqual(result.result.data.items, [item]);
});

test('dependency reads reject scope drift, incomplete coverage and inconsistent version claims', async (t) => {
  let body;
  const { read } = await fixture(t, () => ({ body }));
  for (const mutate of [
    (r) => { r.workstream_id = 'other'; }, (r) => { delete r.authority_version; },
    (r) => { r.data.items[0].work_id = 'other'; }, (r) => { r.data.work_id = 'other'; },
    (r) => { r.data.as_of.source_snapshot_id = 'old'; },
    (r) => { r.data.items[0].dependency_position = 2; }, (r) => { r.data.total = 2; },
    (r) => { r.data.hidden_count = 4; }, (r) => { r.data.count_limited = false; },
    (r) => { r.data.items[0].available_version = null; },
    (r) => { r.data.items[0].required_version.contract_hash = 'incomplete'; },
    (r) => { r.data.items[0].missing.adopted_version = null; },
    (r) => { r.data.items[0].review_assurance = { cross_workstream: { raw: 'must-not-leak' } }; },
  ]) {
    body = reply(); mutate(body);
    const result = await read(scope); assert.equal(result.ok, false, JSON.stringify(body));
    assert.equal(result.error.code, 'BadGateway'); assert.equal(result.result, undefined);
  }
});

test('dependency metadata excludes raw payloads at all response levels', async (t) => {
  let body;
  const { read } = await fixture(t, () => ({ body }));
  for (const select of [(r) => r, (r) => r.data, (r) => r.data.as_of, (r) => r.data.items[0],
    (r) => r.data.items[0].required_version, (r) => r.data.items[0].missing]) {
    body = reply(); select(body).raw_payload = 'must-not-leak';
    const result = await read(scope); assert.equal(result.ok, false);
    assert.equal(result.result, undefined); assert.equal(JSON.stringify(result).includes('must-not-leak'), false);
  }
});

test('dependency query validation refuses writes and unsupported selectors before contacting Team', async (t) => {
  const { read, calls } = await fixture(t, () => ({ body: reply() }));
  for (const suffix of ['&session_id=other', '&limit=101', '&limit=1&limit=2', '&request_id=write', '&max_context_bytes=0']) {
    const result = await read(scope + suffix); assert.equal(result.ok, false); assert.equal(result.error.code, 'InvalidInput');
  }
  assert.equal(calls.length, 0);
});
