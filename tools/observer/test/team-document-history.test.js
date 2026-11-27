'use strict';

/**
 * AWR-PPD-053: Observer read bridge tests for `document.history` (`POST /api/team/read`
 * and `GET /api/team/document-history`). Every upstream response served here is a
 * verbatim web-route reply captured from the real `awr-server` binary against
 * PostgreSQL (`crates/awr-server/tests/samples/document-history-web-*.json`),
 * never a handwritten demo shape.
 */
const test = require('node:test');
const assert = require('node:assert/strict');
const http = require('node:http');
const fs = require('node:fs');
const path = require('node:path');
const { createTeamBridge } = require('../server/team-bridge');

const FIXTURE_DIR = path.resolve(__dirname, 'fixtures');
const SAMPLE_OK = JSON.parse(
  fs.readFileSync(path.join(FIXTURE_DIR, 'document-history-web-read.json'), 'utf8'),
);
const SAMPLE_NOT_FOUND = JSON.parse(
  fs.readFileSync(path.join(FIXTURE_DIR, 'document-history-web-not-found.json'), 'utf8'),
);
const SAMPLE_FORBIDDEN = JSON.parse(
  fs.readFileSync(path.join(FIXTURE_DIR, 'document-history-web-forbidden.json'), 'utf8'),
);

async function setup(t, respond) {
  const calls = [];
  const upstream = http.createServer(async (req, res) => {
    const chunks = [];
    for await (const chunk of req) chunks.push(chunk);
    const query = JSON.parse(Buffer.concat(chunks).toString('utf8'));
    calls.push({ path: req.url, cookie: req.headers.cookie, query });
    const { status = 200, body } = respond(query);
    res.writeHead(status, { 'content-type': 'application/json' });
    res.end(JSON.stringify(body));
  });
  await new Promise((resolve) => upstream.listen(0, '127.0.0.1', resolve));
  t.after(() => new Promise((resolve) => {
    upstream.close(resolve);
    upstream.closeAllConnections();
  }));
  const bridge = createTeamBridge({
    teamUrl: `http://127.0.0.1:${upstream.address().port}`,
    port: 0,
  });
  const reqHeaders = { headers: { cookie: 'awr_web_session=synthetic-history-reader' } };
  const resStub = { getHeader: () => undefined, setHeader: () => {} };
  const readPost = (query = {}) => bridge.routes['POST /api/team/read'](
    new URL('http://localhost/api/team/read'),
    { project: 'portal', query: { protocol_version: 1, op: 'document.history', ...query } },
    reqHeaders,
    resStub,
  );
  const readGet = (params) => bridge.routes['GET /api/team/document-history'](
    new URL(`http://localhost/api/team/document-history?${params}`),
    null,
    reqHeaders,
    resStub,
  );
  return { readPost, readGet, calls };
}

test('document.history verbatim sample passes through POST /api/team/read and GET /api/team/document-history under the observation contract', async (t) => {
  const { readPost, readGet, calls } = await setup(t, () => ({ body: SAMPLE_OK }));

  const postReply = await readPost({ source_path: 'docs/alpha.md', max_context_bytes: 65536 });
  assert.equal(postReply.ok, true, JSON.stringify(postReply));
  assert.deepEqual(postReply.result, SAMPLE_OK);
  assert.equal(postReply.freshness.authoritative.status, 'measured');
  assert.equal(postReply.freshness.authoritative.project_revision, SAMPLE_OK.project_revision);
  assert.equal(postReply.freshness.authoritative.source_snapshot_id, SAMPLE_OK.source_snapshot_id);
  assert.equal(postReply.freshness.page_read.status, 'measured');
  assert.deepEqual(calls[0], {
    path: '/v1/web/projects/portal/query',
    cookie: 'awr_web_session=synthetic-history-reader',
    query: {
      protocol_version: 1,
      op: 'document.history',
      source_path: 'docs/alpha.md',
      max_context_bytes: 65536,
    },
  });

  const getReply = await readGet('project=portal&source_path=docs%2Falpha.md&max_context_bytes=65536');
  assert.equal(getReply.ok, true, JSON.stringify(getReply));
  assert.deepEqual(getReply.result, SAMPLE_OK);
  assert.equal(getReply.freshness.authoritative.status, 'measured');
  assert.equal(getReply.freshness.page_read.status, 'measured');
  assert.deepEqual(calls[1], calls[0]);
});

