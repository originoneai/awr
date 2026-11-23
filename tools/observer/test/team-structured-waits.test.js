'use strict';

/**
 * AWR-PPD-050: structured waits through the Observer bridge and `mapObservation`.
 * Upstream replies come verbatim from the real awr-server web route
 * (captured and compared on every run by `crates/awr-server/tests/structured_waits_mcp.rs`).
 */
const test = require('node:test');
const assert = require('node:assert/strict');
const http = require('node:http');
const fs = require('node:fs');
const path = require('node:path');
const { createTeamBridge } = require('../server/team-bridge');
const { mapObservation, mapStructuredWaits, validStructuredWaits } = require('../server/team-progress');

const FIXTURE_DIR = path.resolve(__dirname, 'fixtures');
const SNAPSHOT_PROVIDED = JSON.parse(
  fs.readFileSync(path.join(FIXTURE_DIR, 'work-snapshot-structured-waits.json'), 'utf8'),
);
const SNAPSHOT_LEGACY = JSON.parse(
  fs.readFileSync(path.join(FIXTURE_DIR, 'work-snapshot-legacy-waits.json'), 'utf8'),
);
const GRAPH_SAMPLE = JSON.parse(
  fs.readFileSync(path.join(FIXTURE_DIR, 'work-graph-structured-waits.json'), 'utf8'),
);

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

async function runRoute(teamUrl, method, urlPath, body = null) {
  const bridge = createTeamBridge({ teamUrl, port: 0 });
  const headers = new Map();
  const url = new URL(`http://127.0.0.1${urlPath}`);
  const handler = bridge.routes[`${method} ${url.pathname}`];
  assert.ok(handler, `missing route ${method} ${url.pathname}`);
  return handler(
    url,
    body,
    { headers: { cookie: 'awr_web_session=synthetic-session' } },
    { getHeader: (k) => headers.get(k), setHeader: (k, v) => headers.set(k, v) },
  );
}

test('verbatim work.snapshot sample maps provided structured waits and legacy not_provided distinctly', async (t) => {
  const upstream = await startMockUpstream(async (req, res) => {
    res.setHeader('content-type', 'application/json');
    const chunks = [];
    for await (const c of req) chunks.push(c);
    const query = JSON.parse(Buffer.concat(chunks).toString('utf8'));
    if (query.op === 'work.snapshot' && query.work_id === 'a') {
      res.end(JSON.stringify(SNAPSHOT_PROVIDED));
      return;
    }
    if (query.op === 'work.snapshot' && query.work_id === 'c') {
      res.end(JSON.stringify(SNAPSHOT_LEGACY));
      return;
    }
    res.statusCode = 404;
    res.end(JSON.stringify({ code: 'NotFound', message: 'not found' }));
  });
  t.after(() => upstream.close());

  const stream = SNAPSHOT_PROVIDED.workstream_id;
  const provided = await runRoute(
    upstream.base,
    'GET',
    `/api/team/work?project=one&work=a&workstream=${encodeURIComponent(stream)}`,
  );
  assert.equal(provided.ok, true);
  assert.equal(provided.work.structured_waits.status, 'provided');
  assert.equal(provided.work.structured_waits.grants_authority, false);
  assert.equal(provided.work.structured_waits.provenance, 'caller_declared');
  assert.deepEqual(
    provided.work.structured_waits.items,
    SNAPSHOT_PROVIDED.data.observation.structured_waits.items,
  );
  assert.equal(provided.work.structured_waits.items[0].waiting_on.kind, 'external_party');
  assert.equal(provided.work.structured_waits.items[0].waiting_on.name, 'Partner Billing Desk');

  const legacy = await runRoute(
    upstream.base,
    'GET',
    `/api/team/work?project=one&work=c&workstream=${encodeURIComponent(stream)}`,
  );
  assert.equal(legacy.ok, true);
  assert.equal(legacy.work.structured_waits.status, 'not_provided');
  assert.equal(legacy.work.structured_waits.items, null);
  assert.equal(legacy.work.structured_waits.grants_authority, false);
  assert.ok(typeof legacy.work.structured_waits.checkpoint_id === 'string');

  // Generic POST /api/team/read also accepts both verbatim work.snapshot samples.
  for (const [workId, expectedStatus] of [['a', 'provided'], ['c', 'not_provided']]) {
    const read = await runRoute(upstream.base, 'POST', '/api/team/read', {
      project: 'one',
      query: { protocol_version: 1, op: 'work.snapshot', work_id: workId, workstream_id: stream },
    });
    assert.equal(read.ok, true);
    assert.equal(read.result.data.observation.structured_waits.status, expectedStatus);
  }
});

