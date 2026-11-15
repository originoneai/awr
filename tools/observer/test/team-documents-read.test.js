'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const http = require('node:http');
const { createTeamBridge } = require('../server/team-bridge');
const digest = 'a'.repeat(64);

function document(position = 1, overrides = {}) {
  return { document_position: position, path: 'docs/guide.md', name: 'Delivery guide',
    summary: 'Check the outcome before sharing it.', summary_kind: 'source_excerpt',
    sha256: digest, byte_length: 82, source_kind: 'registered_source',
    readable: true, status: 'available', reason: null, ...overrides };
}

function reply(items = [document()], { work = null, total = items.length, cursor = null } = {}) {
  return { protocol_version: 1, scope_id: 'main', source_snapshot_id: 'current-documents',
    project_revision: '12', coordinator_epoch: 'current',
    ...(work ? { workstream_id: 'development', authority_version: '1' } : {}),
    data: { work_id: work, items, total, next_cursor: cursor, truncated: cursor !== null,
      read_only: true, execution_authorized: false, content_read_limit_bytes: 262144,
      scope: work ? 'work' : 'authorized_workstreams',
      as_of: { at_unix_ms: 1793534400000, source_snapshot_id: 'current-documents', project_revision: '12' } } };
}

async function setup(t, respond) {
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
  const read = (query = {}) => bridge.routes['POST /api/team/read'](
    new URL('http://localhost/api/team/read'),
    { project: 'portal', query: { protocol_version: 1, op: 'documents.list', ...query } },
    { headers: { cookie: 'awr_web_session=synthetic-library-reader' } },
    { getHeader: () => undefined, setHeader: () => {} },
  );
  return { read, calls };
}

test('document discovery preserves project/work scope, pagination, search and member identity', async (t) => {
  const { read, calls } = await setup(t, (q) => ({ body: reply([document(q.cursor ? 2 : 1)], {
    work: q.work_id, total: 2, cursor: q.cursor ? null : 'page-two',
  }) }));
  for (const work of [undefined, 'work-one']) {
    for (const cursor of [undefined, 'page-two']) {
      const query = { search: 'guide', limit: 1, max_context_bytes: 65536,
        ...(work ? { work_id: work, workstream_id: 'development' } : {}), ...(cursor ? { cursor } : {}) };
      const result = await read(query);
      assert.equal(result.ok, true, JSON.stringify(result));
      assert.equal(result.result.data.scope, work ? 'work' : 'authorized_workstreams');
      assert.deepEqual(calls.at(-1), { path: '/v1/web/projects/portal/query',
        cookie: 'awr_web_session=synthetic-library-reader',
        query: { protocol_version: 1, op: 'documents.list', ...query } });
    }
  }
});

test('document discovery accepts explicit unavailable states without disclosing restricted metadata', async (t) => {
  let body;
  const { read } = await setup(t, () => ({ body }));
  for (const [status, reason] of [['restricted', 'permission_restricted'], ['missing', 'required_document_missing'],
    ['changed', 'source_digest_mismatch'], ['unverified', 'source_text_not_recorded']]) {
    body = reply([document(1, { path: status === 'restricted' ? null : 'docs/guide.md',
      name: null, summary: null, sha256: null, byte_length: null, readable: false, status, reason })]);
    assert.equal((await read()).ok, true, status);
  }
  body = reply([document(1, { status: 'too_large', readable: false,
    byte_length: 262145, reason: 'content_exceeds_read_budget' })]);
  assert.equal((await read()).ok, true);
  body = reply([]);
  assert.equal((await read()).ok, true);
});

