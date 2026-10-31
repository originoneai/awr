'use strict';

/**
 * AWR-OBS-001 read adapter: one bounded, authorized, read-only entry for the
 * observation workspace. Every implemented Team query is exposed except the
 * deferred full ops audit (audit.history/audit.export/audit.count); commands
 * are never forwarded. Selectors, sizes and the response envelope are checked
 * per op; the upstream service reauthorizes the caller's own cookie session.
 */
const test = require('node:test');
const assert = require('node:assert');
const http = require('http');
const { createTeamBridge } = require('../team-bridge');

const FIXTURE_DIR = require('path').resolve(__dirname, 'fixtures');

function startMockUpstream(handler) {
  const server = http.createServer((req, res) => handler(req, res));
  const sockets = new Set();
  server.on('connection', (socket) => {
    sockets.add(socket);
    socket.on('close', () => sockets.delete(socket));
  });
  return new Promise((resolve) => {
    server.listen(0, '127.0.0.1', () => {
      const { port } = server.address();
      resolve({
        base: `http://127.0.0.1:${port}`,
        close: () => new Promise((r) => {
          for (const socket of sockets) socket.destroy();
          server.close(r);
        }),
      });
    });
  });
}

function startLiveBridge(teamUrl) {
  const bridge = createTeamBridge({ teamUrl, teamFixtureDir: FIXTURE_DIR, port: 0 });
  const server = http.createServer(async (req, res) => {
    const url = new URL(req.url, 'http://127.0.0.1');
    const handler = bridge.routes[`${req.method} ${url.pathname}`];
    if (!handler) {
      res.writeHead(404, { 'content-type': 'application/json' });
      res.end('{}');
      return;
    }
    let body = null;
    if (req.method === 'POST') {
      const chunks = [];
      for await (const c of req) chunks.push(c);
      const raw = Buffer.concat(chunks).toString('utf8');
      body = raw ? JSON.parse(raw) : {};
    }
    const json = await handler(url, body, req, res);
    res.writeHead(200, { 'content-type': 'application/json' });
    res.end(JSON.stringify(json));
  });
  return new Promise((resolve) => {
    server.listen(0, '127.0.0.1', () => {
      const { port } = server.address();
      resolve({ base: `http://127.0.0.1:${port}`, close: () => new Promise((r) => server.close(r)) });
    });
  });
}

function req(base, method, path, { body } = {}) {
  return new Promise((resolve, reject) => {
    const request = http.request(base + path, { method, headers: { 'content-type': 'application/json' } }, (res) => {
      const chunks = [];
      res.on('data', (c) => chunks.push(c));
      res.on('end', () => resolve({ status: res.statusCode, json: JSON.parse(Buffer.concat(chunks).toString('utf8')) }));
    });
    request.on('error', reject);
    if (body !== undefined) request.write(JSON.stringify(body));
    request.end();
  });
}

const ENVELOPE = {
  protocol_version: 1,
  scope_id: 'main',
  source_snapshot_id: 'snap-1',
  project_revision: '20',
  coordinator_epoch: 'epoch-1',
};

