'use strict';

/**
 * AWR-OBS-002 unified observation freshness, coverage and partial-failure
 * contract. One shared semantics for every Inspector Team read: three clocks
 * (report received, authoritative measurement, page read) are presented
 * separately and never substitute for each other; composed lists declare
 * their authorized scope, enumerated/succeeded/failed/truncated coverage and
 * per-part as-of instead of claiming a capped enumeration is everything; a
 * non-auth partial failure retains the confirmed parts with their as-of while
 * an auth/permission revocation clears restricted content at once; and a
 * mixed-revision composition never claims a common version it does not have.
 */
const test = require('node:test');
const assert = require('node:assert');
const http = require('http');
const path = require('path');
const { createTeamBridge } = require('../team-bridge');

const FIXTURE_DIR = path.resolve(__dirname, 'fixtures');

const ENVELOPE = {
  protocol_version: 1,
  scope_id: 'main',
  source_snapshot_id: 'snap-1',
  project_revision: '20',
  coordinator_epoch: 'epoch-1',
};

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

test('read responses carry three distinct clocks and never substitute one for another', async () => {
  let report = { reported_at_unix_ms: 1000, recorded_project_revision: '19' };
  const upstream = await startMockUpstream((_request, response) => {
    response.end(JSON.stringify({
      ...ENVELOPE, workstream_id: 'stream',
      data: {
        work_id: 'WORK',
        snapshot: { version: 1, project_revision: '20', source_snapshot_id: 'snap-1', coordinator_epoch: 'epoch-1', queried_at_unix_ms: 1234 },
        ...(report ? { observation: { progress: report } } : {}),
      },
    }));
  });
  const bridge = await startLiveBridge(upstream.base);
  const query = { protocol_version: 1, op: 'work.snapshot', work_id: 'WORK', workstream_id: 'stream' };
  try {
    const started = Date.now();
    const read = await req(bridge.base, 'POST', '/api/team/read', { body: { project: 'demo', query } });
    assert.equal(read.json.ok, true, JSON.stringify(read.json));
    const freshness = read.json.freshness;
    assert.deepEqual(freshness.report_received, { status: 'measured', at_unix_ms: 1000, project_revision: '19' });
    assert.equal(freshness.authoritative.status, 'measured');
    assert.equal(freshness.authoritative.at_unix_ms, 1234);
    assert.equal(freshness.authoritative.project_revision, '20');
    assert.equal(freshness.authoritative.source_snapshot_id, 'snap-1');
    assert.equal(freshness.authoritative.coordinator_epoch, 'epoch-1');
    assert.equal(freshness.page_read.status, 'measured');
    assert.ok(freshness.page_read.at_unix_ms >= started && freshness.page_read.at_unix_ms <= Date.now());
    // The report clock (revision 19, t=1000) is not "refreshed" into the page
    // clock, and the page clock is not backdated into the authoritative one.
    assert.notEqual(freshness.report_received.at_unix_ms, freshness.page_read.at_unix_ms);
    assert.notEqual(freshness.authoritative.project_revision, freshness.report_received.project_revision);

    report = null;
    const unmeasured = await req(bridge.base, 'POST', '/api/team/read', { body: { project: 'demo', query } });
    assert.equal(unmeasured.json.ok, true);
    assert.deepEqual(unmeasured.json.freshness.report_received, { status: 'not_measured' });
    assert.equal(unmeasured.json.freshness.authoritative.status, 'measured');
    assert.equal(unmeasured.json.freshness.page_read.status, 'measured');
  } finally {
    await bridge.close();
    await upstream.close();
  }
});

