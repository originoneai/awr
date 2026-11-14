'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const http = require('node:http');
const { createTeamBridge } = require('../server/team-bridge');

async function sourceBridge(t, respond) {
  const calls = [];
  const upstream = http.createServer(async (req, res) => {
    const chunks = [];
    for await (const chunk of req) chunks.push(chunk);
    const query = JSON.parse(Buffer.concat(chunks).toString());
    calls.push({ path: req.url, cookie: req.headers.cookie, query });
    const { status = 200, body } = respond(query);
    res.writeHead(status, { 'content-type': 'application/json' });
    res.end(JSON.stringify(body));
  });
  await new Promise((resolve) => upstream.listen(0, '127.0.0.1', resolve));
  t.after(() => new Promise((resolve) => { upstream.close(resolve); upstream.closeAllConnections(); }));
  const bridge = createTeamBridge({ teamUrl: `http://127.0.0.1:${upstream.address().port}`, port: 0 });
  const read = (parameters) => bridge.routes['GET /api/team/sources'](
    new URL(`http://localhost/api/team/sources?${parameters}`), null,
    { headers: { cookie: 'awr_web_session=synthetic-document-reader' } },
    { getHeader: () => undefined, setHeader: () => {} },
  );
  return { read, calls };
}

test('source route preserves full-collection completeness across pages and the current member session', async (t) => {
  const pages = [
    { items: [{ path: 'docs/requirements.md', status: 'consistent' }], next_cursor: 'page-two' },
    { items: [{ path: null, status: 'forbidden', expected_sha256: null, current_sha256: null }], next_cursor: null },
  ].map((page) => ({ ...page, total: 2, truncated: page.next_cursor != null,
    collection: { complete: false, reasons: ['required_spec_forbidden'], verification_basis: 'active_source_snapshot' },
    as_of: { at_unix_ms: 1793534400000 }, execution_authorized: false }));
  const { read, calls } = await sourceBridge(t, (query) => ({ body: {
    protocol_version: 1, scope_id: 'main', source_snapshot_id: 'current-documents',
    project_revision: '12', coordinator_epoch: 'current', workstream_id: 'development',
    authority_version: '1', data: { ...pages[query.cursor ? 1 : 0], work_id: query.work_id },
  } }));
  for (const [index, cursor] of ['', '&cursor=page-two'].entries()) {
    const result = await read(`project=portal&work_id=work-one&workstream_id=development&limit=1&max_context_bytes=65536${cursor}`);
    assert.equal(result.ok, true, JSON.stringify(result));
    assert.deepEqual(result.result.data, { ...pages[index], work_id: 'work-one' });
    assert.equal(result.freshness.authoritative.status, 'measured');
    assert.deepEqual(calls[index], {
      path: '/v1/web/projects/portal/query', cookie: 'awr_web_session=synthetic-document-reader',
      query: { protocol_version: 1, op: 'sources.inspect', work_id: 'work-one',
        workstream_id: 'development', limit: 1, max_context_bytes: 65536,
        ...(index ? { cursor: 'page-two' } : {}) },
    });
  }
});

test('source route refuses unrelated controls and preserves revocation and expired cursors', async (t) => {
  let code = 'Forbidden';
  const { read, calls } = await sourceBridge(t, () => ({
    status: 403, body: { code, message: 'Read refused' },
  }));
  for (const parameters of ['project=portal&actor_id=forged', 'project=portal&session_id=other',
    'project=portal&limit=1.5', 'project=portal&project=other', 'project=portal&limit=101',
    'project=portal&max_context_bytes=262145', 'project=portal&work_id=', 'project=portal&op=work.complete']) {
    assert.equal((await read(parameters)).error.code, 'InvalidInput', parameters);
  }
  assert.equal(calls.length, 0);
  for (code of ['Forbidden', 'CursorExpired']) {
    const result = await read('project=portal&cursor=previous-page');
    assert.equal(result.ok, false);
    assert.equal(result.error.code, code);
    assert.equal(result.result, undefined);
  }
});
