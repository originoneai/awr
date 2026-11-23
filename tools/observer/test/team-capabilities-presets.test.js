'use strict';

/**
 * AWR-PPD-061: the bridge hands the capabilities reply's business duty
 * definitions to the frontend unchanged. The upstream reply served here is
 * the verbatim web-route sample captured from the real service
 * (crates/awr-server/tests/samples/capabilities-web-read.json, regenerated
 * and compared by its PostgreSQL suite). The exact per-duty ceilings below
 * deliberately follow the Rust `BusinessRole::action_ceiling`, so a drifted,
 * renamed, reordered or partially filtered field fails in this file too.
 */
const test = require('node:test');
const assert = require('node:assert/strict');
const http = require('http');
const fs = require('fs');
const path = require('path');
const { createTeamBridge } = require('../server/team-bridge');

const FIXTURE_DIR = path.resolve(__dirname, 'fixtures');
const SAMPLE = JSON.parse(
  fs.readFileSync(path.join(FIXTURE_DIR, 'capabilities-web-read.json'), 'utf8'),
);

const EXPECTED_CEILINGS = {
  observer: ['work.read'],
  developer: [
    'work.read',
    'session.maintain_own',
    'claim.manage_own',
    'execution.request_and_report_own',
    'planning.propose',
    'delivery.submit_and_request_review',
  ],
  reviewer: ['work.read', 'session.maintain_own', 'review.decide'],
  supervisor: [
    'work.read',
    'work.assign',
    'session.maintain_own',
    'planning.propose',
    'planning.edit_draft',
    'planning.approve',
    'planning.publish',
    'audit.read_project',
    'directory.manage_project',
  ],
  deliverer: ['work.read', 'session.maintain_own', 'delivery.finalize'],
  administrator: [
    'work.read',
    'access.manage_project',
    'audit.read_project',
    'directory.manage_project',
  ],
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

function req(base, method, urlPath, { body } = {}) {
  return new Promise((resolve, reject) => {
    const payload = body ? JSON.stringify(body) : null;
    const request = http.request(base + urlPath, {
      method,
      headers: {
        'x-awr-observer': '1',
        origin: 'http://127.0.0.1',
        ...(payload ? { 'content-type': 'application/json', 'content-length': Buffer.byteLength(payload) } : {}),
      },
    }, (response) => {
      const chunks = [];
      response.on('data', (c) => chunks.push(c));
      response.on('end', () => {
        try {
          resolve({ status: response.statusCode, json: JSON.parse(Buffer.concat(chunks).toString('utf8')) });
        } catch (error) {
          reject(error);
        }
      });
    });
    request.on('error', reject);
    if (payload) request.write(payload);
    request.end();
  });
}

test('the committed sample keeps every duty ceiling equal to the server contract', () => {
  assert.equal(SAMPLE.protocol, 'awr-team-workstream');
  const presets = SAMPLE.business_role_presets;
  assert.equal(presets.role_names_grant_authority, false, 'duty names stay descriptive');
  assert.deepEqual(presets.roles, [
    'observer', 'developer', 'reviewer', 'supervisor', 'deliverer', 'administrator',
  ]);
  assert.deepEqual(
    Object.keys(presets.action_ceilings).sort(),
    Object.keys(EXPECTED_CEILINGS).sort(),
  );
  for (const [duty, ceiling] of Object.entries(EXPECTED_CEILINGS)) {
    assert.deepEqual(presets.action_ceilings[duty], ceiling, `${duty} ceiling must stay exact`);
  }
});

test('the bridge passes the duty definitions to the frontend unchanged', async () => {
  const seen = [];
  const upstream = await startMockUpstream((request, response) => {
    const chunks = [];
    request.on('data', (c) => chunks.push(c));
    request.on('end', () => {
      seen.push({ path: request.url, query: JSON.parse(Buffer.concat(chunks).toString('utf8')) });
      response.setHeader('content-type', 'application/json');
      response.end(JSON.stringify(SAMPLE));
    });
  });
  const bridge = await startLiveBridge(upstream.base);
  try {
    const read = await req(bridge.base, 'POST', '/api/team/read', {
      body: { project: 'demo', query: { protocol_version: 1, op: 'capabilities' } },
    });
    assert.equal(read.status, 200);
    assert.equal(read.json.ok, true, JSON.stringify(read.json));
    // The whole reply is forwarded verbatim; the duty definitions are one
    // field of it, passed as-is with no renaming, reordering or filtering.
    assert.deepEqual(read.json.result, SAMPLE);
    const presets = read.json.result.business_role_presets;
    assert.equal(presets.role_names_grant_authority, false);
    assert.deepEqual(Object.keys(presets.action_ceilings).sort(), Object.keys(EXPECTED_CEILINGS).sort());
    for (const [duty, ceiling] of Object.entries(EXPECTED_CEILINGS)) {
      assert.deepEqual(presets.action_ceilings[duty], ceiling, `${duty} ceiling must pass through exactly`);
    }
    assert.deepEqual(seen, [
      { path: '/v1/web/projects/demo/query', query: { protocol_version: 1, op: 'capabilities' } },
    ]);
  } finally {
    await bridge.close();
    await upstream.close();
  }
});