// One minimal valid selector set per registered read op. audit.history,
// audit.export and audit.count are deliberately absent (deferred full ops
// audit); no command op is present.
const VALID_QUERIES = {
  'capabilities': {},
  'workstreams.list': { limit: 10 },
  'work.list': { workstream_id: 'stream' },
  'work.search': { search: 'observation', workstream_id: 'stream', limit: 25, cursor: 'opaque-cursor' },
  'work.next': { limit: 5 },
  'work.inbox': { limit: 20, max_context_bytes: 65536 },
  'task.assignees': { work_id: 'WORK', search: 'agent' },
  'work.prepare': { work_id: 'WORK', workstream_id: 'stream' },
  'work.admission': { work_id: 'WORK' },
  'work.snapshot': { work_id: 'WORK', workstream_id: 'stream', max_context_bytes: 262144 },
  'work.observe': { session_id: 'session-1' },
  'work.recovery': { work_id: 'WORK' },
  'events.list': { work_id: 'WORK', limit: 50 },
  'session.inspect': { session_id: 'session-1' },
  'command.inspect': { work_id: 'WORK', request_id: 'request-1' },
  'claim.inspect': { work_id: 'WORK', claim_id: 'claim-1' },
  'execution.inspect': { work_id: 'WORK', execution_id: 'execution-1' },
  'handoff.inspect': { work_id: 'WORK', handoff_id: 'handoff-1' },
  'evidence.inspect': { work_id: 'WORK', evidence_id: 'evidence-1' },
  'review.inspect': { work_id: 'WORK', review_round_id: 'round-1', max_context_bytes: 65536 },
  'completion.inspect': { work_id: 'WORK' },
  'delivery.inspect': { work_id: 'WORK' },
  'delivery.neutral.inspect': { work_id: 'WORK', workstream_id: 'stream' },
  'delivery.submission.describe': { work_id: 'WORK' },
  'delivery.neutral.outcome': { work_id: 'WORK', request_id: 'request-1' },
  'delivery.source.status': { work_id: 'WORK' },
  'delivery.integration.inspect': { work_id: 'WORK', request_id: 'request-1' },
  'source.content': { source_path: 'docs/GOALS.md', expected_sha256: 'a'.repeat(64), max_context_bytes: 65536 },
  'artifact.content': { work_id: 'WORK', export_id: 'export-1', expected_sha256: 'a'.repeat(64), max_context_bytes: 1048576 },
  'artifact.segment': { work_id: 'WORK', artifact_id: 'artifact-1', expected_sha256: 'b'.repeat(64), max_context_bytes: 4096, cursor: 'segment-cursor' },
  'delivery.exports': { work_id: 'WORK', limit: 20, cursor: 'opaque-cursor' },
  'planning.outcome': { request_id: 'request-1', workstream_id: 'stream' },
  'audit.requests': { work_id: 'WORK', member_actor_id: 'member-1', limit: 50, cursor: 'audit-cursor' },
  'audit.development': { category: 'execution', include_denies: true, limit: 50 },
};

test('read adapter forwards every registered read op with its exact selectors, unchanged', async () => {
  const calls = [];
  const upstream = await startMockUpstream((request, response) => {
    const chunks = [];
    request.on('data', (c) => chunks.push(c));
    request.on('end', () => {
      const parsed = JSON.parse(Buffer.concat(chunks).toString('utf8'));
      calls.push({ path: request.url, query: parsed });
      response.end(JSON.stringify(parsed.op === 'capabilities'
        ? { protocol: 'awr-team-workstream', protocol_version: 1, queries: [], commands: [] }
        : { ...ENVELOPE, data: { items: [], next_cursor: null } }));
    });
  });
  const bridge = await startLiveBridge(upstream.base);
  try {
    for (const [op, selectors] of Object.entries(VALID_QUERIES)) {
      const query = { protocol_version: 1, op, ...selectors };
      const read = await req(bridge.base, 'POST', '/api/team/read', { body: { project: 'demo', query } });
      assert.equal(read.json.ok, true, `${op}: ${JSON.stringify(read.json)}`);
      assert.deepEqual(read.json.query, query);
      assert.deepEqual(calls.pop(), { path: '/v1/web/projects/demo/query', query });
    }
    assert.equal(calls.length, 0);
  } finally {
    await bridge.close();
    await upstream.close();
  }
});

test('read adapter rejects commands, the deferred full ops audit and unknown ops without forwarding', async () => {
  let calls = 0;
  const upstream = await startMockUpstream((_request, response) => { calls++; response.end('{}'); });
  const bridge = await startLiveBridge(upstream.base);
  try {
    for (const op of ['claim.acquire', 'work.complete', 'delivery.adopt', 'session.start',
      'audit.history', 'audit.export', 'audit.count', 'work.drop', '']) {
      const read = await req(bridge.base, 'POST', '/api/team/read',
        { body: { project: 'demo', query: { protocol_version: 1, op } } });
      assert.equal(read.json.ok, false, op);
      assert.equal(read.json.error.code, 'Unsupported', op);
      assert.equal(read.json.result, undefined, op);
    }
    assert.equal(calls, 0);
  } finally {
    await bridge.close();
    await upstream.close();
  }
});

