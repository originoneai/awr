'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const http = require('node:http');
const { createTeamBridge } = require('../server/team-bridge');

async function graphRead(t, reply, { scopeChanged = false, scopeTotal = 1 } = {}) {
  const calls = [];
  let scopeReads = 0;
  const server = http.createServer(async (req, res) => {
    res.setHeader('content-type', 'application/json');
    if (req.url === '/v1/web/session') {
      res.end(JSON.stringify({ session_id: 'graph-reader', expires_at_ms: Date.now() + 60000 }));
      return;
    }
    const chunks = [];
    for await (const chunk of req) chunks.push(chunk);
    const query = JSON.parse(Buffer.concat(chunks).toString());
    calls.push(query);
    if (query.op === 'workstreams.list') {
      scopeReads += 1;
      res.end(JSON.stringify({
        items: [{ id: 'development', authority_version: scopeChanged ? scopeReads : 1, external_key: 'development', title: 'Development' }],
        total: scopeTotal, next_cursor: null,
      }));
      return;
    }
    const data = reply(query);
    if (data.code) res.statusCode = 403;
    res.end(JSON.stringify(data.code ? data : {
      protocol_version: 1, project_revision: query.cursor === 'changed' ? '2' : '1',
      source_snapshot_id: 'source-1', coordinator_epoch: 'current', data,
    }));
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  t.after(() => new Promise((resolve) => { server.close(resolve); server.closeAllConnections(); }));
  const bridge = createTeamBridge({ teamUrl: `http://127.0.0.1:${server.address().port}`, port: 0 });
  const headers = new Map();
  const result = await bridge.routes['GET /api/team/graph'](
    new URL('http://localhost/api/team/graph?project=portal'), null,
    { headers: { cookie: 'awr_web_session=synthetic-graph-session' } },
    { getHeader: (key) => headers.get(key), setHeader: (key, value) => headers.set(key, value) },
  );
  return { result, calls };
}

test('Team graphs compose scoped pages without per-work reads', async (t) => {
  const { result, calls } = await graphRead(t, (query) => ({
    items: [{ work_id: query.cursor ? 'ui' : 'api', title: query.cursor ? 'Interface' : 'API' }],
    total: 2, next_cursor: query.cursor ? null : 'second',
  }));
  assert.equal(result.ok, true);
  assert.equal(result.items.length, 2);
  assert.equal(result.as_of.project_revision, '1');
  assert.deepEqual(calls.map((q) => q.op), ['workstreams.list', 'work.graph', 'work.graph', 'workstreams.list']);
  assert.ok(calls.filter((q) => q.op === 'work.graph').every((q) => q.workstream_id === 'development'));
});

test('Team graphs refuse mixed revisions and incomplete pages', async (t) => {
  for (const scenario of ['changed', 'incomplete']) {
    const { result } = await graphRead(t, (query) => ({
      items: [{ work_id: query.cursor ? 'ui' : 'api' }], total: 2,
      next_cursor: !query.cursor && scenario === 'changed' ? 'changed' : null,
    }));
    assert.equal(result.ok, false);
    assert.equal(result.error.code, scenario === 'changed' ? 'SourceChanged' : 'ResponseTooLarge');
    assert.equal(result.items, undefined);
  }
});

test('a scoped graph denial does not fall back to fixtures', async (t) => {
  const { result } = await graphRead(t, () => ({ code: 'Forbidden', message: 'Access denied' }));
  assert.equal(result.ok, false);
  assert.equal(result.error.code, 'Forbidden');
  assert.equal(result.items, undefined);
});

test('graph scope and work pages share one complete revision', async (t) => {
  for (const options of [{ scopeChanged: true }, { scopeTotal: 2 }]) {
    const { result } = await graphRead(t, () => ({ items: [{ work_id: 'api' }], total: 1, next_cursor: null }), options);
    assert.equal(result.ok, false);
    assert.equal(result.error.code, options.scopeChanged ? 'SourceChanged' : 'ResponseTooLarge');
    assert.equal(result.items, undefined);
  }
});
