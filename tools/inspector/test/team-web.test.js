'use strict';

const { test } = require('node:test');
const assert = require('node:assert/strict');
const http = require('http');
const fs = require('fs');
const path = require('path');
const { createTeamBridge } = require('../team-bridge');

const FIXTURE_DIR = path.resolve(__dirname, '../../../tests/fixtures/workstreams/team-web-loop');
const GUARD = { 'x-awr-inspector': '1', origin: 'http://127.0.0.1' };

function startBridge() {
  const bridge = createTeamBridge({ teamFixtureDir: FIXTURE_DIR, port: 0 });
  const server = http.createServer(async (req, res) => {
    const url = new URL(req.url, 'http://127.0.0.1');
    const key = `${req.method} ${url.pathname}`;
    const handler = bridge.routes[key];
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
    try {
      const json = await handler(url, body, req, res);
      const cookie = res.getHeader('set-cookie');
      const headers = { 'content-type': 'application/json' };
      if (cookie) headers['set-cookie'] = cookie;
      res.writeHead(200, headers);
      res.end(JSON.stringify(json));
    } catch (err) {
      res.writeHead(500, { 'content-type': 'application/json' });
      res.end(JSON.stringify({ ok: false, error: { message: String(err.message || err) } }));
    }
  });
  return new Promise((resolve) => {
    server.listen(0, '127.0.0.1', () => {
      const { port } = server.address();
      resolve({
        base: `http://127.0.0.1:${port}`,
        close: () => new Promise((r) => server.close(r)),
        bridge,
      });
    });
  });
}

async function req(base, method, urlPath, { body, headers, cookie } = {}) {
  const res = await fetch(base + urlPath, {
    method,
    headers: {
      ...GUARD,
      ...(headers || {}),
      ...(cookie ? { cookie } : {}),
      ...(body ? { 'content-type': 'application/json' } : {}),
    },
    body: body ? JSON.stringify(body) : undefined,
  });
  const setCookie = typeof res.headers.getSetCookie === 'function' ? res.headers.getSetCookie() : [];
  const json = await res.json();
  return { status: res.status, json, setCookie };
}

test('fixtures exist for personal and team views', () => {
  for (const name of ['personal-view.json', 'team-view.json', 'acceptance-cases.json']) {
    assert.ok(fs.existsSync(path.join(FIXTURE_DIR, name)));
  }
});

test('team overview covers owner agent outcome blocker next and card fields', async () => {
  const b = await startBridge();
  try {
    const { json } = await req(b.base, 'GET', '/api/team/overview?view=team&project=demo');
    assert.equal(json.ok, true);
    assert.ok(json.works.length >= 2);
    const blocked = json.works.find((w) => w.key === 'TW-202');
    assert.ok(blocked.blocker.prerequisite_outcome);
    assert.ok(blocked.blocker.release_condition);
    assert.ok(blocked.blocker.check_basis);
    assert.ok(blocked.depends_on.some((d) => d.visible));
    assert.equal(typeof blocked.blocker.backend_code, 'string');
    const hidden = json.works.find((w) => (w.hidden_deps || []).length);
    assert.ok(hidden.hidden_deps[0].hint);
    assert.equal(hidden.hidden_deps[0].leaks, false);
  } finally {
    await b.close();
  }
});

test('login sets cookie, logout and revoke clear it; bearer not echoed', async () => {
  const b = await startBridge();
  try {
    const login = await req(b.base, 'POST', '/api/team/login', {
      body: { bearer: 'awr1.test.0123456789abcdef' },
    });
    assert.equal(login.json.ok, true);
    assert.equal(login.json.auth.bearer_in_page, false);
    assert.ok(!JSON.stringify(login.json).includes('awr1.test.0123456789abcdef'));
    assert.ok(login.setCookie.some((c) => c.startsWith('awr_web_session=') && c.includes('HttpOnly')));
    const cookie = login.setCookie[0].split(';')[0];
    const logout = await req(b.base, 'POST', '/api/team/logout', { cookie, body: {} });
    assert.equal(logout.json.logged_out, true);
  } finally {
    await b.close();
  }
});

test('actions issue idempotent receipts and map to shared server ops', async () => {
  const b = await startBridge();
  try {
    const body = {
      project: 'demo',
      work_key: 'TW-201',
      action: 'submit_review',
      request_id: 'req-1',
    };
    const first = await req(b.base, 'POST', '/api/team/action', { body });
    assert.equal(first.json.ok, true);
    assert.equal(first.json.replayed, false);
    assert.equal(first.json.server_op, 'delivery.submit_and_request_review');
    const second = await req(b.base, 'POST', '/api/team/action', { body });
    assert.equal(second.json.replayed, true);
    assert.equal(second.json.receipt.id, first.json.receipt.id);
  } finally {
    await b.close();
  }
});

