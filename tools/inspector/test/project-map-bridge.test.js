/**
 * Project map routes of the bridge: each /api/map route is one allowlisted read-only awr command with validated
 * arguments, equal to what the static export runs. The real server.js runs against a fake awr.
 *
 * Run: node --test test/project-map-bridge.test.js
 */

'use strict';

const { test, before, after } = require('node:test');
const assert = require('node:assert/strict');
const { spawn } = require('node:child_process');
const fs = require('node:fs');
const path = require('node:path');
const http = require('node:http');
const commands = require('../public/project-map/commands.js');
const h = require('./fixtures/project-map/helpers.js');
const { createRig } = require('./fixtures/project-map/rig.js');

const ROOT = path.join(__dirname, '..');
const GUARD = { 'x-awr-inspector': '1' };
const rig = createRig(h.readJson('states.json'), { 'display.json': h.readJson('config.json') });
const CONFIG = rig.file('display.json');
const TMP = rig.dir;
const startBridge = (extraArgs, env) => rig.startBridge(extraArgs, env);

/** Run server.js expecting it to refuse to start; resolves {code, stderr}. */
function startFailing(args) {
  return new Promise((resolve) => {
    const child = spawn(process.execPath, ['server.js', '--no-open', '--port', '1', ...args], { cwd: ROOT, stdio: ['ignore', 'pipe', 'pipe'] });
    let stderr = '';
    child.stderr.on('data', (d) => { stderr += d; });
    child.on('exit', (code) => resolve({ code, stderr }));
  });
}

const get = async (bridge, route) => (await fetch(`${bridge.base}${route}`, { headers: GUARD })).json();
const recorded = () => rig.calls();
/** The argument lists the bridge ran for `route`, without the leading --project <dir> --json. */
async function ran(bridge, route) {
  const before = recorded().length;
  const envelope = await get(bridge, route);
  return { envelope, argvs: recorded().slice(before).map((a) => a.slice(3)) };
}

let bridge;
before(async () => {
  bridge = await startBridge(['--map-config', CONFIG]);
});
after(async () => {
  if (bridge) await bridge.stop();
  rig.cleanup();
});

test('each map route runs exactly the command the export would run', async () => {
  const cursor = { project_id: 'P1', project_revision: 100, created_at: 1768348800000, event_id: 'E1' };
  const cases = [
    ['/api/map/work-graph?limit=250', 'workGraph', { limit: 250 }],
    ['/api/map/work-graph?limit=20&cached=1', 'workGraph', { limit: 20, cached: true }],
    ['/api/map/work-graph', 'workGraph', { limit: 100 }],
    ['/api/map/nav', 'nav', {}],
    ['/api/map/nav?milestone=M1', 'nav', { milestone: 'M1' }],
    ['/api/map/goals', 'goals', {}],
    ['/api/map/events?limit=1000&through=100', 'events', { limit: 1000, through: 100 }],
    [`/api/map/events?limit=500&through=100&cursor=${encodeURIComponent(JSON.stringify(cursor))}`, 'events', { limit: 500, through: 100, cursor }],
    ['/api/map/sessions', 'sessions', {}],
    ['/api/map/doctor', 'doctor', {}],
  ];
  for (const [route, name, params] of cases) {
    const { envelope, argvs } = await ran(bridge, route);
    assert.deepEqual(argvs, [commands.argvFor(name, params)], route);
    assert.ok(envelope.command.startsWith('awr '), route);
  }
});

test('every command is read-only: none of the map routes can start, claim, append, reindex or complete anything', async () => {
  for (const route of ['/api/map/work-graph?limit=5', '/api/map/nav?milestone=M1', '/api/map/goals', '/api/map/events', '/api/map/sessions', '/api/map/doctor']) {
    const { argvs } = await ran(bridge, route);
    for (const argv of argvs) assert.ok(!argv.some((a) => ['reindex', 'complete', 'claim', 'append', 'start', 'end', 'progress', 'block', 'cancel', 'create'].includes(a)), argv.join(' '));
  }
  // and the map has no write route at all
  for (const method of ['POST', 'PUT', 'DELETE']) {
    const res = await fetch(`${bridge.base}/api/map/work-graph`, { method, headers: GUARD, body: method === 'DELETE' ? undefined : '{}' });
    assert.equal((await res.json()).error.code, 'NoRoute');
  }
});