test('read adapter rejects forged identity fields, malformed selectors and oversize budgets before any upstream call', async () => {
  let calls = 0;
  const upstream = await startMockUpstream((_request, response) => { calls++; response.end('{}'); });
  const bridge = await startLiveBridge(upstream.base);
  try {
    const cases = [
      { protocol_version: 2, op: 'capabilities' },
      { protocol_version: 1, op: 'capabilities', work_id: 'WORK' },
      { protocol_version: 1, op: 'work.next', work_id: 'WORK' },
      { protocol_version: 1, op: 'work.next', workstream_id: 'stream' },
      { protocol_version: 1, op: 'work.search' },
      { protocol_version: 1, op: 'work.search', search: '' },
      { protocol_version: 1, op: 'work.search', search: 'x'.repeat(513) },
      { protocol_version: 1, op: 'work.search', search: 'x', actor_id: 'administrator' },
      { protocol_version: 1, op: 'work.search', search: 'x', work_id: 'WORK' },
      { protocol_version: 1, op: 'work.search', search: 'x', limit: 0 },
      { protocol_version: 1, op: 'work.search', search: 'x', limit: 101 },
      { protocol_version: 1, op: 'work.search', search: 'x', limit: 1.5 },
      { protocol_version: 1, op: 'work.search', search: 'x', cursor: 'x'.repeat(4097) },
      { protocol_version: 1, op: 'work.snapshot', work_id: 'WORK', cursor: 'opaque' },
      { protocol_version: 1, op: 'work.snapshot', work_id: 'WORK', limit: 10 },
      { protocol_version: 1, op: 'work.snapshot', work_id: 'WORK', max_context_bytes: 262145 },
      { protocol_version: 1, op: 'work.snapshot' },
      { protocol_version: 1, op: 'review.inspect', work_id: 'WORK' },
      { protocol_version: 1, op: 'evidence.inspect', work_id: 'WORK' },
      { protocol_version: 1, op: 'planning.outcome' },
      { protocol_version: 1, op: 'planning.outcome', request_id: 'request-1', work_id: 'WORK' },
      { protocol_version: 1, op: 'delivery.exports', workstream_id: 'stream' },
      { protocol_version: 1, op: 'delivery.neutral.inspect', session_id: 'session-1' },
      { protocol_version: 1, op: 'artifact.content', work_id: 'WORK' },
      { protocol_version: 1, op: 'artifact.content', work_id: 'WORK', artifact_id: 'a', export_id: 'e' },
      { protocol_version: 1, op: 'artifact.content', work_id: 'WORK', export_id: 'e', max_context_bytes: 1048577 },
      { protocol_version: 1, op: 'artifact.segment', work_id: 'WORK', export_id: 'e' },
      { protocol_version: 1, op: 'artifact.segment', work_id: 'WORK', export_id: 'e', expected_sha256: 'zz' },
      { protocol_version: 1, op: 'artifact.segment', work_id: 'WORK', export_id: 'e', expected_sha256: 'a'.repeat(64), max_context_bytes: 4097 },
      { protocol_version: 1, op: 'artifact.segment', work_id: 'WORK', export_id: 'e', expected_sha256: 'a'.repeat(64), limit: 1 },
      { protocol_version: 1, op: 'source.content', source_path: '../secret' },
      { protocol_version: 1, op: 'source.content', source_path: '/absolute' },
      { protocol_version: 1, op: 'source.content', expected_sha256: 'a'.repeat(64) },
      { protocol_version: 1, op: 'audit.requests', workstream_id: 'stream' },
      { protocol_version: 1, op: 'audit.requests', session_id: 'session-1' },
      { protocol_version: 1, op: 'audit.requests', include_denies: 'yes' },
      { protocol_version: 1, op: 'session.inspect' },
      { protocol_version: 1, op: 'work.prepare', work_id: 'WORK', expected_sha256: 'a'.repeat(64) },
      { protocol_version: 1, op: 'capabilities', role: 'project_admin' },
    ];
    for (const query of cases) {
      const read = await req(bridge.base, 'POST', '/api/team/read', { body: { project: 'demo', query } });
      assert.equal(read.json.ok, false, JSON.stringify(query));
      assert.equal(read.json.error.code, 'InvalidInput', JSON.stringify(query));
      assert.equal(read.json.result, undefined, JSON.stringify(query));
      assert.ok(!JSON.stringify(read.json).includes('administrator'), 'never echo selector values');
    }
    assert.equal(calls, 0);
    // A missing or oversized project selector is likewise refused.
    for (const body of [{ query: { protocol_version: 1, op: 'capabilities' } },
      { project: 'x'.repeat(129), query: { protocol_version: 1, op: 'capabilities' } },
      { project: 'demo', query: { protocol_version: 1, op: 'capabilities' }, extra: true }]) {
      const read = await req(bridge.base, 'POST', '/api/team/read', { body });
      assert.equal(read.json.error.code, 'InvalidInput');
    }
    assert.equal(calls, 0);
  } finally {
    await bridge.close();
    await upstream.close();
  }
});