test('expired operations are rejected', async () => {
  const b = await startBridge();
  try {
    const { json } = await req(b.base, 'POST', '/api/team/action', {
      body: {
        project: 'demo',
        work_key: 'TW-201',
        action: 'accept',
        request_id: 'req-expired',
        expired: true,
      },
    });
    assert.equal(json.ok, false);
    assert.equal(json.error.code, 'ExpiredOperation');
  } finally {
    await b.close();
  }
});

test('team-web module double-submit guard', async () => {
  const teamWeb = require('../public/team-web.js');
  const calls = [];
  const api = teamWeb.createTeamWeb({
    i18n: { t: (k) => k },
    $: () => null,
    callApi: async () => ({ ok: true }),
  });
  let resolveGate;
  const gate = new Promise((r) => { resolveGate = r; });
  let started = 0;
  const run = api._guardDouble('act', async () => {
    started += 1;
    calls.push('start');
    await gate;
    calls.push('end');
  });
  const p1 = run();
  const p2 = run(); // should no-op while inflight
  resolveGate();
  await Promise.all([p1, p2]);
  assert.equal(started, 1);
});

test('i18n keys for team web exist in en and zh-CN', () => {
  const enText = fs.readFileSync(path.join(__dirname, '../public/locales/en.js'), 'utf8');
  const zhText = fs.readFileSync(path.join(__dirname, '../public/locales/zh-CN.js'), 'utf8');
  for (const key of [
    'ui.team_web',
    'ui.my_projects',
    'ui.blocker_detail',
    'ui.accept_responsibility',
    'ui.hidden_dep_hint_p0',
  ]) {
    assert.ok(enText.includes('"' + key + '"'), key + ' en');
    assert.ok(zhText.includes('"' + key + '"'), key + ' zh');
  }
});

test('rewriteOwnedCookiePath maps upstream Path=/v1/web to /api/team', () => {
  const { rewriteOwnedCookiePath } = require('../team-bridge');
  const set = rewriteOwnedCookiePath(
    'awr_web_session=ws_abc; HttpOnly; Path=/v1/web; SameSite=Strict; Max-Age=28800'
  );
  assert.ok(set.includes('Path=/api/team'));
  assert.ok(!set.includes('Path=/v1/web'));
  const clear = rewriteOwnedCookiePath(
    'awr_web_session=; HttpOnly; Path=/v1/web; SameSite=Strict; Max-Age=0'
  );
  assert.ok(clear.includes('Path=/api/team'));
  assert.ok(clear.includes('Max-Age=0'));
});

function startMockUpstream(handler) {
  const server = http.createServer((req, res) => handler(req, res));
  return new Promise((resolve) => {
    server.listen(0, '127.0.0.1', () => {
      const { port } = server.address();
      resolve({
        base: `http://127.0.0.1:${port}`,
        close: () => new Promise((r) => server.close(r)),
        port,
      });
    });
  });
}

function startLiveBridge(teamUrl, options = {}) {
  const bridge = createTeamBridge({ teamUrl, teamFixtureDir: FIXTURE_DIR, port: 0, ...options });
  const server = http.createServer(async (req, res) => {
    const url = new URL(req.url, 'http://127.0.0.1');
    const key = `${req.method} ${url.pathname}`;
    const handler = bridge.routes[key];
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
    try {
      const json = await handler(url, body, req, res);
      const cookie = res.getHeader('set-cookie');
      const headers = { 'content-type': 'application/json' };
      if (cookie) headers['set-cookie'] = cookie;
      res.writeHead(200, headers);
      res.end(JSON.stringify(json));
    } catch (err) {
      res.writeHead(500, { 'content-type': 'application/json' });
      res.end(JSON.stringify({ ok: false, error: { message: String(err.message || err) } }));
    }
  });
  return new Promise((resolve) => {
    server.listen(0, '127.0.0.1', () => {
      const { port } = server.address();
      resolve({
        base: `http://127.0.0.1:${port}`,
        close: () => new Promise((r) => server.close(r)),
        bridge,
      });
    });
  });
}

