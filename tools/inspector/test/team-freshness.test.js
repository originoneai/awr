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
const { createTeamBridge, readFreshness, readAttributionError } = require('../team-bridge');
const { OBSERVATION_STALE_AFTER_MS } = require('../observation-contract');

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

function startLiveBridge(teamUrl, opts = {}) {
  const bridge = createTeamBridge({ teamUrl, teamFixtureDir: FIXTURE_DIR, port: 0, ...opts });
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
        snapshot: { version: 1, project_revision: '20', source_snapshot_id: 'snap-1', coordinator_epoch: 'epoch-1', queried_at_unix_ms: Date.now() },
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
    assert.ok(freshness.authoritative.at_unix_ms >= started && freshness.authoritative.at_unix_ms <= Date.now());
    // The bridge declares the staleness bound it applied next to the measurement.
    assert.equal(freshness.authoritative.stale_after_ms, OBSERVATION_STALE_AFTER_MS);
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
    assert.equal(coverage.scope.status, 'succeeded');
    assert.equal(coverage.scope.enumerated, 2);
    assert.equal(coverage.scope.truncated, false);
    assert.equal(coverage.scope.next_cursor, null);
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
    // Every part carries the bridge's own read time next to its as-of.
    assert.ok(Number.isFinite(backend.read_at_unix_ms));
    assert.ok(Number.isFinite(coverage.scope.read_at_unix_ms));
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
    // Every enumerated stream reads as an empty list with a reported total.
    'work.list': (_q, r) => r.end(JSON.stringify({ ...ENVELOPE, data: { items: [], total: 0, next_cursor: null } })),
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
    // A truncated scope has no total even when every enumerated part reported one.
    assert.equal(overview.json.coverage.works_total, null);
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
  const observedAt = Date.now();
  let progress = { phase: 'delivered', summary: 'Implementation submitted', reported_at_unix_ms: 1000,
    reported_contract_hash: 'current', recorded_project_revision: '19', client_observed_at_unix_ms: null, provenance: 'caller_declared' };
  const upstream = await startMockUpstream((_request, response) => {
    response.setHeader('content-type', 'application/json');
    response.end(JSON.stringify({ ...ENVELOPE, workstream_id: 'stream', data: {
      work_id: 'WORK', contract_hash: 'current', runtime: { ...runtime }, context_complete: true,
      visible_contract: { acceptance: ['Check the result'], required_dependencies: [] },
      snapshot: { version: 1, consistency: 'repeatable_read', project_revision: '20', source_snapshot_id: 'snap-1',
        coordinator_epoch: 'epoch-1', queried_at_unix_ms: observedAt },
      observation: { work_id: 'WORK', contract_hash: 'current', observed_at_unix_ms: observedAt, runtime: { ...runtime },
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
    assert.equal(freshness.authoritative.at_unix_ms, observedAt);
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

// --- Plane vocabulary: measured | stale | failed | not_measured, never substituted ---

test('a report revision without a receipt time is measured but never backdated to the page clock', () => {
  const freshness = readFreshness({
    ...ENVELOPE,
    data: { work_id: 'WORK', observation: { progress: { recorded_project_revision: '19' } } },
  }, 5000);
  assert.deepEqual(freshness.report_received, { status: 'measured', project_revision: '19' });
  assert.deepEqual(freshness.page_read, { status: 'measured', at_unix_ms: 5000 });
});

test('a report receipt time without a revision is measured', () => {
  const freshness = readFreshness({
    ...ENVELOPE,
    data: { work_id: 'WORK', observation: { progress: { reported_at_unix_ms: 1000 } } },
  }, 5000);
  assert.deepEqual(freshness.report_received, { status: 'measured', at_unix_ms: 1000 });
});

test('an empty progress record is not a measurement', () => {
  const freshness = readFreshness({
    ...ENVELOPE,
    data: { work_id: 'WORK', observation: { progress: {} } },
  }, 5000);
  assert.deepEqual(freshness.report_received, { status: 'not_measured' });
});

test('a top-level progress record is measured the same as a nested one', () => {
  const freshness = readFreshness({
    ...ENVELOPE,
    data: { work_id: 'WORK', progress: { reported_at_unix_ms: 1000, recorded_project_revision: '19' } },
  }, 5000);
  assert.deepEqual(freshness.report_received, { status: 'measured', at_unix_ms: 1000, project_revision: '19' });
});

test('an authoritative measurement without a query time never borrows the page clock', () => {
  const freshness = readFreshness({ ...ENVELOPE, data: { work_id: 'WORK' } }, 5000);
  assert.deepEqual(freshness.authoritative, {
    status: 'measured',
    project_revision: '20',
    source_snapshot_id: 'snap-1',
    coordinator_epoch: 'epoch-1',
    stale_after_ms: OBSERVATION_STALE_AFTER_MS,
  });
});

test('the authoritative query time falls back to the observation time, never to the page clock', () => {
  const topLevel = readFreshness({ ...ENVELOPE, data: { work_id: 'WORK', observed_at_unix_ms: 1234 } }, 5000);
  assert.equal(topLevel.authoritative.at_unix_ms, 1234);
  assert.equal(topLevel.authoritative.age_ms, 3766);
  const nested = readFreshness({
    ...ENVELOPE,
    data: { work_id: 'WORK', observation: { observed_at_unix_ms: 1234 } },
  }, 5000);
  assert.equal(nested.authoritative.at_unix_ms, 1234);
});

test('a non-numeric envelope revision is not a measurement', () => {
  const freshness = readFreshness({ ...ENVELOPE, project_revision: 'rev-20', data: { work_id: 'WORK' } }, 5000);
  assert.deepEqual(freshness.authoritative, { status: 'not_measured' });
});

test('an incomplete envelope is not_measured, never a partial measurement', () => {
  const noEpoch = { ...ENVELOPE, data: { work_id: 'WORK' } };
  delete noEpoch.coordinator_epoch;
  assert.deepEqual(readFreshness(noEpoch, 5000).authoritative, { status: 'not_measured' });
  const noSource = { ...ENVELOPE, data: { work_id: 'WORK' } };
  delete noSource.source_snapshot_id;
  assert.deepEqual(readFreshness(noSource, 5000).authoritative, { status: 'not_measured' });
});

test('a service-declared stale report reads stale and surfaces its ordering basis and bound', () => {
  const freshness = readFreshness({
    ...ENVELOPE,
    data: {
      work_id: 'WORK',
      reporting: { stale_after_ms: 900000 },
      observation: {
        progress: {
          reported_at_unix_ms: 1000, recorded_project_revision: '19',
          stale: true, report_order_basis: 'recorded_project_revision',
        },
      },
    },
  }, 5000);
  assert.deepEqual(freshness.report_received, {
    status: 'stale',
    at_unix_ms: 1000,
    project_revision: '19',
    report_order_basis: 'recorded_project_revision',
    stale_after_ms: 900000,
    age_ms: 4000,
  });
});

test('a report older than the service-declared bound reads stale with the bound and age reported', () => {
  const freshness = readFreshness({
    ...ENVELOPE,
    data: {
      work_id: 'WORK',
      reporting: { stale_after_ms: 100 },
      observation: { progress: { reported_at_unix_ms: 1000, recorded_project_revision: '19' } },
    },
  }, 5000);
  assert.equal(freshness.report_received.status, 'stale');
  assert.equal(freshness.report_received.stale_after_ms, 100);
  assert.equal(freshness.report_received.age_ms, 4000);
});

test('an authoritative measurement older than its bound reads stale; a fresh one stays measured', () => {
  const declared = readFreshness({
    ...ENVELOPE,
    data: {
      work_id: 'WORK',
      reporting: { stale_after_ms: 100 },
      snapshot: { version: 1, queried_at_unix_ms: 1000 },
    },
  }, 5000);
  assert.equal(declared.authoritative.status, 'stale');
  assert.equal(declared.authoritative.stale_after_ms, 100);
  assert.equal(declared.authoritative.age_ms, 4000);
  // With no service declaration the bridge applies and reports its own bound.
  const fallbackStale = readFreshness({
    ...ENVELOPE,
    data: { work_id: 'WORK', snapshot: { version: 1, queried_at_unix_ms: 5000 - OBSERVATION_STALE_AFTER_MS - 1 } },
  }, 5000);
  assert.equal(fallbackStale.authoritative.status, 'stale');
  assert.equal(fallbackStale.authoritative.stale_after_ms, OBSERVATION_STALE_AFTER_MS);
  const fallbackFresh = readFreshness({
    ...ENVELOPE,
    data: { work_id: 'WORK', snapshot: { version: 1, queried_at_unix_ms: 4999 } },
  }, 5000);
  assert.equal(fallbackFresh.authoritative.status, 'measured');
  assert.equal(fallbackFresh.authoritative.age_ms, 1);
});

// --- Failed reads keep the clocks instead of dropping them ---

test('a failed read reports its planes as failed with the error code and keeps the page clock', async () => {
  const upstream = await startMockUpstream((_request, response) => {
    response.writeHead(500);
    response.end(JSON.stringify({ code: 'DatabaseUnavailable', message: 'synthetic storage failure' }));
  });
  const bridge = await startLiveBridge(upstream.base);
  try {
    const started = Date.now();
    const read = await req(bridge.base, 'POST', '/api/team/read', {
      body: { project: 'demo', query: { protocol_version: 1, op: 'work.snapshot', work_id: 'WORK', workstream_id: 'stream' } },
    });
    assert.equal(read.json.ok, false);
    assert.equal(read.json.error.code, 'DatabaseUnavailable');
    assert.deepEqual(read.json.freshness.report_received, { status: 'failed', error_code: 'DatabaseUnavailable' });
    assert.deepEqual(read.json.freshness.authoritative, { status: 'failed', error_code: 'DatabaseUnavailable' });
    assert.equal(read.json.freshness.page_read.status, 'measured');
    assert.ok(read.json.freshness.page_read.at_unix_ms >= started && read.json.freshness.page_read.at_unix_ms <= Date.now());
  } finally {
    await bridge.close();
    await upstream.close();
  }
});

test('a failed work detail read reports its planes as failed instead of dropping the clocks', async () => {
  const upstream = await startMockUpstream((_request, response) => {
    response.writeHead(500);
    response.end(JSON.stringify({ code: 'DatabaseUnavailable', message: 'synthetic storage failure' }));
  });
  const bridge = await startLiveBridge(upstream.base);
  try {
    const detail = await req(bridge.base, 'GET', '/api/team/work?project=demo&work=WORK&workstream=stream');
    assert.equal(detail.json.ok, false);
    assert.equal(detail.json.error.code, 'DatabaseUnavailable');
    assert.deepEqual(detail.json.freshness.report_received, { status: 'failed', error_code: 'DatabaseUnavailable' });
    assert.deepEqual(detail.json.freshness.authoritative, { status: 'failed', error_code: 'DatabaseUnavailable' });
    assert.equal(detail.json.freshness.page_read.status, 'measured');
  } finally {
    await bridge.close();
    await upstream.close();
  }
});

// --- Attribution: single-object reads must echo the requested identity ---

test('the echo registry refuses an omitted identity per single-object op class', () => {
  for (const op of ['work.prepare', 'work.snapshot', 'work.observe', 'delivery.inspect',
    'delivery.neutral.inspect', 'delivery.submission.describe', 'delivery.neutral.outcome',
    'delivery.source.status']) {
    const query = { protocol_version: 1, op, work_id: 'WORK' };
    assert.equal(readAttributionError(query, { ...ENVELOPE, data: {} }).code, 'BadGateway', `${op} omitted`);
    assert.equal(readAttributionError(query, { ...ENVELOPE, data: { work_id: 'WORK' } }), null, `${op} echoed`);
    assert.equal(readAttributionError(query, { ...ENVELOPE, data: { work_id: 'OTHER' } }).code, 'BadGateway', `${op} mismatched`);
  }
  // review.inspect echoes the work and the round inside the review object.
  const reviewQuery = { protocol_version: 1, op: 'review.inspect', work_id: 'WORK', review_round_id: 'round-1' };
  assert.equal(readAttributionError(reviewQuery, { ...ENVELOPE, data: { work_id: 'WORK', review: { round_id: 'round-1' } } }).code, 'BadGateway', 'review work omitted');
  assert.equal(readAttributionError(reviewQuery, { ...ENVELOPE, data: { work_id: 'WORK', review: { work_id: 'WORK' } } }).code, 'BadGateway', 'review round omitted');
  assert.equal(readAttributionError(reviewQuery, { ...ENVELOPE, data: { review: { work_id: 'WORK', round_id: 'round-1' } } }), null, 'review echoed');
  // delivery.integration.inspect echoes the request as integration_id.
  const integrationQuery = { protocol_version: 1, op: 'delivery.integration.inspect', work_id: 'WORK', request_id: 'request-1' };
  assert.equal(readAttributionError(integrationQuery, { ...ENVELOPE, data: { work_id: 'WORK' } }).code, 'BadGateway', 'integration omitted');
  assert.equal(readAttributionError(integrationQuery, { ...ENVELOPE, data: { work_id: 'WORK', integration_id: 'request-1' } }), null, 'integration echoed');
  // A list read names no object: omission is acceptable.
  assert.equal(readAttributionError(
    { protocol_version: 1, op: 'work.list', workstream_id: 'stream' },
    { ...ENVELOPE, workstream_id: 'stream', data: { items: [] } }
  ), null, 'list read');
  // work.admission and work.recovery carry no upstream object echo today and
  // stay attributable only by their request binding (registry-exempt).
  assert.equal(readAttributionError(
    { protocol_version: 1, op: 'work.recovery', work_id: 'WORK' },
    { ...ENVELOPE, data: { items: [] } }
  ), null, 'echo-exempt op');
});

test('the generic read refuses single-object responses that omit the requested identity', async () => {
  let result;
  const upstream = await startMockUpstream((_request, response) => response.end(JSON.stringify(result)));
  const bridge = await startLiveBridge(upstream.base);
  const query = { protocol_version: 1, op: 'work.snapshot', work_id: 'WORK', workstream_id: 'stream' };
  try {
    result = { ...ENVELOPE, workstream_id: 'stream', data: {} };
    const omitted = await req(bridge.base, 'POST', '/api/team/read', { body: { project: 'demo', query } });
    assert.equal(omitted.json.error.code, 'BadGateway');
    result = { ...ENVELOPE, workstream_id: 'stream', data: { work_id: 'WORK' } };
    const echoed = await req(bridge.base, 'POST', '/api/team/read', { body: { project: 'demo', query } });
    assert.equal(echoed.json.ok, true, JSON.stringify(echoed.json));
  } finally {
    await bridge.close();
    await upstream.close();
  }
});

test('the coordination detail pins its workstream even when the response omits it', async () => {
  let result;
  const upstream = await startMockUpstream((_request, response) => response.end(JSON.stringify(result)));
  const bridge = await startLiveBridge(upstream.base);
  const query = { protocol_version: 1, op: 'work.prepare', work_id: 'WORK', workstream_id: 'stream' };
  try {
    // The generic read accepts an unattributed-workstream response (nothing to
    // compare); the coordination detail always selects one and must refuse it.
    result = { ...ENVELOPE, data: { work_id: 'WORK' } };
    const viaRead = await req(bridge.base, 'POST', '/api/team/read', { body: { project: 'demo', query } });
    assert.equal(viaRead.json.ok, true, JSON.stringify(viaRead.json));
    const viaDetail = await req(bridge.base, 'POST', '/api/team/inbox-detail', { body: { project: 'demo', query } });
    assert.equal(viaDetail.json.error.code, 'BadGateway');
    result = { ...ENVELOPE, workstream_id: 'other-stream', data: { work_id: 'WORK' } };
    const mismatched = await req(bridge.base, 'POST', '/api/team/inbox-detail', { body: { project: 'demo', query } });
    assert.equal(mismatched.json.error.code, 'BadGateway');
  } finally {
    await bridge.close();
    await upstream.close();
  }
});

test('a non-numeric envelope revision is refused before attribution', async () => {
  const result = { ...ENVELOPE, project_revision: 'rev-20', workstream_id: 'stream', data: { work_id: 'WORK' } };
  const upstream = await startMockUpstream((_request, response) => response.end(JSON.stringify(result)));
  const bridge = await startLiveBridge(upstream.base);
  try {
    const read = await req(bridge.base, 'POST', '/api/team/read', {
      body: { project: 'demo', query: { protocol_version: 1, op: 'work.snapshot', work_id: 'WORK', workstream_id: 'stream' } },
    });
    assert.equal(read.json.error.code, 'BadGateway');
  } finally {
    await bridge.close();
    await upstream.close();
  }
});

// --- Auth revocation clears, for every auth failure code ---

test('an Unauthenticated reply mid-composition clears everything', async () => {
  const upstream = await overviewUpstream({
    'workstreams.list': (_q, r) => r.end(JSON.stringify({ ...ENVELOPE, data: { items: [{ id: 'backend' }, { id: 'frontend' }], next_cursor: null } })),
    'work.list:backend': (_q, r) => r.end(JSON.stringify({ ...ENVELOPE, data: { items: [{ work_id: 'api-1' }], total: 3, next_cursor: null } })),
    'work.list:frontend': (_q, r) => { r.writeHead(401); r.end(JSON.stringify({})); },
    'capabilities': (_q, r) => r.end(JSON.stringify({ identity: { actor_id: 'member' } })),
  });
  const bridge = await startLiveBridge(upstream.base);
  try {
    const overview = await req(bridge.base, 'GET', '/api/team/overview?project=demo');
    assert.equal(overview.json.ok, false);
    assert.equal(overview.json.error.code, 'Unauthenticated');
    assert.equal(overview.json.works, undefined);
    assert.equal(overview.json.coverage, undefined);
    assert.deepEqual(overview.json.freshness.authoritative, { status: 'failed', error_code: 'Unauthenticated' });
  } finally {
    await bridge.close();
    await upstream.close();
  }
});

test('a SessionExpired reply mid-composition clears everything', async () => {
  const upstream = await overviewUpstream({
    'workstreams.list': (_q, r) => r.end(JSON.stringify({ ...ENVELOPE, data: { items: [{ id: 'backend' }, { id: 'frontend' }], next_cursor: null } })),
    'work.list:backend': (_q, r) => r.end(JSON.stringify({ ...ENVELOPE, data: { items: [{ work_id: 'api-1' }], total: 3, next_cursor: null } })),
    'work.list:frontend': (_q, r) => { r.writeHead(401); r.end(JSON.stringify({ code: 'SessionExpired' })); },
    'capabilities': (_q, r) => r.end(JSON.stringify({ identity: { actor_id: 'member' } })),
  });
  const bridge = await startLiveBridge(upstream.base);
  try {
    const overview = await req(bridge.base, 'GET', '/api/team/overview?project=demo');
    assert.equal(overview.json.ok, false);
    assert.equal(overview.json.error.code, 'SessionExpired');
    assert.equal(overview.json.works, undefined);
    assert.equal(overview.json.coverage, undefined);
  } finally {
    await bridge.close();
    await upstream.close();
  }
});

// --- Paging as-of and totals ---

test('pages of one stream that mix revisions claim no as-of but keep their works', async () => {
  const upstream = await overviewUpstream({
    'workstreams.list': (_q, r) => r.end(JSON.stringify({ ...ENVELOPE, data: { items: [{ id: 'backend' }], next_cursor: null } })),
    'work.list:backend': (q, r) => r.end(JSON.stringify(q.cursor
      ? { ...ENVELOPE, project_revision: '21', data: { items: [{ work_id: 'api-2' }], total: 2, next_cursor: null } }
      : { ...ENVELOPE, data: { items: [{ work_id: 'api-1' }], total: 2, next_cursor: 'b2' } })),
    'capabilities': (_q, r) => r.end(JSON.stringify({ identity: { actor_id: 'member' } })),
  });
  const bridge = await startLiveBridge(upstream.base);
  try {
    const overview = await req(bridge.base, 'GET', '/api/team/overview?project=demo');
    assert.equal(overview.json.ok, true, JSON.stringify(overview.json));
    const backend = overview.json.coverage.streams.find((s) => s.workstream_id === 'backend');
    assert.equal(backend.status, 'succeeded');
    assert.equal(backend.works_enumerated, 2);
    assert.equal(backend.as_of, null);
    // The composition cannot claim one revision from mixed pages.
    assert.equal(overview.json.freshness.authoritative.status, 'not_measured');
    assert.deepEqual(overview.json.works.map((w) => w.key), ['api-1', 'api-2']);
  } finally {
    await bridge.close();
    await upstream.close();
  }
});

test('a page with an incomplete envelope triple claims no as-of', async () => {
  const upstream = await overviewUpstream({
    'workstreams.list': (_q, r) => r.end(JSON.stringify({ ...ENVELOPE, data: { items: [{ id: 'backend' }], next_cursor: null } })),
    'work.list:backend': (q, r) => {
      if (!q.cursor) {
        r.end(JSON.stringify({ ...ENVELOPE, data: { items: [{ work_id: 'api-1' }], next_cursor: 'b2' } }));
        return;
      }
      const incomplete = { ...ENVELOPE, data: { items: [{ work_id: 'api-2' }], next_cursor: null } };
      delete incomplete.coordinator_epoch;
      r.end(JSON.stringify(incomplete));
    },
    'capabilities': (_q, r) => r.end(JSON.stringify({ identity: { actor_id: 'member' } })),
  });
  const bridge = await startLiveBridge(upstream.base);
  try {
    const overview = await req(bridge.base, 'GET', '/api/team/overview?project=demo');
    assert.equal(overview.json.ok, true, JSON.stringify(overview.json));
    const backend = overview.json.coverage.streams.find((s) => s.workstream_id === 'backend');
    assert.equal(backend.status, 'succeeded');
    assert.equal(backend.as_of, null);
  } finally {
    await bridge.close();
    await upstream.close();
  }
});

test('a truncated stream with mixed page revisions declares truncation and no as-of', async () => {
  const upstream = await overviewUpstream({
    'workstreams.list': (_q, r) => r.end(JSON.stringify({ ...ENVELOPE, data: { items: [{ id: 'backend' }], next_cursor: null } })),
    'work.list:backend': (q, r) => {
      const page = q.cursor ? Number(q.cursor.slice(1)) : 0;
      r.end(JSON.stringify({
        ...ENVELOPE, project_revision: page % 2 ? '21' : '20',
        data: { items: [{ work_id: `api-${page}` }], total: 150, next_cursor: `c${page + 1}` },
      }));
    },
    'capabilities': (_q, r) => r.end(JSON.stringify({ identity: { actor_id: 'member' } })),
  });
  const bridge = await startLiveBridge(upstream.base);
  try {
    const overview = await req(bridge.base, 'GET', '/api/team/overview?project=demo');
    assert.equal(overview.json.ok, true, JSON.stringify(overview.json));
    const backend = overview.json.coverage.streams.find((s) => s.workstream_id === 'backend');
    assert.equal(backend.truncated, true);
    assert.equal(backend.works_enumerated, 100);
    assert.equal(backend.as_of, null);
  } finally {
    await bridge.close();
    await upstream.close();
  }
});

test('a negative page total is not a total', async () => {
  const upstream = await overviewUpstream({
    'workstreams.list': (_q, r) => r.end(JSON.stringify({ ...ENVELOPE, data: { items: [{ id: 'backend' }, { id: 'frontend' }], next_cursor: null } })),
    'work.list:backend': (_q, r) => r.end(JSON.stringify({ ...ENVELOPE, data: { items: [{ work_id: 'api-1' }], total: -3, next_cursor: null } })),
    'work.list:frontend': (_q, r) => r.end(JSON.stringify({ ...ENVELOPE, data: { items: [{ work_id: 'ui-1' }], next_cursor: null } })),
    'capabilities': (_q, r) => r.end(JSON.stringify({ identity: { actor_id: 'member' } })),
  });
  const bridge = await startLiveBridge(upstream.base);
  try {
    const overview = await req(bridge.base, 'GET', '/api/team/overview?project=demo');
    assert.equal(overview.json.ok, true, JSON.stringify(overview.json));
    const backend = overview.json.coverage.streams.find((s) => s.workstream_id === 'backend');
    assert.equal(backend.works_total, null);
    // One part without a total (and one with an invalid one) means no composition total.
    assert.equal(overview.json.coverage.works_total, null);
  } finally {
    await bridge.close();
    await upstream.close();
  }
});

test('a composition with a part that reported no total has no total', async () => {
  const upstream = await overviewUpstream({
    'workstreams.list': (_q, r) => r.end(JSON.stringify({ ...ENVELOPE, data: { items: [{ id: 'backend' }, { id: 'frontend' }], next_cursor: null } })),
    'work.list:backend': (_q, r) => r.end(JSON.stringify({ ...ENVELOPE, data: { items: [{ work_id: 'api-1' }], total: 5, next_cursor: null } })),
    'work.list:frontend': (_q, r) => r.end(JSON.stringify({ ...ENVELOPE, data: { items: [{ work_id: 'ui-1' }], next_cursor: null } })),
    'capabilities': (_q, r) => r.end(JSON.stringify({ identity: { actor_id: 'member' } })),
  });
  const bridge = await startLiveBridge(upstream.base);
  try {
    const overview = await req(bridge.base, 'GET', '/api/team/overview?project=demo');
    assert.equal(overview.json.ok, true, JSON.stringify(overview.json));
    assert.equal(overview.json.coverage.works_total, null);
    assert.equal(overview.json.coverage.works_enumerated, 2);
  } finally {
    await bridge.close();
    await upstream.close();
  }
});

// --- Partial failure keeps the confirmed pages (AWR-OBS-002 acceptance 3) ---

test('a mid-stream page failure retains the confirmed pages with their as-of, resume cursor and read time', async () => {
  const upstream = await overviewUpstream({
    'workstreams.list': (_q, r) => r.end(JSON.stringify({ ...ENVELOPE, data: { items: [{ id: 'backend' }, { id: 'frontend' }], next_cursor: null } })),
    'work.list:backend': (q, r) => {
      if (!q.cursor) {
        r.end(JSON.stringify({ ...ENVELOPE, data: { items: [{ work_id: 'api-1' }], total: 3, next_cursor: 'b2' } }));
        return;
      }
      r.writeHead(500);
      r.end(JSON.stringify({ code: 'DatabaseUnavailable', message: 'synthetic storage failure' }));
    },
    'work.list:frontend': (_q, r) => r.end(JSON.stringify({ ...ENVELOPE, data: { items: [{ work_id: 'ui-1' }], total: 1, next_cursor: null } })),
    'capabilities': (_q, r) => r.end(JSON.stringify({ identity: { actor_id: 'member' } })),
  });
  const bridge = await startLiveBridge(upstream.base);
  try {
    const overview = await req(bridge.base, 'GET', '/api/team/overview?project=demo');
    assert.equal(overview.json.ok, true, JSON.stringify(overview.json));
    // The confirmed first page is retained, not dropped with the failed part.
    assert.deepEqual(overview.json.works.map((w) => w.key), ['api-1', 'ui-1']);
    const backend = overview.json.coverage.streams.find((s) => s.workstream_id === 'backend');
    assert.equal(backend.status, 'failed');
    assert.equal(backend.error_code, 'DatabaseUnavailable');
    assert.equal(backend.works_enumerated, 1);
    assert.equal(backend.works_total, 3);
    assert.deepEqual(backend.as_of, { project_revision: '20', source_snapshot_id: 'snap-1', coordinator_epoch: 'epoch-1' });
    assert.equal(backend.next_cursor, 'b2');
    assert.ok(Number.isFinite(backend.read_at_unix_ms));
    assert.equal(overview.json.coverage.failed, 1);
    assert.equal(overview.json.coverage.works_total, null);
    assert.equal(overview.json.coverage.works_enumerated, 2);
  } finally {
    await bridge.close();
    await upstream.close();
  }
});

test('an auth failure on a later page clears everything instead of retaining the confirmed pages', async () => {
  const upstream = await overviewUpstream({
    'workstreams.list': (_q, r) => r.end(JSON.stringify({ ...ENVELOPE, data: { items: [{ id: 'backend' }], next_cursor: null } })),
    'work.list:backend': (q, r) => {
      if (!q.cursor) {
        r.end(JSON.stringify({ ...ENVELOPE, data: { items: [{ work_id: 'api-1' }], next_cursor: 'b2' } }));
        return;
      }
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

// --- One deadline bounds the whole composition ---

test('two stalled streams are bounded by one composition deadline, not by two page timeouts', async () => {
  const upstream = await overviewUpstream({
    'workstreams.list': (_q, r) => r.end(JSON.stringify({ ...ENVELOPE, data: { items: [{ id: 'stalled-a' }, { id: 'stalled-b' }], next_cursor: null } })),
    'work.list:stalled-a': () => {}, // never answers
    'work.list:stalled-b': () => {}, // never answers
    'capabilities': (_q, r) => r.end(JSON.stringify({ identity: { actor_id: 'member' } })),
  });
  const bridge = await startLiveBridge(upstream.base, { readTimeoutMs: 800, overviewDeadlineMs: 1000 });
  try {
    const started = Date.now();
    const overview = await req(bridge.base, 'GET', '/api/team/overview?project=demo');
    const elapsed = Date.now() - started;
    assert.ok(elapsed < 1600, `elapsed ${elapsed}ms must stay under two page timeouts`);
    assert.equal(overview.json.ok, false);
    assert.equal(overview.json.error.code, 'ReadTimeout');
    assert.equal(overview.json.works, undefined);
    assert.deepEqual(overview.json.freshness.authoritative, { status: 'failed', error_code: 'ReadTimeout' });
  } finally {
    await bridge.close();
    await upstream.close();
  }
});

test('a stalled stream is declared failed while healthy streams are still read within the deadline', async () => {
  const upstream = await overviewUpstream({
    'workstreams.list': (_q, r) => r.end(JSON.stringify({ ...ENVELOPE, data: { items: [{ id: 'stalled' }, { id: 'healthy' }], next_cursor: null } })),
    'work.list:stalled': () => {}, // never answers
    'work.list:healthy': (_q, r) => r.end(JSON.stringify({ ...ENVELOPE, data: { items: [{ work_id: 'ui-1' }], total: 1, next_cursor: null } })),
    'capabilities': (_q, r) => r.end(JSON.stringify({ identity: { actor_id: 'member' } })),
  });
  const bridge = await startLiveBridge(upstream.base, { readTimeoutMs: 800, overviewDeadlineMs: 5000 });
  try {
    const overview = await req(bridge.base, 'GET', '/api/team/overview?project=demo');
    assert.equal(overview.json.ok, true, JSON.stringify(overview.json));
    const stalled = overview.json.coverage.streams.find((s) => s.workstream_id === 'stalled');
    assert.equal(stalled.status, 'failed');
    assert.equal(stalled.error_code, 'ReadTimeout');
    assert.equal(stalled.works_enumerated, 0);
    assert.ok(Number.isFinite(stalled.read_at_unix_ms));
    const healthy = overview.json.coverage.streams.find((s) => s.workstream_id === 'healthy');
    assert.equal(healthy.status, 'succeeded');
    assert.deepEqual(overview.json.works.map((w) => w.key), ['ui-1']);
  } finally {
    await bridge.close();
    await upstream.close();
  }
});
