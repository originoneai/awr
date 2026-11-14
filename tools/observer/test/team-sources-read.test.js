'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const http = require('node:http');
const { createTeamBridge } = require('../server/team-bridge');
const at = 1793534400000;
const digest = 'a'.repeat(64);

function sourceReply(items, total = items.length, cursor = null) {
  const reasons = [...new Set(items.map((item) => item.reason).filter(Boolean))];
  const complete = reasons.length === 0;
  return { protocol_version: 1, scope_id: 'main', source_snapshot_id: 'current-documents',
    project_revision: '12', coordinator_epoch: 'current', workstream_id: 'development', authority_version: '1',
    data: { work_id: 'work-one', items, total, next_cursor: cursor, truncated: cursor !== null,
      collection: { complete, completeness: complete ? 'complete' : 'incomplete', required_total: total,
        checked_at_unix_ms: at, scope: 'work', reasons, verification_basis: 'active_source_snapshot', recovery_scope: 'page',
        recovery: items.filter((item) => item.reason).map((item) => ({ source_position: item.source_position,
          reason: item.reason, recognized: true, next_action: 'Ask the project administrator to check access' })) },
      as_of: { at_unix_ms: at, source_snapshot_id: 'current-documents', project_revision: '12' }, execution_authorized: false } };
}

function consistentSource(position = 1) {
  return { source_position: position, path: 'docs/requirements.md', status: 'consistent', reason: null,
    expected_sha256: digest, current_sha256: digest, checked_at_unix_ms: at,
    missing: { path: null, expected_sha256: null, current_sha256: null } };
}

function forbiddenSource(position = 1) {
  return { source_position: position, path: null, status: 'forbidden', reason: 'required_spec_forbidden',
    expected_sha256: null, current_sha256: null, checked_at_unix_ms: at,
    missing: { path: 'permission_restricted', expected_sha256: 'permission_restricted', current_sha256: 'permission_restricted' } };
}

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
  const pages = [sourceReply([consistentSource()], 2, 'page-two'), sourceReply([forbiddenSource(2)], 2)];
  pages[0].data.collection.complete = false;
  pages[0].data.collection.completeness = 'incomplete';
  pages[0].data.collection.reasons = ['required_spec_forbidden'];
  const { read, calls } = await sourceBridge(t, (query) => ({ body: pages[query.cursor ? 1 : 0] }));
  for (const [index, cursor] of ['', '&cursor=page-two'].entries()) {
    const result = await read(`project=portal&work_id=work-one&workstream_id=development&limit=1&max_context_bytes=65536${cursor}`);
    assert.equal(result.ok, true, JSON.stringify(result));
    assert.deepEqual(result.result.data, pages[index].data);
    assert.equal(result.freshness.authoritative.status, 'measured');
    assert.deepEqual(calls[index], {
      path: '/v1/web/projects/portal/query', cookie: 'awr_web_session=synthetic-document-reader',
      query: { protocol_version: 1, op: 'sources.inspect', work_id: 'work-one',
        workstream_id: 'development', limit: 1, max_context_bytes: 65536,
        ...(index ? { cursor: 'page-two' } : {}) },
    });
  }
});

test('source reads refuse missing attribution, contradictory metadata and forbidden disclosures', async (t) => {
  let body;
  const { read } = await sourceBridge(t, () => ({ body }));
  const mutations = [
    ['missing work identity', (r) => { delete r.data.work_id; }],
    ['different work identity', (r) => { r.data.work_id = 'must-not-leak'; }],
    ['missing workstream identity', (r) => { delete r.workstream_id; }],
    ['different workstream identity', (r) => { r.workstream_id = 'must-not-leak'; }],
    ['missing authority', (r) => { delete r.authority_version; }],
    ['missing items', (r) => { delete r.data.items; }],
    ['missing collection', (r) => { delete r.data.collection; }],
    ['different totals', (r) => { r.data.total = 2; }],
    ['contradictory readiness', (r) => { r.data.collection.complete = true; r.data.collection.completeness = 'complete'; r.data.collection.reasons = []; }],
    ['different as-of revision', (r) => { r.data.as_of.project_revision = 'earlier'; }],
    ['invalid cursor', (r) => { r.data.next_cursor = 3; }],
    ['unknown status', (r) => { r.data.items[0].status = 'must-not-leak'; }],
    ['forbidden path', (r) => { r.data.items[0].path = 'docs/must-not-leak.md'; }],
    ['forbidden expected digest', (r) => { r.data.items[0].expected_sha256 = digest; }],
    ['forbidden current digest', (r) => { r.data.items[0].current_sha256 = digest; }],
    ['hidden permission classification', (r) => { r.data.items[0].status = 'unverified'; }],
    ['missing recovery', (r) => { r.data.collection.recovery = []; }],
    ['recovery from another page', (r) => { r.data.collection.recovery[0].source_position = 2; }],
    ['unknown recovery action', (r) => { r.data.collection.recovery[0].recognized = false; }],
  ];
  for (const [name, mutate] of mutations) {
    body = sourceReply([forbiddenSource()]);
    mutate(body);
    const result = await read('project=portal&work_id=work-one&workstream_id=development');
    assert.equal(result.ok, false, name);
    assert.equal(result.error.code, 'BadGateway', name);
    assert.equal(result.result, undefined, name);
    assert.equal(JSON.stringify(result).includes('must-not-leak'), false, name);
  }
});

test('source pages cover their declared positions without missing prefixes, gaps or terminal tails', async (t) => {
  let body;
  const { read } = await sourceBridge(t, () => ({ body }));
  const scope = 'project=portal&work_id=work-one&workstream_id=development';
  const invalid = [
    ['omitted terminal tail', sourceReply([consistentSource()], 2), ''],
    ['missing first position', sourceReply([consistentSource(2)], 2), ''],
    ['noncontiguous positions', sourceReply([consistentSource(1), consistentSource(3)], 3), ''],
    ['reversed positions', sourceReply([consistentSource(2), consistentSource(1)], 2), ''],
    ['empty nonempty collection', sourceReply([], 2), ''],
    ['cursor past the last position', sourceReply([consistentSource()], 1, 'page-two'), '&limit=1'],
    ['incomplete nonterminal page', sourceReply([consistentSource()], 3, 'page-two'), '&limit=2'],
  ];
  for (const [name, reply, parameters] of invalid) {
    body = reply;
    const result = await read(scope + parameters);
    assert.equal(result.ok, false, name);
    assert.equal(result.error.code, 'BadGateway', name);
    assert.equal(result.result, undefined, name);
  }
  for (const [reply, parameters] of [
    [sourceReply([]), ''],
    [sourceReply([consistentSource(1), consistentSource(2)]), ''],
    [sourceReply([consistentSource()], 2, 'page-two'), '&limit=1'],
    [sourceReply([consistentSource(2)], 2), '&limit=1&cursor=page-two'],
  ]) {
    body = reply;
    const result = await read(scope + parameters);
    assert.equal(result.ok, true, JSON.stringify(result));
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