test('live mode never invents demo receipts; proxies command store', async () => {
  const calls = [];
  const upstream = await startMockUpstream((req, res) => {
    const chunks = [];
    req.on('data', (c) => chunks.push(c));
    req.on('end', () => {
      const raw = Buffer.concat(chunks).toString('utf8');
      calls.push({ method: req.method, url: req.url, cookie: req.headers.cookie || null, body: raw });
      if (req.url === '/v1/web/login' && req.method === 'POST') {
        res.writeHead(200, {
          'content-type': 'application/json',
          'set-cookie':
            'awr_web_session=ws_live1; HttpOnly; Path=/v1/web; SameSite=Strict; Max-Age=28800',
        });
        res.end(JSON.stringify({ ok: true, session_id: 'ws_live1', projects: ['demo'] }));
        return;
      }
      if (req.url === '/v1/web/session' && req.method === 'GET') {
        if (!req.headers.cookie || !req.headers.cookie.includes('awr_web_session=ws_live1')) {
          res.writeHead(401, { 'content-type': 'application/json' });
          res.end(JSON.stringify({ code: 'Unauthenticated', message: 'no web session' }));
          return;
        }
        res.writeHead(200, { 'content-type': 'application/json' });
        res.end(JSON.stringify({ ok: true, session_id: 'ws_live1', expires_at_ms: Date.now() + 60000 }));
        return;
      }
      if (req.url === '/v1/web/projects/demo/query' && req.method === 'POST') {
        res.writeHead(200, { 'content-type': 'application/json' });
        const query = JSON.parse(raw);
        res.end(JSON.stringify(query.op === 'workstreams.list'
          ? { items: [{ id: 'stream-live' }], next_cursor: null }
          : { data: { items: [{ external_key: 'TW-LIVE', status: 'open' }], next_cursor: null } }));
        return;
      }
      if (req.url === '/v1/web/projects/demo/command' && req.method === 'POST') {
        const body = JSON.parse(raw || '{}');
        if (body.request_id === 'req-replay') {
          res.writeHead(200, { 'content-type': 'application/json' });
          res.end(
            JSON.stringify({
              replayed: true,
              receipt: { id: 'rcpt_store', request_id: 'req-replay', op: 'review.accept' },
            })
          );
          return;
        }
        res.writeHead(200, { 'content-type': 'application/json' });
        res.end(
          JSON.stringify({
            replayed: false,
            receipt: { id: 'rcpt_store', request_id: body.request_id, op: body.op },
          })
        );
        return;
      }
      res.writeHead(404, { 'content-type': 'application/json' });
      res.end(JSON.stringify({ code: 'NotFound' }));
    });
  });

  const b = await startLiveBridge(upstream.base);
  try {
    const login = await req(b.base, 'POST', '/api/team/login', {
      body: { bearer: 'awr1.test.0123456789abcdef' },
    });
    assert.equal(login.json.ok, true);
    assert.ok(login.setCookie.some((c) => c.includes('Path=/api/team')));
    assert.ok(!login.setCookie.some((c) => c.includes('Path=/v1/web')));
    const cookie = login.setCookie[0].split(';')[0];

    const overviewNoCookie = await req(b.base, 'GET', '/api/team/overview?project=demo');
    assert.equal(overviewNoCookie.json.ok, false);
    assert.equal(overviewNoCookie.json.error.code, 'Unauthenticated');

    const overview = await req(b.base, 'GET', '/api/team/overview?project=demo', { cookie });
    assert.equal(overview.json.ok, true);
    assert.equal(overview.json.works[0].key, 'TW-LIVE');
    assert.equal(overview.json.schema, 'awr-team-web-loop-live/v1');

    const denyAction = await req(b.base, 'POST', '/api/team/action', {
      body: { project: 'demo', action: 'accept', request_id: 'x', work_key: 'TW-LIVE' },
    });
    assert.equal(denyAction.json.ok, false);
    assert.equal(denyAction.json.error.code, 'Unauthenticated');

    const accept = await req(b.base, 'POST', '/api/team/action', {
      cookie,
      body: {
        project: 'demo',
        command: {
          protocol_version: 1,
          request_id: 'req-1',
          op: 'review.accept',
          workstream_id: '1',
          work_id: 'TW-LIVE',
          coordinator_epoch: 'e',
          expected_project_revision: '1',
          expected_authority_version: '1',
          expected_ownership_version: '1',
          expected_contract_hash: 'a'.repeat(64),
          args: {},
        },
      },
    });
    assert.equal(accept.json.ok, true);
    assert.equal(accept.json.replayed, false);
    assert.equal(accept.json.receipt.op, 'review.accept');
    assert.equal(accept.json.receipt.id, 'rcpt_store');
    assert.ok(!String(accept.json.receipt.id).startsWith('rcpt_req'));

    const replay = await req(b.base, 'POST', '/api/team/action', {
      cookie,
      body: {
        project: 'demo',
        command: {
          protocol_version: 1,
          request_id: 'req-replay',
          op: 'review.accept',
          workstream_id: '1',
          work_id: 'TW-LIVE',
          coordinator_epoch: 'e',
          expected_project_revision: '1',
          expected_authority_version: '1',
          expected_ownership_version: '1',
          expected_contract_hash: 'a'.repeat(64),
          args: {},
        },
      },
    });
    assert.equal(replay.json.ok, true);
    assert.equal(replay.json.replayed, true);
    assert.equal(replay.json.receipt.id, 'rcpt_store');

    assert.ok(calls.some((c) => c.url === '/v1/web/login'));
    assert.ok(calls.some((c) => c.url === '/v1/web/projects/demo/query'));
    assert.ok(calls.some((c) => c.url === '/v1/web/projects/demo/command'));
    // Overview + action must not invent fixture receipts without upstream.
    assert.ok(!calls.every((c) => c.url === '/v1/web/login'));
  } finally {
    await b.close();
    await upstream.close();
  }
});

