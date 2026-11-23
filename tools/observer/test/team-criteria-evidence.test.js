'use strict';

/**
 * AWR-PPD-040 bridge contract tests against verbatim `work.snapshot` replies
 * captured from the real `awr-server` binary (tools/observer/test/fixtures/ppd040/).
 * Exercises both `GET /api/team/work` and `POST /api/team/read` (`work.snapshot`).
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

function get(base, urlPath) {
  return new Promise((resolve, reject) => {
    http.get(base + urlPath, (res) => {
      const chunks = [];
      res.on('data', (c) => chunks.push(c));
      res.on('end', () => resolve({ status: res.statusCode, json: JSON.parse(Buffer.concat(chunks).toString('utf8')) }));
    }).on('error', reject);
  });
}

function post(base, urlPath, body) {
  return new Promise((resolve, reject) => {
    const req = http.request(base + urlPath, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
    }, (res) => {
      const chunks = [];
      res.on('data', (c) => chunks.push(c));
      res.on('end', () => resolve({ status: res.statusCode, json: JSON.parse(Buffer.concat(chunks).toString('utf8')) }));
    });
    req.on('error', reject);
    req.write(JSON.stringify(body));
    req.end();
  });
}

test('bridge forwards and validates verbatim work.snapshot acceptance_evidence on both GET /api/team/work and POST /api/team/read', async () => {
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
    const readBody = {
      project: 'one',
      query: {
        protocol_version: 1,
        op: 'work.snapshot',
        work_id: 'a',
        workstream_id: workstream,
        max_context_bytes: 262144,
      },
    };

    // 1. Legacy task without examples -> status is not_provided, basis distinguishes legacy from omitted, items is null.
    current = samples.legacy;
    const legacy = await get(bridge.base, workUrl);
    assert.equal(legacy.json.ok, true);
    assert.equal(legacy.json.work.acceptance_evidence.status, 'not_provided');
    assert.equal(legacy.json.work.acceptance_evidence.basis, 'contract_examples_absent');
    assert.equal(legacy.json.work.acceptance_evidence.items, null);
    const legacyRead = await post(bridge.base, '/api/team/read', readBody);
    assert.equal(legacyRead.json.ok, true);
    assert.equal(legacyRead.json.result.data.acceptance_evidence.status, 'not_provided');
    assert.equal(legacyRead.json.result.data.acceptance_evidence.basis, 'contract_examples_absent');
    assert.equal(legacyRead.json.result.data.acceptance_evidence.items, null);

    // 1b. Older upstream that omits acceptance_evidence is explicitly marked upstream_omitted on /api/team/work,
    //     distinct from a legacy contract's contract_examples_absent.
    const omitted = structuredClone(samples.legacy);
    delete omitted.data.acceptance_evidence;
    current = omitted;
    const omittedWork = await get(bridge.base, workUrl);
    assert.equal(omittedWork.json.ok, true);
    assert.equal(omittedWork.json.work.acceptance_evidence.status, 'not_provided');
    assert.equal(omittedWork.json.work.acceptance_evidence.basis, 'upstream_omitted');
    assert.equal(omittedWork.json.work.acceptance_evidence.items, null);
    const omittedRead = await post(bridge.base, '/api/team/read', readBody);
    assert.equal(omittedRead.json.ok, true);
    assert.equal(omittedRead.json.result.data.acceptance_evidence, undefined);

    // 2. Supported task: bound criterion supported, unbound criterion no_bound_check.
    current = samples.supported;
    const supported = await get(bridge.base, workUrl);
    assert.equal(supported.json.ok, true);
    assert.equal(supported.json.work.acceptance_evidence.status, 'provided');
    assert.equal(supported.json.work.acceptance_evidence.basis, 'contract_examples_and_current_candidate');
    assert.equal(supported.json.work.acceptance_evidence.facts_truncated, false);
    assert.equal(supported.json.work.acceptance_evidence.review_current_candidate, true);
    assert.equal(supported.json.work.acceptance_evidence.items.length, 2);
    assert.equal(supported.json.work.acceptance_evidence.items[0].status, 'supported');
    assert.equal(supported.json.work.acceptance_evidence.items[0].reason, 'bound_checks_passed_and_review_approved');
    assert.deepEqual(
      supported.json.work.acceptance_evidence.items[0].bound_checks,
      ['unit.filter.pos', 'unit.filter.neg'],
    );
    assert.equal(supported.json.work.acceptance_evidence.items[1].status, 'no_bound_check');
    const supportedRead = await post(bridge.base, '/api/team/read', readBody);
    assert.equal(supportedRead.json.ok, true);
    assert.deepEqual(
      supportedRead.json.result.data.acceptance_evidence,
      samples.supported.data.acceptance_evidence,
    );

    // 3. Candidate changed -> old evidence invalidated, criterion returns to waiting.
    current = samples.candidateChanged;
    const changed = await get(bridge.base, workUrl);
    assert.equal(changed.json.ok, true);
    assert.equal(changed.json.work.acceptance_evidence.items[0].status, 'waiting');
    assert.equal(changed.json.work.acceptance_evidence.review_current_candidate, false);
    const changedRead = await post(bridge.base, '/api/team/read', readBody);
    assert.equal(changedRead.json.ok, true);
    assert.equal(changedRead.json.result.data.acceptance_evidence.review_current_candidate, false);

    // 4. Review returned -> criterion status is failed with reason review_returned.
    current = samples.reviewReturned;
    const returned = await get(bridge.base, workUrl);
    assert.equal(returned.json.ok, true);
    assert.equal(returned.json.work.acceptance_evidence.items[0].status, 'failed');
    assert.equal(returned.json.work.acceptance_evidence.items[0].reason, 'review_returned');
    const returnedRead = await post(bridge.base, '/api/team/read', readBody);
    assert.equal(returnedRead.json.ok, true);
    assert.equal(returnedRead.json.result.data.acceptance_evidence.items[0].reason, 'review_returned');

    // 5. Truncated delivery facts and inactive work reasons pass strict validation on both routes.
    const truncated = structuredClone(samples.supported);
    truncated.data.acceptance_evidence.facts_truncated = true;
    truncated.data.acceptance_evidence.items[0].status = 'waiting';
    truncated.data.acceptance_evidence.items[0].reason = 'facts_truncated';
    truncated.data.acceptance_evidence.items[0].checks[1] = {
      check: 'unit.filter.neg',
      outcome: 'unknown',
      run_id: null,
    };
    current = truncated;
    const truncatedWork = await get(bridge.base, workUrl);
    assert.equal(truncatedWork.json.ok, true);
    assert.equal(truncatedWork.json.work.acceptance_evidence.facts_truncated, true);
    assert.equal(truncatedWork.json.work.acceptance_evidence.items[0].reason, 'facts_truncated');
    assert.equal(truncatedWork.json.work.acceptance_evidence.items[0].checks[1].outcome, 'unknown');
    const truncatedRead = await post(bridge.base, '/api/team/read', readBody);
    assert.equal(truncatedRead.json.ok, true);
    assert.equal(truncatedRead.json.result.data.acceptance_evidence.items[0].reason, 'facts_truncated');

    const inactive = structuredClone(samples.candidateChanged);
    inactive.data.acceptance_evidence.items[0].status = 'not_started';
    inactive.data.acceptance_evidence.items[0].reason = 'work_inactive';
    current = inactive;
    assert.equal((await get(bridge.base, workUrl)).json.ok, true);
    assert.equal((await post(bridge.base, '/api/team/read', readBody)).json.ok, true);

    // 6. Malformed acceptance_evidence fails closed with BadGateway on BOTH routes.
    const invalidCases = [
      ['invented status', (s) => { s.data.acceptance_evidence.items[0].status = 'invented_status'; }],
      ['missing reason', (s) => { delete s.data.acceptance_evidence.items[0].reason; }],
      ['null reason', (s) => { s.data.acceptance_evidence.items[0].reason = null; }],
      ['invented reason', (s) => { s.data.acceptance_evidence.items[0].reason = 'invented_reason'; }],
      ['status/reason mismatch', (s) => { s.data.acceptance_evidence.items[0].reason = 'work_not_started'; }],
      ['missing examples_count (must not default to 0)', (s) => { delete s.data.acceptance_evidence.items[0].examples_count; }],
      ['missing positive_examples (must not default to 0)', (s) => { delete s.data.acceptance_evidence.items[0].positive_examples; }],
      ['missing negative_examples (must not default to 0)', (s) => { delete s.data.acceptance_evidence.items[0].negative_examples; }],
      ['mismatched example counts sum', (s) => { s.data.acceptance_evidence.items[0].examples_count = 99; }],
      ['JSON string instead of top-level object', (s) => { s.data.acceptance_evidence = JSON.stringify(s.data.acceptance_evidence); }],
      ['JSON string instead of item object', (s) => { s.data.acceptance_evidence.items[0] = JSON.stringify(s.data.acceptance_evidence.items[0]); }],
      ['JSON string instead of check object', (s) => { s.data.acceptance_evidence.items[0].checks[0] = JSON.stringify(s.data.acceptance_evidence.items[0].checks[0]); }],
      ['null acceptance_evidence', (s) => { s.data.acceptance_evidence = null; }],
      ['not_provided with empty items array', (s) => {
        s.data.acceptance_evidence = { ...samples.legacy.data.acceptance_evidence, items: [] };
      }],
      ['not_provided with missing basis', (s) => {
        s.data.acceptance_evidence = { ...samples.legacy.data.acceptance_evidence };
        delete s.data.acceptance_evidence.basis;
      }],
      ['provided with missing facts_truncated boolean', (s) => { delete s.data.acceptance_evidence.facts_truncated; }],
      ['provided with missing review_current_candidate boolean', (s) => { delete s.data.acceptance_evidence.review_current_candidate; }],
      ['unknown check outcome', (s) => { s.data.acceptance_evidence.items[0].checks[0].outcome = 'errored'; }],
    ];
    for (const [label, mutate] of invalidCases) {
      const malformed = structuredClone(samples.supported);
      mutate(malformed);
      current = malformed;
      const badWork = await get(bridge.base, workUrl);
      assert.equal(badWork.json.ok, false, `GET /api/team/work must reject: ${label}`);
      assert.equal(badWork.json.error.code, 'BadGateway', `GET /api/team/work BadGateway for: ${label}`);
      const badRead = await post(bridge.base, '/api/team/read', readBody);
      assert.equal(badRead.json.ok, false, `POST /api/team/read must reject: ${label}`);
      assert.equal(badRead.json.error.code, 'BadGateway', `POST /api/team/read BadGateway for: ${label}`);
    }

    // 7. Revoked / 403 refusal returns Forbidden with no leaked acceptance_evidence on both routes.
    denied = true;
    const refused = await get(bridge.base, workUrl);
    assert.equal(refused.json.ok, false);
    assert.equal(refused.json.error.code, 'Forbidden');
    assert.equal(refused.json.work, undefined);
    const refusedRead = await post(bridge.base, '/api/team/read', readBody);
    assert.equal(refusedRead.json.ok, false);
    assert.equal(refusedRead.json.error.code, 'Forbidden');
    assert.equal(refusedRead.json.result, undefined);
  } finally {
    await bridge.close();
    await upstream.close();
  }
});