test('read adapter maps upstream denial, staleness, timeout and partial pages to explainable results', async () => {
  let behavior = { status: 401, body: { code: 'Unauthenticated', message: 'no web session' } };
  let stall = false;
  const upstream = await startMockUpstream((request, response) => {
    const chunks = [];
    request.on('data', (c) => chunks.push(c));
    request.on('end', () => {
      if (stall) return; // never answer: the bridge bound must fire
      response.writeHead(behavior.status, { 'content-type': 'application/json' });
      response.end(JSON.stringify(behavior.body));
    });
  });
  const bridge = await startLiveBridge(upstream.base);
  const query = { protocol_version: 1, op: 'work.search', search: 'observation' };
  try {
    let read = await req(bridge.base, 'POST', '/api/team/read', { body: { project: 'demo', query } });
    assert.equal(read.json.error.code, 'Unauthenticated');
    behavior = { status: 403, body: { code: 'Forbidden', message: 'work outside the authorized scope' } };
    read = await req(bridge.base, 'POST', '/api/team/read', { body: { project: 'demo', query } });
    assert.equal(read.json.error.code, 'Forbidden');
    behavior = { status: 400, body: { code: 'CursorExpired', message: 'cursor predates the current snapshot' } };
    read = await req(bridge.base, 'POST', '/api/team/read', { body: { project: 'demo', query: { ...query, cursor: 'stale' } } });
    assert.equal(read.json.error.code, 'CursorExpired');
    behavior = { status: 200, body: { ...ENVELOPE, data: { items: [{ work_id: 'a' }], total: 7, next_cursor: 'more', facts_truncated: true } } };
    read = await req(bridge.base, 'POST', '/api/team/read', { body: { project: 'demo', query } });
    assert.equal(read.json.ok, true);
    assert.equal(read.json.result.data.next_cursor, 'more');
    assert.equal(read.json.result.data.facts_truncated, true);
    stall = true;
    const started = Date.now();
    read = await req(bridge.base, 'POST', '/api/team/read', { body: { project: 'demo', query } });
    assert.equal(read.json.error.code, 'ReadTimeout');
    assert.ok(Date.now() - started < 6500);
  } finally {
    await bridge.close();
    await upstream.close();
  }
});

test('read adapter refuses oversized or malformed upstream envelopes without exposing data', async () => {
  let result;
  const upstream = await startMockUpstream((_request, response) => response.end(JSON.stringify(result)));
  const bridge = await startLiveBridge(upstream.base);
  const query = { protocol_version: 1, op: 'work.next' };
  try {
    for (const candidate of [
      { ...ENVELOPE, protocol_version: 2 },
      { ...ENVELOPE, data: null },
      { ...ENVELOPE },
      { ...ENVELOPE, data: ['not-an-object'] },
      { ...ENVELOPE, data: { items: [], next_cursor: 'x'.repeat(4097) } },
    ]) {
      result = candidate;
      const read = await req(bridge.base, 'POST', '/api/team/read', { body: { project: 'demo', query } });
      assert.equal(read.json.error.code, 'BadGateway', JSON.stringify(candidate));
      assert.equal(read.json.result, undefined);
    }
    result = { ...ENVELOPE, data: { items: [], padded: 'x'.repeat(2 * 1024 * 1024) } };
    const read = await req(bridge.base, 'POST', '/api/team/read', { body: { project: 'demo', query } });
    assert.equal(read.json.error.code, 'ResponseTooLarge');
    assert.equal(read.json.result, undefined);
  } finally {
    await bridge.close();
    await upstream.close();
  }
});

test('read adapter never fabricates demo reads', async () => {
  const bridge = createTeamBridge({ teamFixtureDir: FIXTURE_DIR, port: 0 });
  const server = http.createServer(async (req, res) => {
    const url = new URL(req.url, 'http://127.0.0.1');
    const handler = bridge.routes[`${req.method} ${url.pathname}`];
    if (!handler) {
      res.writeHead(404, { 'content-type': 'application/json' });
      res.end('{}');
      return;
    }
    let body = null;
    const chunks = [];
    for await (const c of req) chunks.push(c);
    const raw = Buffer.concat(chunks).toString('utf8');
    body = raw ? JSON.parse(raw) : {};
    const json = await handler(url, body, req, res);
    res.writeHead(200, { 'content-type': 'application/json' });
    res.end(JSON.stringify(json));
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  const base = `http://127.0.0.1:${server.address().port}`;
  try {
    const read = await req(base, 'POST', '/api/team/read',
      { body: { project: 'demo', query: { protocol_version: 1, op: 'capabilities' } } });
    assert.equal(read.json.ok, false);
    assert.equal(read.json.error.code, 'Unsupported');
  } finally {
    await new Promise((r) => server.close(r));
  }
});