test('live overview discovers authorized streams and follows scoped pagination', async () => {
  const queries = [];
  const upstream = await startMockUpstream((request, response) => {
    const chunks = [];
    request.on('data', (chunk) => chunks.push(chunk));
    request.on('end', () => {
      response.setHeader('content-type', 'application/json');
      if (request.url === '/v1/web/session') {
        response.end(JSON.stringify({ session_id: 'ws_multi' }));
        return;
      }
      const query = JSON.parse(Buffer.concat(chunks).toString());
      queries.push(query);
      assert.equal(request.headers.cookie, 'awr_web_session=ws_multi');
      if (query.op === 'capabilities') {
        response.end(JSON.stringify({ identity: { actor_id: 'member', can_manage_members: false } }));
      } else if (query.op === 'workstreams.list') {
        response.end(JSON.stringify(query.cursor
          ? { items: [{ id: 'frontend' }], next_cursor: null }
          : { items: [{ id: 'backend' }], next_cursor: 'streams-2' }));
      } else if (query.workstream_id === 'backend') {
        response.end(JSON.stringify({ data: query.cursor
          ? { items: [{ work_id: 'api-2' }], next_cursor: null }
          : { items: [{ work_id: 'api-1' }], next_cursor: 'backend-2' } }));
      } else if (query.workstream_id === 'frontend') {
        response.end(JSON.stringify({ data: { items: [{ work_id: 'ui-1' }], next_cursor: null } }));
      } else {
        response.writeHead(403);
        response.end(JSON.stringify({ code: 'Forbidden' }));
      }
    });
  });
  const bridge = await startLiveBridge(upstream.base);
  try {
    const overview = await req(bridge.base, 'GET', '/api/team/overview?project=demo', {
      cookie: 'awr_web_session=ws_multi',
    });
    assert.equal(overview.json.ok, true);
    assert.equal(overview.json.identity.actor_id, 'member');
    assert.deepEqual(overview.json.works.map((work) => [work.key, work.workstream_id]), [
      ['api-1', 'backend'], ['api-2', 'backend'], ['ui-1', 'frontend'],
    ]);
    assert.deepEqual(overview.json.workstreams, [{ id: 'backend' }, { id: 'frontend' }]);
    assert.deepEqual(queries.map(({ op, workstream_id, cursor }) => [op, workstream_id, cursor]), [
      ['workstreams.list', undefined, undefined],
      ['workstreams.list', undefined, 'streams-2'],
      ['work.list', 'backend', undefined],
      ['work.list', 'backend', 'backend-2'],
      ['work.list', 'frontend', undefined],
      ['capabilities', undefined, undefined],
    ]);
  } finally {
    await bridge.close();
    await upstream.close();
  }
});

