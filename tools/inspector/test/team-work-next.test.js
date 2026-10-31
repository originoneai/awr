'use strict';

/**
 * The generic read adapter against the REAL work.next response shape
 * (captured from a live Team service, minimized and de-identified): the
 * upstream answer carries only { data, project_revision, protocol_version }
 * — no scope_id, no source_snapshot_id, no coordinator_epoch. work.next is
 * an identity-scoped navigation read, so the attribution envelope does not
 * apply; nothing may be fabricated for it, and its authoritative freshness
 * plane is not_measured (no comparable measurement, never borrowed).
 */
const test = require('node:test');
const assert = require('node:assert');
const http = require('http');
const path = require('path');
const { createTeamBridge } = require('../team-bridge');

const FIXTURE_DIR = path.resolve(__dirname, 'fixtures');

// Minimal fixture with the exact top-level shape of the real upstream
// response (live sample lin-upstream-work-next.json, de-identified).
const REAL_WORK_NEXT = {
  data: {
    identity: { actor_id: 'agent-1', role: 'developer' },
    items: [
      {
        navigation: 'resume',
        work_id: 'issue-filter-api',
        workstream_id: 'WS1',
        state: 'claimed',
        title: 'issue-filter-api',
        responsibility: { relation: 'owned_by_me', owner: 'lin', version: '3' },
        next_query: { op: 'work.observe', protocol_version: 1, work_id: 'issue-filter-api', workstream_id: 'WS1' },
      },
      {
        navigation: 'prepare',
        work_id: 'issue-filter-page',
        workstream_id: 'WS1',
        state: null,
        title: 'issue-filter-page',
        responsibility: { relation: 'pool', owner: null, version: '0' },
        next_query: { op: 'work.prepare', protocol_version: 1, work_id: 'issue-filter-page', workstream_id: 'WS1' },
      },
    ],
    resume: [
      {
        work_id: 'issue-filter-api',
        session_id: 'session-1',
        session_version: '2',
        next_query: { op: 'work.observe', protocol_version: 1, work_id: 'issue-filter-api', workstream_id: 'WS1' },
      },
    ],
    next_cursor: null,
    next_action: 'Resume own current-client sessions first; navigation is advisory.',
    recheck_on: ['progress', 'source_change'],
    execution_authorized: false,
  },
  project_revision: '42',
  protocol_version: 1,
};

function startMockUpstream(payloadFor) {
  const server = http.createServer((req, res) => {
    let raw = '';
    req.on('data', (c) => { raw += c; });
    req.on('end', () => {
      const query = JSON.parse(raw || '{}');
      res.setHeader('content-type', 'application/json');
      res.end(JSON.stringify(payloadFor(query)));
    });
  });
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
  const sockets = new Set();
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
  server.on('connection', (socket) => {
    sockets.add(socket);
    socket.on('close', () => sockets.delete(socket));
  });
  return new Promise((resolve) => {
    server.listen(0, '127.0.0.1', () => {
      const { port } = server.address();
      resolve({
        base: `http://127.0.0.1:${port}`,
        close: async () => {
          // fetch() keep-alive connections would otherwise hold server.close.
          for (const socket of sockets) socket.destroy();
          await new Promise((r) => server.close(r));
        },
      });
    });
  });
}

async function readWorkNext(base, query) {
  const response = await fetch(`${base}/api/team/read`, {
    method: 'POST',
    headers: { 'content-type': 'application/json', 'x-awr-observer': '1' },
    body: JSON.stringify({ project: 'demo', query: { protocol_version: 1, op: 'work.next', ...query } }),
  });
  return response.json();
}

async function withBridge(payloadFor, fn) {
  const upstream = await startMockUpstream(payloadFor);
  const bridge = await startLiveBridge(upstream.base);
  try {
    return await fn(bridge.base);
  } finally {
    await bridge.close();
    await upstream.close();
  }
}

test('a real work.next response passes the generic read with its items/resume intact', async () => {
  await withBridge(() => REAL_WORK_NEXT, async (base) => {
    const reply = await readWorkNext(base, {});
    assert.equal(reply.ok, true, JSON.stringify(reply));
    assert.deepEqual(reply.result.data.items, REAL_WORK_NEXT.data.items);
    assert.deepEqual(reply.result.data.resume, REAL_WORK_NEXT.data.resume);
    assert.equal(reply.result.data.next_cursor, null);
    // No envelope was fabricated for the identity-scoped read.
    assert.equal(reply.result.scope_id, undefined);
    assert.equal(reply.result.source_snapshot_id, undefined);
    assert.equal(reply.result.coordinator_epoch, undefined);
  });
});

test('work.next freshness: authoritative not_measured, page_read measured, no borrowed clock', async () => {
  await withBridge(() => REAL_WORK_NEXT, async (base) => {
    const reply = await readWorkNext(base, {});
    assert.equal(reply.ok, true, JSON.stringify(reply));
    // A bare project_revision is not a comparable measurement: the plane
    // says not_measured instead of borrowing the page clock or claiming one.
    assert.deepEqual(reply.freshness.authoritative, { status: 'not_measured' });
    assert.deepEqual(reply.freshness.report_received, { status: 'not_measured' });
    assert.equal(reply.freshness.page_read.status, 'measured');
    assert.ok(Number.isFinite(reply.freshness.page_read.at_unix_ms));
  });
});

test('work.next still refuses execution_authorized: true', async () => {
  const forged = JSON.parse(JSON.stringify(REAL_WORK_NEXT));
  forged.data.execution_authorized = true;
  await withBridge(() => forged, async (base) => {
    const reply = await readWorkNext(base, {});
    assert.equal(reply.ok, false);
    assert.equal(reply.error.code, 'BadGateway');
  });
});

test('work.next still refuses malformed responses', async () => {
  for (const [name, result] of [
    ['wrong protocol', { ...REAL_WORK_NEXT, protocol_version: 2 }],
    ['data not an object', { ...REAL_WORK_NEXT, data: [] }],
    ['invalid cursor', {
      ...REAL_WORK_NEXT,
      data: { ...REAL_WORK_NEXT.data, next_cursor: 42 },
    }],
  ]) {
    await withBridge(() => result, async (base) => {
      const reply = await readWorkNext(base, {});
      assert.equal(reply.ok, false, name);
      assert.equal(reply.error.code, 'BadGateway', name);
    });
  }
});

test('work.inbox keeps the full attribution envelope requirement', async () => {
  // The same shape as work.next must still fail for envelope-carrying reads.
  await withBridge(() => REAL_WORK_NEXT, async (base) => {
    const response = await fetch(`${base}/api/team/read`, {
      method: 'POST',
      headers: { 'content-type': 'application/json', 'x-awr-observer': '1' },
      body: JSON.stringify({ project: 'demo', query: { protocol_version: 1, op: 'work.inbox' } }),
    });
    const reply = await response.json();
    assert.equal(reply.ok, false);
    assert.equal(reply.error.code, 'BadGateway');
  });
});

test('a fully-enveloped work.next response still passes (no behavior change for fixtures)', async () => {
  const enveloped = {
    ...REAL_WORK_NEXT,
    scope_id: 'main',
    source_snapshot_id: 'snap-1',
    coordinator_epoch: 'epoch-1',
  };
  await withBridge(() => enveloped, async (base) => {
    const reply = await readWorkNext(base, {});
    assert.equal(reply.ok, true, JSON.stringify(reply));
    // With a real measurement present the authoritative plane is measured.
    assert.equal(reply.freshness.authoritative.status, 'measured');
    assert.equal(reply.freshness.authoritative.project_revision, '42');
  });
});