test('document discovery refuses malformed, unbound and contradictory metadata', async (t) => {
  let body;
  const { read } = await setup(t, () => ({ body }));
  const mutations = [
    (r) => { r.data.work_id = 'must-not-leak'; },
    (r) => { r.workstream_id = 'must-not-leak'; },
    (r) => { delete r.authority_version; },
    (r) => { r.data.scope = 'authorized_workstreams'; },
    (r) => { r.data.read_only = false; },
    (r) => { r.data.execution_authorized = true; },
    (r) => { r.data.as_of.project_revision = '11'; },
    (r) => { r.data.next_cursor = 3; },
    (r) => { r.data.total = 2; },
    (r) => { r.data.truncated = true; },
    (r) => { r.data.items[0].document_position = 2; },
    (r) => { r.data.items[0].body = 'must-not-leak'; },
    (r) => { r.data.items[0].path = '../must-not-leak'; },
    (r) => { r.data.items[0].sha256 = 'invalid'; },
    (r) => { r.data.items[0].byte_length = -1; },
    (r) => { r.data.items[0].name = 'x'.repeat(257); },
    (r) => { r.data.items[0].summary = 'x'.repeat(513); },
    (r) => { r.data.items[0].summary_kind = 'generated'; },
    (r) => { r.data.items[0].status = 'restricted'; },
    (r) => { r.data.items[0].readable = false; },
  ];
  for (const mutate of mutations) {
    body = reply([document()], { work: 'work-one' });
    mutate(body);
    const result = await read({ work_id: 'work-one', workstream_id: 'development' });
    assert.equal(result.ok, false, mutate.toString());
    assert.equal(result.error.code, 'BadGateway');
    assert.equal(result.result, undefined);
    assert.equal(JSON.stringify(result).includes('must-not-leak'), false);
  }
  for (const field of ['path', 'name', 'summary', 'sha256', 'byte_length']) {
    body = reply([document(1, { path: null, name: null, summary: null, sha256: null,
      byte_length: null, readable: false, status: 'restricted', reason: 'permission_restricted',
      [field]: field === 'byte_length' ? 82 : field === 'sha256' ? digest : 'must-not-leak' })]);
    assert.equal((await read()).ok, false, field);
  }
});

test('document discovery refuses unrelated controls and preserves revocation and cursor expiry', async (t) => {
  let code = 'Forbidden';
  const { read, calls } = await setup(t, () => ({ status: 403, body: { code, message: 'Read refused' } }));
  for (const query of [{ session_id: 'other' }, { actor_id: 'forged' }, { source_path: 'docs/guide.md' },
    { limit: 101 }, { search: 'x'.repeat(513) }, { max_context_bytes: 262145 }]) {
    assert.equal((await read(query)).error.code, 'InvalidInput');
  }
  assert.equal(calls.length, 0);
  for (code of ['Forbidden', 'CursorExpired']) {
    const result = await read({ cursor: 'previous-page' });
    assert.equal(result.ok, false);
    assert.equal(result.error.code, code);
    assert.equal(result.result, undefined);
  }
});


test('document discovery carries an exact task selector for controlled content reads', async (t) => {
  const body = reply([document(1, { content_work_id: 'task-one' })]);
  body.data.scope = 'authorized_works';
  const { read, calls } = await setup(t, () => ({ body }));
  const observed = await read();
  assert.equal(observed.ok, true);
  assert.equal(observed.result.data.items[0].content_work_id, 'task-one');
  assert.equal(observed.result.data.scope, 'authorized_works');
  assert.deepEqual(calls[0].query, { protocol_version: 1, op: 'documents.list' });
});

test('document discovery validates content selectors and their exact work attribution', async (t) => {
  let body;
  const { read } = await setup(t, () => ({ body }));
  body = reply([document(1, { content_work_id: 'task-one' })]);
  assert.equal((await read()).error.code, 'BadGateway');
  for (const value of ['', null, 1, {}, 'bad\u0000identity']) {
    body = reply([document(1, { content_work_id: value })]);
    body.data.scope = 'authorized_works';
    assert.equal((await read()).error.code, 'BadGateway');
  }
  body = reply([document(1, { content_work_id: 'another-task' })], { work: 'task-one' });
  assert.equal((await read({ work_id: 'task-one' })).error.code, 'BadGateway');
  body = reply([document(1, { content_work_id: 'task-one' })], { work: 'task-one' });
  assert.equal((await read({ work_id: 'task-one' })).ok, true);
  body = reply([document(1, { path: null, name: null, summary: null, sha256: null, byte_length: null,
    readable: false, status: 'restricted', reason: 'permission_restricted', content_work_id: 'task-one' })]);
  assert.equal((await read()).error.code, 'BadGateway');
});

test('controlled source reads forward the document-selected task without changing identity', async (t) => {
  const text = '# Delivery guide';
  const fingerprint = require('node:crypto').createHash('sha256').update(text).digest('hex');
  const body = reply([]);
  body.data = { kind: 'source', path: 'docs/guide.md', sha256: fingerprint, byte_length: Buffer.byteLength(text),
    text, snapshot: 'active', history_bypass: false, url_bypass: false, next_step: null };
  const { read, calls } = await setup(t, () => ({ body }));
  const query = { op: 'source.content', work_id: 'task-one', source_path: 'docs/guide.md', expected_sha256: fingerprint, max_context_bytes: 262144 };
  assert.equal((await read(query)).ok, true);
  assert.deepEqual(calls[0], { path: '/v1/web/projects/portal/query', cookie: 'awr_web_session=synthetic-library-reader',
    query: { protocol_version: 1, ...query } });
});