for (const scenario of ['empty', 'revoked', 'invalid_page', 'repeated_cursor']) {
  test(`live overview handles ${scenario} without claiming a partial result`, async () => {
    const upstream = await startMockUpstream((request, response) => {
      const chunks = [];
      request.on('data', (chunk) => chunks.push(chunk));
      request.on('end', () => {
        response.setHeader('content-type', 'application/json');
        if (request.url === '/v1/web/session') {
          response.end(JSON.stringify({ session_id: 'ws_test' }));
          return;
        }
        const query = JSON.parse(Buffer.concat(chunks).toString());
        if (query.op === 'workstreams.list') {
          response.end(JSON.stringify({ items: scenario === 'empty' ? [] : [{ id: 'backend' }], next_cursor: null }));
        } else if (scenario === 'revoked') {
          response.writeHead(403);
          response.end(JSON.stringify({ code: 'Forbidden', message: 'grant revoked' }));
        } else if (scenario === 'invalid_page') {
          response.end(JSON.stringify({ data: { items: null } }));
        } else {
          response.end(JSON.stringify({ data: { items: [{ work_id: 'api' }], next_cursor: 'same' } }));
        }
      });
    });
    const bridge = await startLiveBridge(upstream.base);
    try {
      const overview = await req(bridge.base, 'GET', '/api/team/overview?project=demo');
      if (scenario === 'empty') {
        assert.equal(overview.json.ok, true);
        assert.deepEqual(overview.json.works, []);
      } else {
        assert.equal(overview.json.ok, false);
        assert.equal(overview.json.error.code, scenario === 'revoked' ? 'Forbidden' : 'BadGateway');
        assert.equal(overview.json.works, undefined);
      }
    } finally {
      await bridge.close();
      await upstream.close();
    }
  });
}

test('live mode expired session denies overview and action', async () => {
  const upstream = await startMockUpstream((req, res) => {
    if (req.url === '/v1/web/session') {
      res.writeHead(401, { 'content-type': 'application/json' });
      res.end(JSON.stringify({ code: 'SessionExpired', message: 'web session expired or revoked' }));
      return;
    }
    res.writeHead(500, { 'content-type': 'application/json' });
    res.end(JSON.stringify({ code: 'Unexpected' }));
  });
  const b = await startLiveBridge(upstream.base);
  try {
    const overview = await req(b.base, 'GET', '/api/team/overview?project=demo', {
      cookie: 'awr_web_session=ws_expired',
    });
    assert.equal(overview.json.ok, false);
    assert.equal(overview.json.error.code, 'SessionExpired');
    const action = await req(b.base, 'POST', '/api/team/action', {
      cookie: 'awr_web_session=ws_expired',
      body: {
        project: 'demo',
        command: { protocol_version: 1, request_id: 'r', op: 'review.accept' },
      },
    });
    assert.equal(action.json.ok, false);
    assert.equal(action.json.error.code, 'SessionExpired');
  } finally {
    await b.close();
    await upstream.close();
  }
});

test('live project discovery restores the existing cookie session even with no projects', async () => {
  const upstream = await startMockUpstream((request, response) => {
    response.setHeader('content-type', 'application/json');
    response.end(JSON.stringify(request.url === '/v1/web/session'
      ? { ok: true, session_id: 'ws_existing', expires_at_ms: 1000 }
      : { ok: true, projects: [] }));
  });
  const bridge = await startLiveBridge(upstream.base);
  try {
    const result = await req(bridge.base, 'GET', '/api/team/projects');
    assert.equal(result.json.session.session_id, 'ws_existing');
    assert.deepEqual(result.json.projects, []);
  } finally {
    await bridge.close();
    await upstream.close();
  }
});