test('generic read verifies the response belongs to the requested work, workstream and scope', async () => {
  let result;
  const upstream = await startMockUpstream((_request, response) => response.end(JSON.stringify(result)));
  const bridge = await startLiveBridge(upstream.base);
  const base = { ...ENVELOPE, workstream_id: 'stream', data: { work_id: 'WORK' } };
  try {
    for (const candidate of [
      { ...base, data: { work_id: 'OTHER' } },
      { ...base, workstream_id: 'private-stream' },
      { ...base, scope_id: 'private-scope' },
      { ...base, data: { work_id: 'WORK', execution_authorized: true } },
      { ...base, data: { work_id: 'WORK', review: { work_id: 'OTHER' } } },
    ]) {
      result = candidate;
      const read = await req(bridge.base, 'POST', '/api/team/read', {
        body: { project: 'demo', query: { protocol_version: 1, op: 'work.snapshot', work_id: 'WORK', workstream_id: 'stream' } },
      });
      assert.equal(read.json.error.code, 'BadGateway', JSON.stringify(candidate));
      assert.equal(read.json.result, undefined);
    }
    // A mismatched review round is not the requested object either.
    result = { ...base, data: { work_id: 'WORK', review: { work_id: 'WORK', round_id: 'OTHER' } } };
    const round = await req(bridge.base, 'POST', '/api/team/read', {
      body: { project: 'demo', query: { protocol_version: 1, op: 'review.inspect', work_id: 'WORK', review_round_id: 'round' } },
    });
    assert.equal(round.json.error.code, 'BadGateway');
    // Control: the matching response passes.
    result = base;
    const ok = await req(bridge.base, 'POST', '/api/team/read', {
      body: { project: 'demo', query: { protocol_version: 1, op: 'work.snapshot', work_id: 'WORK', workstream_id: 'stream' } },
    });
    assert.equal(ok.json.ok, true, JSON.stringify(ok.json));
  } finally {
    await bridge.close();
    await upstream.close();
  }
});

test('generic read and coordination detail share one attribution semantics', async () => {
  const mismatched = { ...ENVELOPE, workstream_id: 'stream', data: { work_id: 'OTHER', execution_authorized: false } };
  const upstream = await startMockUpstream((_request, response) => response.end(JSON.stringify(mismatched)));
  const bridge = await startLiveBridge(upstream.base);
  const query = { protocol_version: 1, op: 'work.prepare', work_id: 'WORK', workstream_id: 'stream' };
  try {
    const viaRead = await req(bridge.base, 'POST', '/api/team/read', { body: { project: 'demo', query } });
    const viaDetail = await req(bridge.base, 'POST', '/api/team/inbox-detail', { body: { project: 'demo', query } });
    assert.equal(viaRead.json.error.code, 'BadGateway');
    assert.equal(viaDetail.json.error.code, 'BadGateway');
    assert.equal(viaRead.json.result, undefined);
    assert.equal(viaDetail.json.result, undefined);
  } finally {
    await bridge.close();
    await upstream.close();
  }
});

function overviewUpstream(pages, sessionId = 'ws_obs2') {
  return startMockUpstream((request, response) => {
    const chunks = [];
    request.on('data', (c) => chunks.push(c));
    request.on('end', () => {
      response.setHeader('content-type', 'application/json');
      if (request.url === '/v1/web/session') {
        response.end(JSON.stringify({ session_id: sessionId }));
        return;
      }
      const query = JSON.parse(Buffer.concat(chunks).toString('utf8'));
      const key = query.op === 'work.list' ? `work.list:${query.workstream_id}` : query.op;
      const handler = pages[key] || pages[query.op];
      if (!handler) {
        response.writeHead(404);
        response.end(JSON.stringify({ code: 'NotFound' }));
        return;
      }
      handler(query, response);
    });
  });
}