test('bad parameters are rejected before awr is started', async () => {
  const bad = [
    '/api/map/work-graph?limit=0', '/api/map/work-graph?limit=1001', '/api/map/work-graph?limit=abc', '/api/map/work-graph?limit=1.5',
    '/api/map/nav?milestone=not%20a%20key', `/api/map/nav?milestone=${'x'.repeat(201)}`, '/api/map/nav?milestone=M1;rm',
    '/api/map/events?limit=0', '/api/map/events?limit=1001', '/api/map/events?through=-1', '/api/map/events?through=x',
    '/api/map/events?cursor=not-json', `/api/map/events?cursor=${encodeURIComponent('{"event_id":"E"}')}`,
    `/api/map/events?cursor=${encodeURIComponent(JSON.stringify({ project_id: 'P', project_revision: 1, created_at: 1, event_id: 'E', extra: 1 }))}`,
    `/api/map/events?cursor=${encodeURIComponent(JSON.stringify({ project_id: 'P;x', project_revision: 1, created_at: 1, event_id: 'E' }))}`,
    `/api/map/events?cursor=${encodeURIComponent(JSON.stringify({ project_id: 'P', project_revision: -1, created_at: 1, event_id: 'E' }))}`,
  ];
  for (const route of bad) {
    const { envelope, argvs } = await ran(bridge, route);
    assert.equal(envelope.ok, false, route);
    assert.equal(envelope.error.code, 'BadRequest', route);
    assert.deepEqual(argvs, [], `${route} must not reach awr`);
  }
});

test('doctor findings come back as a result even though awr exits nonzero', async () => {
  const envelope = await get(bridge, '/api/map/doctor');
  assert.equal(envelope.ok, false);
  assert.equal(envelope.error.code, 'CommandFailed');
  assert.equal(envelope.data.findings.length, 3);
  assert.equal(envelope.data.ok, false);
});

test('the display configuration is served from --map-config, and only its base name is shown', async () => {
  const envelope = await get(bridge, '/api/map/config');
  assert.equal(envelope.ok, true);
  assert.deepEqual(envelope.data.config, h.readJson('config.json'));
  assert.equal(envelope.data.source, 'display.json');
  assert.ok(!JSON.stringify(envelope).includes(TMP), 'no local path is disclosed');
  const plain = await startBridge();
  try {
    assert.deepEqual((await get(plain, '/api/map/config')).data, { config: null, source: null });
  } finally {
    await plain.stop();
  }
});

test('an invalid or missing --map-config stops the server with a readable message', async () => {
  const broken = path.join(TMP, 'broken.json');
  fs.writeFileSync(broken, '{"stale_days": 0}');
  let result = await startFailing(['--map-config', broken]);
  assert.equal(result.code, 1);
  assert.match(result.stderr, /--map-config: .*stale_days/);
  fs.writeFileSync(broken, 'not json');
  result = await startFailing(['--map-config', broken]);
  assert.match(result.stderr, /not valid JSON/);
  result = await startFailing(['--map-config', path.join(TMP, 'missing.json')]);
  assert.match(result.stderr, /cannot read missing\.json/);
  assert.doesNotMatch(result.stderr, new RegExp(TMP.replace(/[\\/]/g, '.')), 'only the file name is reported');
});

test('demo mode serves no live map data', async () => {
  const demo = await startBridge(['--demo']);
  try {
    const before = recorded().length;
    const envelope = await get(demo, '/api/map/work-graph?limit=5');
    assert.equal(envelope.error.code, 'DemoMode');
    assert.equal(recorded().length, before);
  } finally {
    await demo.stop();
  }
});

test('the request-origin boundary covers the map routes', async () => {
  const res = await new Promise((resolve, reject) => {
    const req = http.request({ host: '127.0.0.1', port: bridge.port, path: '/api/map/nav', method: 'GET', headers: { Host: 'attacker.example' } }, (r) => {
      const chunks = [];
      r.on('data', (c) => chunks.push(c));
      r.on('end', () => resolve({ status: r.statusCode, body: JSON.parse(Buffer.concat(chunks).toString('utf8')) }));
    });
    req.on('error', reject);
    req.end();
  });
  assert.equal(res.status, 403);
  assert.equal(res.body.error.code, 'ForbiddenHost');
});