for (const scenario of ['detail', 'detail-no-guidance', 'changed', 'denied', 'observe-denied', 'observe-changed']) {
  test(`live work detail preserves scope and handles ${scenario}`, async () => {
    const queries = [];
    const upstream = await startMockUpstream((request, response) => {
      const chunks = [];
      request.on('data', (chunk) => chunks.push(chunk));
      request.on('end', () => {
        const query = JSON.parse(Buffer.concat(chunks).toString());
        queries.push(query);
        response.setHeader('content-type', 'application/json');
        if (scenario === 'denied' || (scenario === 'observe-denied' && query.op === 'work.observe')) {
          response.writeHead(403);
          response.end(JSON.stringify({ code: 'Forbidden', message: 'not authorized' }));
          return;
        }
        if (query.op === 'work.snapshot') {
          response.writeHead(400);
          response.end(JSON.stringify({ code: 'Unsupported', message: 'Legacy query catalog' }));
          return;
        }
        if (query.op === 'work.observe') {
          response.end(JSON.stringify({ workstream_id: 'stream', data: {
            work_id: 'WORK', contract_hash: scenario === 'observe-changed' ? 'new' : 'current', observed_at_unix_ms: 1234,
            session: { id: 'session', actor_name: 'Developer', client_id: 'agent-client' },
            checkpoint: { id: 'checkpoint', contract_matches_current: true, next_action: 'Review the change', open_loops: [] },
            guidance: { code: 'inspect_delivery', action: { note: 'Inspect delivery' } },
            runtime: null, pr_deliveries: [],
          } }));
          return;
        }
        response.end(JSON.stringify({ workstream_id: 'stream', data: {
          work_id: 'WORK', contract_hash: 'current', runtime: null,
          visible_contract: { acceptance: ['Contract criterion'], required_dependencies: ['VISIBLE'] },
          dependency_export_unavailable: true, context_complete: false,
          guidance: scenario === 'detail-no-guidance' ? undefined : { code: 'restore_context', action: { note: 'Restore missing context' } },
          next_step: 'Restore missing context',
          completeness_reasons: ['dependency_export_unavailable'], execution_admission: 'not_evaluated',
        } }));
      });
    });
    const bridge = await startLiveBridge(upstream.base);
    try {
      const result = await req(bridge.base, 'GET', '/api/team/work?project=demo&work=WORK&workstream=stream&contract=' + (scenario === 'changed' ? 'old' : 'current'));
      const expectedOps = scenario === 'denied' ? ['work.snapshot'] : ['work.snapshot', 'work.prepare',
        ...(['detail', 'detail-no-guidance', 'observe-denied', 'observe-changed'].includes(scenario) ? ['work.observe'] : [])];
      assert.deepEqual(queries, expectedOps.map(op => ({ protocol_version: 1, op, work_id: 'WORK', workstream_id: 'stream',
        ...(op === 'work.snapshot' ? { max_context_bytes: 262144 } : {}) })));
      if (!scenario.startsWith('detail')) {
        assert.equal(result.json.ok, false);
        assert.equal(result.json.error.code, scenario.endsWith('changed') ? 'SourceChanged' : 'Forbidden');
        assert.equal(result.json.work, undefined);
      } else {
        assert.equal(result.json.work.status, null);
        assert.equal(result.json.work.session_id, 'session');
        assert.equal(result.json.work.next_step, 'Restore missing context');
        assert.equal(result.json.work.guidance.code, 'restore_context');
        assert.equal(result.json.work.claimant, null);
        assert.equal(result.json.work.last_participant, 'Developer');
        assert.equal(result.json.work.dependency_export_unavailable, true);
        assert.equal(result.json.work.snapshot.consistency, 'unconfirmed_legacy');
        assert.deepEqual(result.json.work.acceptance, ['Contract criterion']);
        assert.deepEqual(result.json.work.depends_on, [{ key: 'VISIBLE', visible: true }]);
      }
    } finally {
      await bridge.close();
      await upstream.close();
    }
  });
}


function atomicWorkFixture() {
  const runtime = { state: 'in_progress', work_version: '7', last_fence: '3', recovery_blocked: false, selected_completion_id: null };
  return { project_revision: '20', source_snapshot_id: 'source-snapshot', coordinator_epoch: 'epoch', workstream_id: 'stream', data: {
    work_id: 'WORK', contract_hash: 'current', runtime: { ...runtime }, context_complete: true,
    visible_contract: { acceptance: ['Check the result'], required_dependencies: [] },
    snapshot: { version: 1, consistency: 'repeatable_read', project_revision: '20', source_snapshot_id: 'source-snapshot',
      coordinator_epoch: 'epoch', queried_at_unix_ms: 1234 },
    observation: { work_id: 'WORK', contract_hash: 'current', observed_at_unix_ms: 1234, runtime: { ...runtime },
      session: { id: 'session', actor_name: 'Member', client_id: 'client' },
      progress: { phase: 'delivered', summary: 'Implementation submitted', reported_at_unix_ms: 1000,
        reported_contract_hash: 'current', recorded_project_revision: '19', client_observed_at_unix_ms: null, provenance: 'caller_declared' },
      execution: { state: 'succeeded', terminal_reported: true, artifact_verified: false, effects_settled: true,
        settlement_basis: 'caller_asserted', settlement_scope: 'admitted_workspace_paths' },
      guidance: { code: 'inspect_delivery', action: { note: 'Inspect the current candidate' } }, pr_deliveries: [],
    },
  } };
}