test('verbatim work.graph sample preserves provided waits and legacy not_provided per work', async (t) => {
  const stream = GRAPH_SAMPLE.workstream_id;
  const upstream = await startMockUpstream(async (req, res) => {
    res.setHeader('content-type', 'application/json');
    if (req.url === '/v1/web/session') {
      res.end(JSON.stringify({ session_id: 'graph-reader', expires_at_ms: Date.now() + 60000 }));
      return;
    }
    const chunks = [];
    for await (const c of req) chunks.push(c);
    const query = JSON.parse(Buffer.concat(chunks).toString('utf8'));
    if (query.op === 'workstreams.list') {
      res.end(JSON.stringify({
        items: [{ id: stream, authority_version: 1, external_key: 'stream-1', title: 'Stream 1' }],
        total: 1,
        next_cursor: null,
      }));
      return;
    }
    if (query.op === 'work.graph') {
      res.end(JSON.stringify(GRAPH_SAMPLE));
      return;
    }
    res.statusCode = 404;
    res.end('{}');
  });
  t.after(() => upstream.close());

  const result = await runRoute(upstream.base, 'GET', '/api/team/graph?project=one');
  assert.equal(result.ok, true);
  const workA = result.items.find((i) => i.work_id === 'a');
  const workC = result.items.find((i) => i.work_id === 'c');
  assert.ok(workA && workC);
  assert.equal(workA.structured_waits.status, 'provided');
  assert.equal(workA.structured_waits.items.length, 3);
  assert.equal(workA.structured_waits.items[0].waiting_on.kind, 'external_party');
  assert.equal(workC.structured_waits.status, 'not_provided');
  assert.equal(workC.structured_waits.items, null);
});

test('malformed structured waits fail closed in mapObservation and bridge routes', async (t) => {
  // Missing structured_waits on a legacy server maps to not_provided with null items (never []).
  const absent = mapObservation({});
  assert.equal(absent.structured_waits.status, 'not_provided');
  assert.equal(absent.structured_waits.items, null);

  const validProvided = SNAPSHOT_PROVIDED.data.observation.structured_waits;
  assert.equal(validStructuredWaits(validProvided), true);

  const badCases = [
    // Legacy not_provided falsely carrying empty array instead of null
    { ...SNAPSHOT_LEGACY.data.observation.structured_waits, items: [] },
    // Unknown party kind
    {
      ...validProvided,
      items: [{
        ...validProvided.items[0],
        waiting_on: { kind: 'vendor', name: 'Acme' },
      }],
    },
    // Oversized name (> 80 chars)
    {
      ...validProvided,
      items: [{
        ...validProvided.items[0],
        waiting_on: { kind: 'external_party', name: '名'.repeat(81) },
      }],
    },
    // Oversized what (> 200 chars)
    {
      ...validProvided,
      items: [{
        ...validProvided.items[0],
        what: '说'.repeat(201),
      }],
    },
    // Oversized release_condition (> 200 chars)
    {
      ...validProvided,
      items: [{
        ...validProvided.items[0],
        release_condition: '解'.repeat(201),
      }],
    },
    // grants_authority flipped to true
    { ...validProvided, grants_authority: true },
  ];

  for (const malformed of badCases) {
    assert.equal(validStructuredWaits(malformed), false);
    assert.deepEqual(mapStructuredWaits(malformed), { status: 'unknown', items: null });
  }

  // Bridge rejects malformed structured_waits on both GET /api/team/work and GET /api/team/graph with BadGateway.
  const stream = SNAPSHOT_PROVIDED.workstream_id;
  const tamperedSnap = structuredClone(SNAPSHOT_PROVIDED);
  tamperedSnap.data.observation.structured_waits.items[0].waiting_on.kind = 'vendor';
  const tamperedGraph = structuredClone(GRAPH_SAMPLE);
  tamperedGraph.data.items[0].structured_waits = {
    ...SNAPSHOT_LEGACY.data.observation.structured_waits,
    items: [],
  };

  const upstream = await startMockUpstream(async (req, res) => {
    res.setHeader('content-type', 'application/json');
    if (req.url === '/v1/web/session') {
      res.end(JSON.stringify({ session_id: 'graph-reader', expires_at_ms: Date.now() + 60000 }));
      return;
    }
    const chunks = [];
    for await (const c of req) chunks.push(c);
    const query = JSON.parse(Buffer.concat(chunks).toString('utf8'));
    if (query.op === 'workstreams.list') {
      res.end(JSON.stringify({
        items: [{ id: stream, authority_version: 1, external_key: 'stream-1', title: 'Stream 1' }],
        total: 1,
        next_cursor: null,
      }));
      return;
    }
    if (query.op === 'work.snapshot') {
      res.end(JSON.stringify(tamperedSnap));
      return;
    }
    if (query.op === 'work.graph') {
      res.end(JSON.stringify(tamperedGraph));
      return;
    }
    res.statusCode = 404;
    res.end('{}');
  });
  t.after(() => upstream.close());

  const badWork = await runRoute(
    upstream.base,
    'GET',
    `/api/team/work?project=one&work=a&workstream=${encodeURIComponent(stream)}`,
  );
  assert.equal(badWork.ok, false);
  assert.equal(badWork.error.code, 'BadGateway');

  const badGraph = await runRoute(upstream.base, 'GET', '/api/team/graph?project=one');
  assert.equal(badGraph.ok, false);
  assert.equal(badGraph.error.code, 'BadGateway');

  const badRead = await runRoute(upstream.base, 'POST', '/api/team/read', {
    project: 'one',
    query: { protocol_version: 1, op: 'work.snapshot', work_id: 'a', workstream_id: stream },
  });
  assert.equal(badRead.ok, false);
  assert.equal(badRead.error.code, 'BadGateway');
});