test('overview declares authorized scope, per-part coverage and never counts a capped enumeration as all', async () => {
  const upstream = await overviewUpstream({
    'workstreams.list': (_q, r) => r.end(JSON.stringify({ ...ENVELOPE, data: { items: [{ id: 'backend' }, { id: 'frontend' }], next_cursor: null } })),
    'work.list:backend': (q, r) => r.end(JSON.stringify(q.cursor
      ? { ...ENVELOPE, data: { items: [{ work_id: 'api-2' }], total: 5, next_cursor: null } }
      : { ...ENVELOPE, data: { items: [{ work_id: 'api-1' }], total: 5, next_cursor: 'b2' } })),
    'work.list:frontend': (_q, r) => r.end(JSON.stringify({ ...ENVELOPE, data: { items: [{ work_id: 'ui-1' }], total: 1, next_cursor: null } })),
    'capabilities': (_q, r) => r.end(JSON.stringify({ identity: { actor_id: 'member' } })),
  });
  const bridge = await startLiveBridge(upstream.base);
  try {
    const overview = await req(bridge.base, 'GET', '/api/team/overview?project=demo');
    assert.equal(overview.json.ok, true, JSON.stringify(overview.json));
    const coverage = overview.json.coverage;
    assert.deepEqual(coverage.scope, { enumerated: 2, truncated: false, next_cursor: null });
    assert.equal(coverage.succeeded, 2);
    assert.equal(coverage.failed, 0);
    assert.equal(coverage.truncated, false);
    assert.equal(coverage.works_enumerated, 3);
    assert.equal(coverage.works_total, 6);
    const backend = coverage.streams.find((s) => s.workstream_id === 'backend');
    assert.equal(backend.status, 'succeeded');
    assert.equal(backend.works_enumerated, 2);
    assert.equal(backend.works_total, 5);
    assert.deepEqual(backend.as_of, { project_revision: '20', source_snapshot_id: 'snap-1', coordinator_epoch: 'epoch-1' });
    // Every part shares one revision here, so the composition may say so.
    assert.equal(overview.json.freshness.authoritative.status, 'measured');
    assert.equal(overview.json.freshness.authoritative.project_revision, '20');
    assert.equal(overview.json.freshness.report_received.status, 'not_measured');
    assert.equal(overview.json.freshness.page_read.status, 'measured');
    assert.deepEqual(overview.json.works.map((w) => w.key), ['api-1', 'api-2', 'ui-1']);
  } finally {
    await bridge.close();
    await upstream.close();
  }
});

test('a mixed-revision composition never claims a common authoritative revision', async () => {
  const upstream = await overviewUpstream({
    'workstreams.list': (_q, r) => r.end(JSON.stringify({ ...ENVELOPE, data: { items: [{ id: 'backend' }, { id: 'frontend' }], next_cursor: null } })),
    'work.list:backend': (_q, r) => r.end(JSON.stringify({ ...ENVELOPE, data: { items: [{ work_id: 'api-1' }], next_cursor: null } })),
    'work.list:frontend': (_q, r) => r.end(JSON.stringify({
      ...ENVELOPE, project_revision: '21',
      data: { items: [{ work_id: 'ui-1' }], next_cursor: null },
    })),
    'capabilities': (_q, r) => r.end(JSON.stringify({ identity: { actor_id: 'member' } })),
  });
  const bridge = await startLiveBridge(upstream.base);
  try {
    const overview = await req(bridge.base, 'GET', '/api/team/overview?project=demo');
    assert.equal(overview.json.ok, true);
    // The old page (revision 20) must not be overridden by, or merged into,
    // the newer one (21): each part keeps its own as-of and the composition
    // claims no common revision.
    assert.equal(overview.json.freshness.authoritative.status, 'not_measured');
    const parts = overview.json.coverage.streams;
    assert.equal(parts.find((s) => s.workstream_id === 'backend').as_of.project_revision, '20');
    assert.equal(parts.find((s) => s.workstream_id === 'frontend').as_of.project_revision, '21');
  } finally {
    await bridge.close();
    await upstream.close();
  }
});

test('a non-auth partial failure retains the confirmed parts with their as-of', async () => {
  const upstream = await overviewUpstream({
    'workstreams.list': (_q, r) => r.end(JSON.stringify({ ...ENVELOPE, data: { items: [{ id: 'backend' }, { id: 'frontend' }], next_cursor: null } })),
    'work.list:backend': (_q, r) => r.end(JSON.stringify({ ...ENVELOPE, data: { items: [{ work_id: 'api-1' }], total: 3, next_cursor: null } })),
    'work.list:frontend': (_q, r) => {
      r.writeHead(500);
      r.end(JSON.stringify({ code: 'DatabaseUnavailable', message: 'synthetic storage failure' }));
    },
    'capabilities': (_q, r) => r.end(JSON.stringify({ identity: { actor_id: 'member' } })),
  });
  const bridge = await startLiveBridge(upstream.base);
  try {
    const overview = await req(bridge.base, 'GET', '/api/team/overview?project=demo');
    assert.equal(overview.json.ok, true, JSON.stringify(overview.json));
    assert.deepEqual(overview.json.works.map((w) => w.key), ['api-1']);
    const coverage = overview.json.coverage;
    assert.equal(coverage.succeeded, 1);
    assert.equal(coverage.failed, 1);
    const failed = coverage.streams.find((s) => s.workstream_id === 'frontend');
    assert.equal(failed.status, 'failed');
    assert.equal(failed.error_code, 'DatabaseUnavailable');
    assert.equal(failed.works_enumerated, 0);
    const backend = coverage.streams.find((s) => s.workstream_id === 'backend');
    assert.equal(backend.status, 'succeeded');
    assert.deepEqual(backend.as_of, { project_revision: '20', source_snapshot_id: 'snap-1', coordinator_epoch: 'epoch-1' });
    // The failed part's total is unknown, so the composition has no total.
    assert.equal(coverage.works_total, null);
    assert.equal(coverage.works_enumerated, 1);
  } finally {
    await bridge.close();
    await upstream.close();
  }
});