for (const scenario of ['normal', 'incomplete', 'forbidden', 'context_error', 'budget_error', 'server_error',
  'revision', 'source', 'epoch', 'query_time', 'runtime', 'empty_runtime', 'work', 'stream', 'version']) {
  test(`atomic work detail handles ${scenario} without an implicit legacy fallback`, async () => {
    const queries = [], fixture = atomicWorkFixture();
    if (scenario === 'incomplete') { fixture.data.context_complete = false; fixture.data.next_step = 'Restore the source'; }
    if (scenario === 'revision') fixture.data.snapshot.project_revision = '19';
    if (scenario === 'source') fixture.data.snapshot.source_snapshot_id = 'other';
    if (scenario === 'epoch') fixture.data.snapshot.coordinator_epoch = 'other';
    if (scenario === 'query_time') fixture.data.snapshot.queried_at_unix_ms = 1233;
    if (scenario === 'runtime') fixture.data.observation.runtime.work_version = '6';
    if (scenario === 'empty_runtime') { fixture.data.runtime = {}; fixture.data.observation.runtime = {}; }
    if (scenario === 'work') fixture.data.observation.work_id = 'OTHER';
    if (scenario === 'stream') fixture.workstream_id = 'other';
    if (scenario === 'version') fixture.data.snapshot.version = 2;
    const codes = { forbidden: [403, 'Forbidden'], context_error: [409, 'ContextChanged'],
      budget_error: [400, 'ContextIncomplete'], server_error: [500, 'DatabaseUnavailable'] };
    const upstream = await startMockUpstream((request, response) => {
      const chunks = []; request.on('data', chunk => chunks.push(chunk)); request.on('end', () => {
        queries.push(JSON.parse(Buffer.concat(chunks).toString())); response.setHeader('content-type', 'application/json');
        const error = codes[scenario];
        response.writeHead(error ? error[0] : 200);
        response.end(JSON.stringify(error ? { code: error[1], message: 'Synthetic read failure' } : fixture));
      });
    });
    const bridge = await startLiveBridge(upstream.base);
    try {
      const result = await req(bridge.base, 'GET', '/api/team/work?project=demo&work=WORK&workstream=stream&contract=current');
      assert.deepEqual(queries, [{ protocol_version: 1, op: 'work.snapshot', work_id: 'WORK', workstream_id: 'stream', max_context_bytes: 262144 }]);
      if (['normal', 'incomplete'].includes(scenario)) {
        const work = result.json.work;
        assert.equal(work.snapshot.consistency, 'repeatable_read'); assert.equal(work.snapshot.project_revision, '20');
        assert.equal(work.status, 'in_progress'); assert.equal(work.progress_report.phase, 'delivered');
        assert.equal(work.progress_report.recorded_project_revision, '19');
        assert.equal(work.progress_report.client_observed_at_unix_ms, null);
        assert.equal(work.execution.terminal_reported, true); assert.equal(work.execution.artifact_verified, false);
        assert.equal(work.execution.settlement_basis, 'caller_asserted');
        if (scenario === 'incomplete') assert.equal(work.guidance.code, 'restore_context');
      } else {
        assert.equal(result.json.ok, false); assert.equal(result.json.work, undefined);
        assert.equal(result.json.error.code, codes[scenario]?.[1] || 'BadGateway');
      }
    } finally { await bridge.close(); await upstream.close(); }
  });
}

test('a stalled optional repository observation never delays the atomic AWR response', async () => {
  let release, timer;
  const gate = new Promise(resolve => { release = resolve; });
  const fixture = atomicWorkFixture();
  fixture.data.observation.pr_deliveries = [{ state: 'active', contract_matches_current: true,
    url: 'https://github.com/example/repo/pull/1', head_sha: 'a'.repeat(40) }];
  const upstream = await startMockUpstream((_request, response) => {
    response.setHeader('content-type', 'application/json'); response.end(JSON.stringify(fixture));
  });
  const bridge = await startLiveBridge(upstream.base, { githubFetch: async () => { await gate; return { ok: false, status: 503 }; } });
  try {
    const result = await Promise.race([
      req(bridge.base, 'GET', '/api/team/work?project=demo&work=WORK&workstream=stream&contract=current'),
      new Promise((_resolve, reject) => { timer = setTimeout(() => reject(new Error('AWR waited for the optional repository')), 1000); }),
    ]);
    assert.equal(result.json.ok, true); assert.equal(result.json.work.progress_report.summary, 'Implementation submitted');
    assert.equal(result.json.work.github.pending, true); assert.equal(result.json.work.github.observed_at_ms, null);
  } finally { clearTimeout(timer); release(); await bridge.close(); await upstream.close(); }
});