test('document.history preserves verbatim missing-path and forbidden error categories without inventing empty history', async (t) => {
  const { readPost, readGet } = await setup(t, (query) => (
    query.source_path === 'docs/missing-alpha.md'
      ? { status: 400, body: SAMPLE_NOT_FOUND }
      : { status: 403, body: SAMPLE_FORBIDDEN }
  ));

  const missing = await readGet('project=portal&source_path=docs%2Fmissing-alpha.md');
  assert.equal(missing.ok, false);
  assert.equal(missing.error.code, 'InvalidInput');
  assert.equal(missing.result, undefined);
  assert.equal(missing.freshness.authoritative.status, 'failed');
  assert.equal(missing.freshness.authoritative.error_code, 'InvalidInput');

  const forbidden = await readPost({ source_path: 'docs/private-beta.md' });
  assert.equal(forbidden.ok, false);
  assert.equal(forbidden.error.code, 'Forbidden');
  assert.equal(forbidden.result, undefined);
  assert.equal(forbidden.freshness.authoritative.status, 'failed');
  assert.equal(forbidden.freshness.authoritative.error_code, 'Forbidden');
});

test('document.history rejects invalid selectors and unexpected query parameters before contacting upstream', async (t) => {
  const { readPost, readGet, calls } = await setup(t, () => ({ body: SAMPLE_OK }));

  for (const query of [
    {},
    { source_path: '../secret.md' },
    { source_path: '/etc/passwd' },
    { source_path: 'docs/alpha.md', session_id: 'session-1' },
    { source_path: 'docs/alpha.md', workstream_id: 'stream-1' },
    { source_path: 'docs/alpha.md', limit: 10 },
    { source_path: 'docs/alpha.md', cursor: 'opaque' },
    { source_path: 'docs/alpha.md', search: 'alpha' },
    { source_path: 'docs/alpha.md', expected_sha256: 'a'.repeat(64) },
    { source_path: 'docs/alpha.md', max_context_bytes: 262145 },
    { source_path: 'docs/alpha.md', actor_id: 'forged' },
  ]) {
    const res = await readPost(query);
    assert.equal(res.ok, false, JSON.stringify(query));
    assert.equal(res.error.code, 'InvalidInput', JSON.stringify(query));
  }

  for (const params of [
    'project=portal',
    'project=portal&source_path=docs%2Falpha.md&limit=5',
    'project=portal&source_path=docs%2Falpha.md&source_path=docs%2Fother.md',
    'project=portal&source_path=docs%2Falpha.md&max_context_bytes=not-a-number',
  ]) {
    const res = await readGet(params);
    assert.equal(res.ok, false, params);
    assert.equal(res.error.code, 'InvalidInput', params);
  }
  assert.equal(calls.length, 0);
});

test('document.history refuses tampered, empty, unbound or execution-authorizing upstream replies', async (t) => {
  let body;
  const { readPost } = await setup(t, () => ({ body }));
  const mutations = [
    (r) => { r.data.path = 'docs/other.md'; },
    (r) => { r.data.items = []; r.data.total = 0; },
    (r) => { r.data.limit = 50; },
    (r) => { r.data.order = 'oldest_first'; },
    (r) => { r.data.read_only = false; },
    (r) => { r.data.execution_authorized = true; },
    (r) => { r.data.truncated = true; },
    (r) => { r.data.items[0].digest_prefix = 'short'; },
    (r) => { r.data.items[0].digest_prefix = 'A'.repeat(12); },
    (r) => { r.data.items[0].extra = 'must-not-leak'; },
    (r) => { r.data.items[0].submitted_by = { person_id: 'agent', display_name: '' }; },
    (r) => { r.data.items[1].source_snapshot_id = r.data.items[0].source_snapshot_id; },
    (r) => { r.data.as_of.project_revision = '999999'; },
  ];
  for (const mutate of mutations) {
    body = structuredClone(SAMPLE_OK);
    mutate(body);
    const res = await readPost({ source_path: 'docs/alpha.md' });
    assert.equal(res.ok, false, mutate.toString());
    assert.equal(res.error.code, 'BadGateway', mutate.toString());
    assert.equal(res.result, undefined);
    assert.equal(JSON.stringify(res).includes('must-not-leak'), false);
  }
});