test('an auth revocation mid-composition clears everything instead of retaining restricted content', async () => {
  const upstream = await overviewUpstream({
    'workstreams.list': (_q, r) => r.end(JSON.stringify({ ...ENVELOPE, data: { items: [{ id: 'backend' }, { id: 'frontend' }], next_cursor: null } })),
    'work.list:backend': (_q, r) => r.end(JSON.stringify({ ...ENVELOPE, data: { items: [{ work_id: 'api-1' }], total: 3, next_cursor: null } })),
    'work.list:frontend': (_q, r) => {
      r.writeHead(403);
      r.end(JSON.stringify({ code: 'Forbidden', message: 'grant revoked' }));
    },
    'capabilities': (_q, r) => r.end(JSON.stringify({ identity: { actor_id: 'member' } })),
  });
  const bridge = await startLiveBridge(upstream.base);
  try {
    const overview = await req(bridge.base, 'GET', '/api/team/overview?project=demo');
    assert.equal(overview.json.ok, false);
    assert.equal(overview.json.error.code, 'Forbidden');
    assert.equal(overview.json.works, undefined);
    assert.equal(overview.json.coverage, undefined);
  } finally {
    await bridge.close();
    await upstream.close();
  }
});

test('an enumeration that hits the page cap is declared truncated, never counted as all', async () => {
  const upstream = await overviewUpstream({
    'workstreams.list': (_q, r) => r.end(JSON.stringify({ ...ENVELOPE, data: { items: [{ id: 'backend' }, { id: 'frontend' }], next_cursor: null } })),
    'work.list:backend': (q, r) => {
      const page = q.cursor ? Number(q.cursor.slice(1)) : 0;
      r.end(JSON.stringify({ ...ENVELOPE, data: { items: [{ work_id: `api-${page}` }], total: 150, next_cursor: `c${page + 1}` } }));
    },
    'work.list:frontend': (_q, r) => r.end(JSON.stringify({ ...ENVELOPE, data: { items: [{ work_id: 'ui-1' }], total: 1, next_cursor: null } })),
    'capabilities': (_q, r) => r.end(JSON.stringify({ identity: { actor_id: 'member' } })),
  });
  const bridge = await startLiveBridge(upstream.base);
  try {
    const overview = await req(bridge.base, 'GET', '/api/team/overview?project=demo');
    assert.equal(overview.json.ok, true, JSON.stringify(overview.json));
    const backend = overview.json.coverage.streams.find((s) => s.workstream_id === 'backend');
    assert.equal(backend.truncated, true);
    assert.equal(typeof backend.next_cursor, 'string');
    assert.equal(backend.works_enumerated, 100);
    assert.equal(backend.works_total, 150);
    assert.equal(overview.json.coverage.truncated, true);
    assert.equal(overview.json.coverage.works_enumerated, 101);
    assert.equal(overview.json.coverage.works_total, 151);
  } finally {
    await bridge.close();
    await upstream.close();
  }
});

test('a truncated authorized-scope enumeration is declared instead of silently narrowing the scope', async () => {
  const upstream = await overviewUpstream({
    'workstreams.list': (q, r) => {
      const page = q.cursor ? Number(q.cursor.slice(1)) : 0;
      r.end(JSON.stringify({ ...ENVELOPE, data: { items: [{ id: `stream-${page}` }], next_cursor: `c${page + 1}` } }));
    },
    // Every enumerated stream reads as an empty list.
    'work.list': (_q, r) => r.end(JSON.stringify({ ...ENVELOPE, data: { items: [], next_cursor: null } })),
    'capabilities': (_q, r) => r.end(JSON.stringify({ identity: { actor_id: 'member' } })),
  });
  const bridge = await startLiveBridge(upstream.base);
  try {
    const overview = await req(bridge.base, 'GET', '/api/team/overview?project=demo');
    assert.equal(overview.json.ok, true, JSON.stringify(overview.json));
    assert.equal(overview.json.coverage.scope.truncated, true);
    assert.equal(typeof overview.json.coverage.scope.next_cursor, 'string');
    assert.equal(overview.json.coverage.scope.enumerated, 100);
    assert.equal(overview.json.coverage.truncated, true);
  } finally {
    await bridge.close();
    await upstream.close();
  }
});

