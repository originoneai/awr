'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const http = require('node:http');
const { createTeamBridge } = require('../server/team-bridge');
const reference = `agent_${'a'.repeat(64)}`;

async function fixture(t) {
  const calls = [];
  let response = { ok: true, data: { items: [{ connection_id: reference, kind: 'agent', client_name: 'Grok Build',
    state: 'active', can_revoke: true, revocation_scope: 'connection_family', raw_payload: 'private-diagnostics' }],
    next_cursor: null, raw_payload: 'private-diagnostics' } };
  const server = http.createServer(async (req, res) => {
    const chunks = [];
    for await (const chunk of req) chunks.push(chunk);
    calls.push({ path: req.url, cookie: req.headers.cookie, body: JSON.parse(Buffer.concat(chunks).toString()) });
    res.writeHead(200, { 'content-type': 'application/json' });
    res.end(JSON.stringify(response));
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  t.after(() => new Promise((resolve) => { server.close(resolve); server.closeAllConnections(); }));
  const bridge = createTeamBridge({ teamUrl: `http://127.0.0.1:${server.address().port}`, port: 0 });
  const request = { headers: { cookie: 'awr_web_session=synthetic-browser' } };
  const res = { getHeader: () => undefined, setHeader: () => {} };
  return { calls, setResponse: (value) => { response = value; },
    call: (action, body) => bridge.routes[`POST /api/team/connections/${action}`](null, body, request, res) };
}

test('connection list forwards the signed-in cookie and exposes only product fields', async (t) => {
  const { calls, call } = await fixture(t);
  const result = await call('list', { project: 'customer', limit: 20 });
  assert.equal(result.ok, true);
  assert.deepEqual(calls[0], { path: '/v1/web/projects/customer/connections/list',
    cookie: 'awr_web_session=synthetic-browser', body: { protocol_version: 1, limit: 20 } });
  assert.equal(result.data.items[0].connection_id, reference);
  assert.equal(JSON.stringify(result).includes('private-diagnostics'), false);
});

test('connection mutations reject unconfirmed or mismatched upstream receipts', async (t) => {
  const { call, setResponse } = await fixture(t);
  for (const data of [{}, { connection_id: reference, state: 'unknown' },
    { connection_id: `agent_${'b'.repeat(64)}`, state: 'revoked' }]) {
    setResponse({ ok: true, data });
    const result = await call('revoke', { project: 'customer', connection_id: reference });
    assert.equal(result.ok, false);
    assert.equal(result.error.code, 'BadGateway');
  }
  setResponse({ ok: true, data: { connection_id: reference, state: 'revoked', ends_current: true, bearer: 'private' } });
  assert.deepEqual(await call('revoke', { project: 'customer', connection_id: reference }),
    { ok: true, data: { connection_id: reference, state: 'revoked', ends_current: true } });
});

test('malformed lists and injected identity or workflow fields are refused', async (t) => {
  const { call, setResponse, calls } = await fixture(t);
  for (const body of [{ project: 'customer', subject: { actor_id: 'other' } },
    { project: 'customer', op: 'claim.acquire' }, { project: 'customer', bearer: 'private' }]) {
    assert.equal((await call('list', body)).error.code, 'InvalidInput');
  }
  assert.equal(calls.length, 0);
  for (const data of [{ items: [], next_cursor: 'private_cursor' },
    { items: [{ connection_id: reference, state: 'unknown' }], next_cursor: null }]) {
    setResponse({ ok: true, data });
    assert.equal((await call('list', { project: 'customer' })).error.code, 'BadGateway');
  }
});