for (const legacy of [false, true]) {
  test(`scoped detail deadline aborts the upstream read${legacy ? ' across legacy fallback' : ''} without retrying`, async () => {
    const queries = [];
    let aborted;
    const stopped = new Promise(resolve => { aborted = resolve; });
    const upstream = await startMockUpstream((request, response) => {
      const chunks = []; request.on('data', chunk => chunks.push(chunk)); request.on('end', () => {
        const query = JSON.parse(Buffer.concat(chunks).toString()); queries.push(query.op);
        if (legacy && query.op === 'work.snapshot') {
          response.writeHead(400, { 'content-type': 'application/json' });
          response.end(JSON.stringify({ code: 'Unsupported' }));
        } else response.on('close', () => aborted(!response.writableEnded));
      });
    });
    const bridge = await startLiveBridge(upstream.base);
    try {
      const result = await req(bridge.base, 'GET', '/api/team/work?project=demo&work=WORK&workstream=stream&contract=current');
      assert.equal(result.json.ok, false); assert.equal(result.json.error.code, 'ReadTimeout');
      assert.equal(result.json.work, undefined);
      assert.equal(await stopped, true);
      assert.deepEqual(queries, legacy ? ['work.snapshot', 'work.prepare'] : ['work.snapshot']);
    } finally { await bridge.close(); await upstream.close(); }
  });
}

test('member connection URL is configured independently of the private bridge URL', async () => {
  const upstream = await startMockUpstream(async (req, res) => {
    res.setHeader('content-type', 'application/json');
    if (req.url === '/v1/web/session') res.end(JSON.stringify({ session_id: 'synthetic' }));
    else res.end(JSON.stringify({ items: [], next_cursor: null }));
  });
  try {
    const bridge = createTeamBridge({ teamUrl: upstream.base, teamPublicUrl: 'https://team.example/awr', port: 7382 });
    const result = await bridge.routes['GET /api/team/overview'](new URL('http://localhost/api/team/overview?project=p'), null,
      { headers: { cookie: 'synthetic' } }, { setHeader() {} });
    assert.equal(result.mcp_url, 'https://team.example/awr/v1/projects/p/mcp');
    for (const url of ['https://user:secret@team.example', 'https://team.example/?token=hidden', 'https://team.example/;command'])
      assert.throws(() => createTeamBridge({ teamUrl: upstream.base, teamPublicUrl: url }));
  } finally { await upstream.close(); }
});


test('member administration and activity proxies allow only designed operations and preserve server denials', async () => {
  const calls = [];
  const upstream = await startMockUpstream((request, response) => {
    const chunks=[]; request.on('data', c=>chunks.push(c)); request.on('end',()=>{
      calls.push({ path: request.url, body: JSON.parse(Buffer.concat(chunks).toString()), cookie: request.headers.cookie });
      response.setHeader('content-type','application/json');
      if (request.headers.cookie !== 'awr_web_session=manager') {
        response.writeHead(403); response.end(JSON.stringify({ code: 'Forbidden' }));
      } else response.end(JSON.stringify(request.url.endsWith('/query') ? { data: { scope:'project',items:[],next_cursor:null } } : { items:[],next_cursor:null }));
    });
  });
  const bridge=await startLiveBridge(upstream.base);
  try {
    const directory=await req(bridge.base,'POST','/api/team/access',{ cookie:'awr_web_session=manager',body:{ project:'demo',operation:'inspect',payload:{protocol_version:1,limit:25} } });
    assert.equal(directory.json.ok,true); assert.deepEqual(directory.json.data.items,[]);
    const denied=await req(bridge.base,'POST','/api/team/access',{ cookie:'awr_web_session=member',body:{ project:'demo',operation:'inspect',payload:{protocol_version:1} } });
    assert.equal(denied.json.error.code,'Forbidden');
    const invalid=await req(bridge.base,'POST','/api/team/access',{ cookie:'awr_web_session=manager',body:{ project:'demo',operation:'../command',payload:{} } });
    assert.equal(invalid.json.error.code,'InvalidInput');
    const activity=await req(bridge.base,'GET','/api/team/activity?project=demo&kind=requests&member_actor_id=alex&cursor=page',{cookie:'awr_web_session=manager'});
    assert.equal(activity.json.ok,true);
    assert.deepEqual(calls.at(-1).body,{protocol_version:1,op:'audit.requests',limit:50,cursor:'page',member_actor_id:'alex'});
    assert.equal(calls.length,3);
  } finally {await bridge.close();await upstream.close();}
});