test('a stalled scoped list read is bounded and reported as a timeout, not as empty data', async () => {
  const upstream = await overviewUpstream({
    'workstreams.list': (_q, r) => r.end(JSON.stringify({ ...ENVELOPE, data: { items: [{ id: 'backend' }], next_cursor: null } })),
    'work.list:backend': () => {}, // never answers
    'capabilities': (_q, r) => r.end(JSON.stringify({ identity: { actor_id: 'member' } })),
  });
  const bridge = await startLiveBridge(upstream.base);
  try {
    const started = Date.now();
    const overview = await req(bridge.base, 'GET', '/api/team/overview?project=demo');
    assert.ok(Date.now() - started < 9000);
    assert.equal(overview.json.ok, false);
    assert.equal(overview.json.error.code, 'ReadTimeout');
    assert.equal(overview.json.works, undefined);
  } finally {
    await bridge.close();
    await upstream.close();
  }
});

test('work detail presents report, authoritative and page-read clocks side by side', async () => {
  const runtime = { state: 'in_progress', work_version: '7', last_fence: '3', recovery_blocked: false, selected_completion_id: null };
  let progress = { phase: 'delivered', summary: 'Implementation submitted', reported_at_unix_ms: 1000,
    reported_contract_hash: 'current', recorded_project_revision: '19', client_observed_at_unix_ms: null, provenance: 'caller_declared' };
  const upstream = await startMockUpstream((_request, response) => {
    response.setHeader('content-type', 'application/json');
    response.end(JSON.stringify({ ...ENVELOPE, workstream_id: 'stream', data: {
      work_id: 'WORK', contract_hash: 'current', runtime: { ...runtime }, context_complete: true,
      visible_contract: { acceptance: ['Check the result'], required_dependencies: [] },
      snapshot: { version: 1, consistency: 'repeatable_read', project_revision: '20', source_snapshot_id: 'snap-1',
        coordinator_epoch: 'epoch-1', queried_at_unix_ms: 1234 },
      observation: { work_id: 'WORK', contract_hash: 'current', observed_at_unix_ms: 1234, runtime: { ...runtime },
        session: { id: 'session', actor_name: 'Member', client_id: 'client' },
        ...(progress ? { progress } : {}),
        guidance: { code: 'inspect_delivery', action: { note: 'Inspect the current candidate' } }, pr_deliveries: [] },
    } }));
  });
  const bridge = await startLiveBridge(upstream.base);
  try {
    const started = Date.now();
    const detail = await req(bridge.base, 'GET', '/api/team/work?project=demo&work=WORK&workstream=stream&contract=current');
    assert.equal(detail.json.ok, true, JSON.stringify(detail.json));
    const freshness = detail.json.freshness;
    assert.deepEqual(freshness.report_received, { status: 'measured', at_unix_ms: 1000, project_revision: '19' });
    assert.equal(freshness.authoritative.status, 'measured');
    assert.equal(freshness.authoritative.at_unix_ms, 1234);
    assert.equal(freshness.authoritative.project_revision, '20');
    assert.ok(freshness.page_read.at_unix_ms >= started && freshness.page_read.at_unix_ms <= Date.now());
    progress = null;
    const unmeasured = await req(bridge.base, 'GET', '/api/team/work?project=demo&work=WORK&workstream=stream&contract=current');
    assert.equal(unmeasured.json.ok, true, JSON.stringify(unmeasured.json));
    assert.deepEqual(unmeasured.json.freshness.report_received, { status: 'not_measured' });
    assert.equal(unmeasured.json.freshness.authoritative.status, 'measured');
  } finally {
    await bridge.close();
    await upstream.close();
  }
});
