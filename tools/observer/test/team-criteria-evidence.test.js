'use strict';

/**
 * AWR-PPD-040 bridge contract tests against verbatim `work.snapshot` replies
 * captured from the real `awr-server` binary (tools/observer/test/fixtures/ppd040/).
 */
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('fs');
const path = require('path');
const http = require('http');
const { createTeamBridge } = require('../server/team-bridge');

const FIXTURE_DIR = path.resolve(__dirname, 'fixtures');
const PPD040_DIR = path.join(FIXTURE_DIR, 'ppd040');

function loadVerbatim(name) {
  const raw = fs.readFileSync(path.join(PPD040_DIR, name), 'utf8');
  return JSON.parse(raw);
}

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
    const json = await handler(url, null, req, res);
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

function get(base, urlPath) {
  return new Promise((resolve, reject) => {
    http.get(base + urlPath, (res) => {
      const chunks = [];
      res.on('data', (c) => chunks.push(c));
      res.on('end', () => resolve({ status: res.statusCode, json: JSON.parse(Buffer.concat(chunks).toString('utf8')) }));
    }).on('error', reject);
  });
}

test('bridge forwards verbatim work.snapshot acceptance_evidence across legacy, supported, candidate-change and returned-review samples', async () => {
  const samples = {
    legacy: loadVerbatim('work-snapshot-legacy.json'),
    supported: loadVerbatim('work-snapshot-supported.json'),
    candidateChanged: loadVerbatim('work-snapshot-candidate-changed.json'),
    reviewReturned: loadVerbatim('work-snapshot-review-returned.json'),
  };

  let current = samples.legacy;
  let denied = false;
  const upstream = await startMockUpstream((req, res) => {
    const chunks = [];
    req.on('data', (c) => chunks.push(c));
    req.on('end', () => {
      res.setHeader('content-type', 'application/json');
      if (denied) {
        res.writeHead(403);
        res.end(JSON.stringify({ code: 'Forbidden', message: 'revoked' }));
        return;
      }
      res.end(JSON.stringify(current));
    });
  });
  const bridge = await startLiveBridge(upstream.base);
  try {
    const workstream = samples.legacy.workstream_id;
    const workUrl = `/api/team/work?project=one&work=a&workstream=${encodeURIComponent(workstream)}`;

    // 1. Legacy task without examples -> status is not_provided and items is null (never []).
    current = samples.legacy;
    const legacy = await get(bridge.base, workUrl);
    assert.equal(legacy.json.ok, true);
    assert.equal(legacy.json.work.acceptance_evidence.status, 'not_provided');
    assert.equal(legacy.json.work.acceptance_evidence.items, null);

    // 2. Supported task: bound criterion supported, unbound criterion no_bound_check.
    current = samples.supported;
    const supported = await get(bridge.base, workUrl);
    assert.equal(supported.json.ok, true);
    assert.equal(supported.json.work.acceptance_evidence.status, 'provided');
    assert.equal(supported.json.work.acceptance_evidence.items.length, 2);
    assert.equal(supported.json.work.acceptance_evidence.items[0].status, 'supported');
    assert.deepEqual(
      supported.json.work.acceptance_evidence.items[0].bound_checks,
      ['unit.filter.pos', 'unit.filter.neg'],
    );
    assert.equal(supported.json.work.acceptance_evidence.items[1].status, 'no_bound_check');

    // 3. Candidate changed -> old evidence invalidated, criterion returns to waiting.
    current = samples.candidateChanged;
    const changed = await get(bridge.base, workUrl);
    assert.equal(changed.json.ok, true);
    assert.equal(changed.json.work.acceptance_evidence.items[0].status, 'waiting');
    assert.equal(changed.json.work.acceptance_evidence.review_current_candidate, false);

    // 4. Review returned -> criterion status is failed with reason review_returned.
    current = samples.reviewReturned;
    const returned = await get(bridge.base, workUrl);
    assert.equal(returned.json.ok, true);
    assert.equal(returned.json.work.acceptance_evidence.items[0].status, 'failed');
    assert.equal(returned.json.work.acceptance_evidence.items[0].reason, 'review_returned');

    // 5. Malformed acceptance_evidence fails closed with BadGateway.
    const malformed = structuredClone(samples.supported);
    malformed.data.acceptance_evidence.items[0].status = 'invented_status';
    current = malformed;
    const bad = await get(bridge.base, workUrl);
    assert.equal(bad.json.ok, false);
    assert.equal(bad.json.error.code, 'BadGateway');

    // 6. Revoked / 403 refusal returns Forbidden with no leaked acceptance_evidence.
    denied = true;
    const refused = await get(bridge.base, workUrl);
    assert.equal(refused.json.ok, false);
    assert.equal(refused.json.error.code, 'Forbidden');
    assert.equal(refused.json.work, undefined);
  } finally {
    await bridge.close();
    await upstream.close();
  }
});
