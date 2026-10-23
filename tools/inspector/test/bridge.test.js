/**
 * Bridge tests using node:test with no dependencies.
 *
 * Each case starts the real server.js with a fake awr on PATH and uses fetch
 * for real HTTP requests, exercising the request boundary rather than internals.
 *
 * Run: node --test test/
 */

'use strict';

const { test, before, after } = require('node:test');
const assert = require('node:assert');
const { spawn } = require('node:child_process');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const http = require('node:http');

const ROOT = path.join(__dirname, '..');
const GUARD = { 'x-awr-inspector': '1' };

/** Create a bin directory containing an awr launcher for the stub. */
function makeStubBin() {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'awr-stub-'));
  const stub = path.join(__dirname, 'fixtures', 'stub-awr.js');
  if (process.platform === 'win32') {
    const bin = path.join(dir, 'awr.cmd');
    fs.writeFileSync(bin, `"${process.execPath}" "${stub}" %*\r\n`);
    return dir;
  }
  const bin = path.join(dir, 'awr');
  fs.writeFileSync(bin, `#!/bin/sh\nexec "${process.execPath}" "${stub}" "$@"\n`);
  fs.chmodSync(bin, 0o755);
  return dir;
}

const STUB_BIN = makeStubBin();
let nextPort = 7500;

/** Start a bridge, wait until it listens, and return {port, stop}. */
async function startBridge(opts = {}) {
  const port = nextPort++;
  const args = ['server.js', '--no-open', '--port', String(port)];
  const project = opts.project || ROOT;
  args.push('--project', project);
  if (opts.allowReindex) args.push('--allow-reindex');
  if (opts.demo) args.push('--demo');
  if (opts.teamOnly) args.push('--team-only', '--team-url', 'http://127.0.0.1:1');

  const sep = process.platform === 'win32' ? ';' : ':';
  const child = spawn(process.execPath, args, {
    cwd: ROOT,
    env: Object.assign({}, process.env, opts.env, { PATH: `${STUB_BIN}${sep}${process.env.PATH}` }),
    stdio: ['ignore', 'pipe', 'pipe'],
  });

  await new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error('Bridge startup timed out')), 15000);
    child.stdout.on('data', (d) => {
      if (String(d).includes('started')) {
        clearTimeout(timer);
        resolve();
      }
    });
    child.on('exit', (code) => {
      clearTimeout(timer);
      reject(new Error(`Bridge exited with code=${code}`));
    });
  });

  return {
    port,
    base: `http://127.0.0.1:${port}`,
    stop: () => new Promise((r) => { child.on('exit', r); child.kill('SIGKILL'); }),
  };
}

let bridge;

before(async () => { bridge = await startBridge(); });
after(async () => { if (bridge) await bridge.stop(); fs.rmSync(STUB_BIN, { recursive: true, force: true }); });

// 1. Request-origin boundary

/** fetch cannot override Host; construct hostile Host headers with a raw request. */
function rawRequest(port, options) {
  return new Promise((resolve, reject) => {
    const req = http.request(
      Object.assign({ host: '127.0.0.1', port, method: 'GET' }, options),
      (res) => {
        const chunks = [];
        res.on('data', (c) => chunks.push(c));
        res.on('end', () =>
          resolve({ status: res.statusCode, text: Buffer.concat(chunks).toString('utf8') })
        );
      }
    );
    req.on('error', reject);
    if (options && options.body) req.write(options.body);
    req.end();
  });
}

test('rejects hostile Host headers (DNS rebinding)', async () => {
  const res = await rawRequest(bridge.port, {
    path: '/api/health',
    headers: { Host: 'attacker.example' },
  });
  assert.equal(res.status, 403);
  const body = JSON.parse(res.text);
  assert.equal(body.error.code, 'ForbiddenHost');
  assert.ok(!res.text.includes('agent-work-runtime'), 'must not disclose the project path');
});

test('accepts localhost Host headers', async () => {
  const res = await rawRequest(bridge.port, {
    path: '/api/health',
    headers: { Host: `localhost:${bridge.port}` },
  });
  assert.equal(res.status, 200);
});

test('Team-only deployment advertises its scope and rejects local project routes', async () => {
  const team = await startBridge({ teamOnly: true });
  try {
    const health = await (await fetch(team.base + '/api/health')).json();
    assert.equal(health.data.teamOnly, true);
    assert.equal(health.data.mode, 'team');
    assert.equal(health.data.project, null);
    for (const route of ['/api/status', '/api/sources', '/api/work?key=example']) {
      const result = await fetch(team.base + route);
      assert.equal(result.status, 403);
      assert.equal((await result.json()).error.code, 'TeamOnlyDeployment');
    }
  } finally {
    await team.stop();
  }
});

test('rejects hostile origins', async () => {
  const res = await fetch(`${bridge.base}/api/status`, {
    headers: Object.assign({ origin: 'https://example.attacker' }, GUARD),
  });
  assert.equal(res.status, 403);
  assert.equal((await res.json()).error.code, 'ForbiddenOrigin');
});

test("rejects Origin: null", async () => {
  const res = await fetch(`${bridge.base}/api/status`, { headers: { origin: 'null' } });
  assert.equal(res.status, 403);
  assert.equal((await res.json()).error.code, 'ForbiddenOrigin');
});

test('rejects cross-site Sec-Fetch-Site', async () => {
  const res = await fetch(`${bridge.base}/api/status`, {
    headers: { 'sec-fetch-site': 'cross-site' },
  });
  assert.equal(res.status, 403);
  assert.equal((await res.json()).error.code, 'ForbiddenSite');
});

test('accepts same-origin Sec-Fetch-Site', async () => {
  const res = await fetch(`${bridge.base}/api/status`, {
    headers: { 'sec-fetch-site': 'same-origin' },
  });
  assert.equal(res.status, 200);
  assert.equal((await res.json()).ok, true);
});

test('cross-site form POST cannot trigger reindex', async () => {
  const res = await fetch(`${bridge.base}/api/source/reindex`, {
    method: 'POST',
    headers: { 'content-type': 'application/x-www-form-urlencoded' },
    body: 'x=1',
  });
  assert.equal(res.status, 403);
  assert.equal((await res.json()).error.code, 'MissingGuardHeader');
});

test('static resources reject path traversal', async () => {
  const res = await fetch(`${bridge.base}/../server.js`);
  assert.ok(res.status === 403 || res.status === 404, `expected 403/404, received ${res.status}`);
});

test('static responses include CSP', async () => {
  const res = await fetch(`${bridge.base}/`);
  const csp = res.headers.get('content-security-policy');
  assert.ok(csp && csp.includes("default-src 'self'"), 'CSP header is missing');
  assert.ok(!csp.includes('unsafe-inline'), 'CSP must not allow inline resources');
});

// 2. Command construction

test('search uses positional text rather than --text', async () => {
  const argvLog = path.join(os.tmpdir(), `argv-${Date.now()}.log`);
  const b = await startBridge({ env: { STUB_ARGV_OUT: argvLog } });
  try {
    const res = await fetch(`${b.base}/api/search?text=source`, { headers: GUARD });
    const body = await res.json();
    assert.equal(body.ok, true, JSON.stringify(body));
    assert.equal(body.data.query.text, 'source');

    const lines = fs.readFileSync(argvLog, 'utf8').trim().split('\n').map(JSON.parse);
    const call = lines.find((a) => a.includes('search'));
    assert.ok(!call.includes('--text'), 'must not pass --text');
    assert.ok(call.includes('--'), 'must separate positional arguments with --');
  } finally {
    await b.stop();
    fs.rmSync(argvLog, { force: true });
  }
});

test('search accepts Chinese text', async () => {
  const res = await fetch(`${bridge.base}/api/search?text=${encodeURIComponent('任务')}`, {
    headers: GUARD,
  });
  const body = await res.json();
  assert.equal(body.ok, true, JSON.stringify(body));
  assert.equal(body.data.query.text, '任务');
});

test('search accepts text starting with a hyphen', async () => {
  const res = await fetch(`${bridge.base}/api/search?text=${encodeURIComponent('-flag')}`, {
    headers: GUARD,
  });
  const body = await res.json();
  assert.equal(body.ok, true, JSON.stringify(body));
  assert.equal(body.data.query.text, '-flag');
});

test('search rejects control characters', async () => {
  const withNul = 'a' + String.fromCharCode(0) + 'b';
  const res = await fetch(`${bridge.base}/api/search?text=${encodeURIComponent(withNul)}`, {
    headers: GUARD,
  });
  assert.equal((await res.json()).error.code, 'BadRequest');
});

test('a bad request does not change the --json position', async () => {
  const argvLog = path.join(os.tmpdir(), `argv2-${Date.now()}.log`);
  const b = await startBridge({ env: { STUB_ARGV_OUT: argvLog } });
  try {
    // First send a request that makes the stub report an unexpected argument.
    await fetch(`${b.base}/api/work?key=NOPE%3B`, { headers: GUARD });
    // Then send a valid request; --json must still be in the global position.
    await fetch(`${b.base}/api/status`, { headers: GUARD });
    const lines = fs.readFileSync(argvLog, 'utf8').trim().split('\n').map(JSON.parse);
    const last = lines[lines.length - 1];
    assert.equal(last.indexOf('--json'), 2, `--json must not move to the end: ${JSON.stringify(last)}`);
  } finally {
    await b.stop();
    fs.rmSync(argvLog, { force: true });
  }
});

// 3. Child-process output

test('multibyte UTF-8 survives byte-by-byte output', async () => {
  const b = await startBridge({ env: { STUB_MODE: 'multibyte' } });
  try {
    const body = await (await fetch(`${b.base}/api/status`, { headers: GUARD })).json();
    assert.equal(body.ok, true, JSON.stringify(body));
    assert.equal(body.data.title, '任务：源文件索引 — αβγ 🧭');
    assert.ok(!JSON.stringify(body).includes('�'), 'replacement character found');
  } finally {
    await b.stop();
  }
});

test('oversized output is bounded instead of exhausting memory', async () => {
  const b = await startBridge({ env: { STUB_MODE: 'huge' } });
  try {
    const body = await (await fetch(`${b.base}/api/status`, { headers: GUARD })).json();
    assert.equal(body.ok, false);
    assert.equal(body.error.code, 'OutputTooLarge');
  } finally {
    await b.stop();
  }
});

test('oversized request bodies are rejected', async () => {
  const res = await fetch(`${bridge.base}/api/context/compile`, {
    method: 'POST',
    headers: Object.assign({ 'content-type': 'application/json' }, GUARD),
    body: JSON.stringify({ work: 'A', intent: 'x'.repeat(200000) }),
  });
  assert.equal(res.status, 413);
  assert.equal((await res.json()).error.code, 'BodyTooLarge');
});

test('JSON errors on stderr preserve their error code', async () => {
  const b = await startBridge({ env: { STUB_MODE: 'stderrjson' } });
  try {
    const body = await (await fetch(`${b.base}/api/status`, { headers: GUARD })).json();
    assert.equal(body.ok, false);
    assert.equal(body.error.code, 'SourceStale');
  } finally {
    await b.stop();
  }
});

// 4. Reindex semantics

test('reindex is disabled by default', async () => {
  const body = await (
    await fetch(`${bridge.base}/api/source/reindex`, { method: 'POST', headers: GUARD })
  ).json();
  assert.equal(body.ok, false);
  assert.equal(body.error.code, 'ReindexNotAllowed');
});

test('--allow-reindex enables reindex', async () => {
  const b = await startBridge({ allowReindex: true });
  try {
    const body = await (
      await fetch(`${b.base}/api/source/reindex`, { method: 'POST', headers: GUARD })
    ).json();
    assert.equal(body.ok, true, JSON.stringify(body));
  } finally {
    await b.stop();
  }
});

// 5. Demo mode

test('demo mode executes no awr commands', async () => {
  const argvLog = path.join(os.tmpdir(), `argv3-${Date.now()}.log`);
  const b = await startBridge({ demo: true, env: { STUB_ARGV_OUT: argvLog } });
  try {
    const health = await (await fetch(`${b.base}/api/health`, { headers: GUARD })).json();
    assert.equal(health.data.mode, 'demo');

    const status = await (await fetch(`${b.base}/api/status`, { headers: GUARD })).json();
    assert.equal(status.error.code, 'DemoMode');

    assert.ok(!fs.existsSync(argvLog), 'demo mode must not invoke awr');
  } finally {
    await b.stop();
    fs.rmSync(argvLog, { force: true });
  }
});

// 6. Four P2 regressions from review

test('malformed request targets do not terminate the server', async () => {
  const b = await startBridge();
  try {
    // GET // HTTP/1.1 causes new URL('//', base) to throw.
    const bad = await new Promise((resolve, reject) => {
      const sock = require('node:net').connect(b.port, '127.0.0.1', () => {
        sock.write('GET // HTTP/1.1\r\nHost: 127.0.0.1:' + b.port + '\r\n\r\n');
      });
      let text = '';
      sock.on('data', (d) => (text += d));
      sock.on('end', () => resolve(text));
      sock.on('error', reject);
      setTimeout(() => { sock.end(); resolve(text); }, 1500);
    });
    assert.ok(/HTTP\/1\.1 4\d\d/.test(bad), `expected 4xx, received headers: ${bad.slice(0, 60)}`);

    // The process must remain alive and serve subsequent requests.
    const health = await fetch(`${b.base}/api/health`, { headers: GUARD });
    assert.equal(health.status, 200);
    assert.equal((await health.json()).ok, true);
  } finally {
    await b.stop();
  }
});

test('slots follow child lifetimes rather than HTTP responses', async () => {
  // Use a 300 ms write timeout with a 30-second child lifetime; the response returns first.
  const b = await startBridge({
    allowReindex: true,
    env: { STUB_MODE: 'slowwrite', AWR_INSPECTOR_WRITE_TIMEOUT_MS: '300' },
  });
  try {
    const hit = () =>
      fetch(`${b.base}/api/source/reindex`, { method: 'POST', headers: GUARD }).then((r) => r.json());

    const first = [];
    for (let i = 0; i < 4; i++) first.push(await hit());
    for (const r of first) {
      assert.equal(r.error.code, 'OutcomeUnknown', JSON.stringify(r));
    }

    // All four children are still alive; reject the fifth command.
    const fifth = await hit();
    assert.equal(fifth.error.code, 'BridgeBusy', JSON.stringify(fifth));
  } finally {
    await b.stop();
  }
});

test('write output overflow keeps the child alive and reports an unknown outcome', async () => {
  const b = await startBridge({
    allowReindex: true,
    env: { STUB_MODE: 'hugewrite' },
  });
  try {
    const r = await (
      await fetch(`${b.base}/api/source/reindex`, { method: 'POST', headers: GUARD })
    ).json();
    assert.equal(r.ok, false);
    // Not OutputTooLarge: the writer was not killed and its outcome is unknown.
    assert.equal(r.error.code, 'OutcomeUnknown', JSON.stringify(r));
    assert.ok(!/run.*directly|retry/i.test(r.error.message) || /do not retry blindly/i.test(r.error.message),
      'must not recommend blindly rerunning a write with an unknown outcome');
  } finally {
    await b.stop();
  }
});

test('read-only output overflow reports OutputTooLarge', async () => {
  const b = await startBridge({ env: { STUB_MODE: 'huge' } });
  try {
    const r = await (await fetch(`${b.base}/api/status`, { headers: GUARD })).json();
    assert.equal(r.error.code, 'OutputTooLarge', JSON.stringify(r));
  } finally {
    await b.stop();
  }
});

// 7. Frontend detail-request generation guard

test('late detail responses cannot replace the current selection', () => {
  const { createGenerationGuard } = require('../public/app.js');
  const guard = createGenerationGuard();

  const a = guard.begin('A');
  const b = guard.begin('B');

  // B arrives first and is current; accept it.
  assert.equal(guard.isCurrent(b), true);
  // A arrives later and is stale; discard it.
  assert.equal(guard.isCurrent(a), false);
});

test('refresh invalidates in-flight detail requests', () => {
  const { createGenerationGuard } = require('../public/app.js');
  const guard = createGenerationGuard();

  const inflight = guard.begin('A');
  guard.invalidate();
  assert.equal(guard.isCurrent(inflight), false, 'old requests must not apply after refresh');

  // An older request for the same key is also invalid after refresh.
  const fresh = guard.begin('A');
  const older = { generation: fresh.generation - 1, key: 'A' };
  assert.equal(guard.isCurrent(older), false);
  assert.equal(guard.isCurrent(fresh), true);
});

test('preserve reports returned with a nonzero exit code', async () => {
  // Incomplete context exits with code 1 but its stdout report must remain available.
  const b = await startBridge({ env: { STUB_MODE: 'incomplete' } });
  try {
    const r = await (
      await fetch(`${b.base}/api/context/compile`, {
        method: 'POST',
        headers: Object.assign({ 'content-type': 'application/json' }, GUARD),
        body: JSON.stringify({ work: 'RECON-020', budget: 8000 }),
      })
    ).json();

    assert.equal(r.ok, false, 'Report the nonzero exit as failure');
    assert.equal(r.error.code, 'ContextIncomplete');
    assert.ok(r.data, 'Preserve the report for the completeness panel');
    assert.equal(r.data.completeness.status, 'CONTEXT INCOMPLETE');
    assert.equal(r.data.completeness.rules_complete, false);
    assert.ok(r.data.work_context.rendered_context.length > 0);
  } finally {
    await b.stop();
  }
});

test('do not mistake an error envelope for a report', async () => {
  // An envelope containing only code/message is not a report.
  const b = await startBridge({ env: { STUB_MODE: 'stderrjson' } });
  try {
    const r = await (await fetch(`${b.base}/api/status`, { headers: GUARD })).json();
    assert.equal(r.ok, false);
    assert.equal(r.error.code, 'SourceStale');
    assert.ok(!r.data, 'Do not return error metadata as report data');
  } finally {
    await b.stop();
  }
});

// 8. Issue #63: budget limits

test('budgets may reach the AWR limit instead of being capped at 16000', async () => {
  // The CLI limit is 100000, not the old selector cap of 16000
  // or the previous bridge constant of 200000.
  const r = await (
    await fetch(`${bridge.base}/api/context/compile`, {
      method: 'POST',
      headers: Object.assign({ 'content-type': 'application/json' }, GUARD),
      body: JSON.stringify({ work: 'RECON-001', budget: 100000 }),
    })
  ).json();
  assert.ok(r.command.includes('--budget 100000'), `Budget was not forwarded: ${r.command}`);
});

test('reject excessive budgets instead of falling back to the default', async () => {
  const r = await (
    await fetch(`${bridge.base}/api/context/compile`, {
      method: 'POST',
      headers: Object.assign({ 'content-type': 'application/json' }, GUARD),
      body: JSON.stringify({ work: 'RECON-001', budget: 100001 }),
    })
  ).json();
  // Reject invalid budgets with an explicit range instead of omitting --budget.
  assert.equal(r.ok, false);
  assert.equal(r.error.code, 'BadRequest');
  assert.ok(/100000/.test(r.error.message), `The error must state the valid range: ${r.error.message}`);
  assert.ok(!r.command, 'Do not spawn a child process');
});

// 9. Shell-free path handling

test('project paths containing spaces are passed correctly', async () => {
  const tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), 'proj '));
  try {
    const b = await startBridge({ project: tmpDir });
    try {
      const r = await fetch(`${b.base}/api/status`, { headers: GUARD });
      const body = await r.json();
      assert.equal(body.ok, true, `Project paths with spaces must work, got: ${JSON.stringify(body)}`);
    } finally {
      await b.stop();
    }
  } finally {
    fs.rmSync(tmpDir, { recursive: true, force: true });
  }
});

test('search preserves shell metacharacters as one literal argument', async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'awr-argv-'));
  const argvLog = path.join(dir, 'argv.jsonl');
  const text = 'literal & | ^ > < %PATH% "quoted words"';
  let b;
  try {
    b = await startBridge({ env: { STUB_ARGV_OUT: argvLog } });
    const res = await fetch(`${b.base}/api/search?text=${encodeURIComponent(text)}`, {
      headers: GUARD,
    });
    const body = await res.json();
    assert.equal(res.status, 200);
    assert.equal(body.ok, true, JSON.stringify(body));
    assert.equal(body.data.query.text, text);

    const calls = fs.readFileSync(argvLog, 'utf8').trim().split('\n').map(JSON.parse);
    const searches = calls.filter((args) => args.includes('search'));
    assert.equal(searches.length, 1, 'Exactly one search command must run');
    const args = searches[0];
    const separator = args.indexOf('--');
    assert.ok(separator >= 0, 'Search text must follow the option terminator');
    assert.deepEqual(args.slice(separator + 1), [text]);
  } finally {
    if (b) await b.stop();
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test('session list and event history are forwarded with a bounded limit', async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'awr-argv-'));
  const argvLog = path.join(dir, 'argv.jsonl');
  let b;
  try {
    b = await startBridge({ env: { STUB_ARGV_OUT: argvLog } });
    const sessions = await (await fetch(`${b.base}/api/sessions?limit=7`, { headers: GUARD })).json();
    assert.equal(sessions.ok, true, JSON.stringify(sessions));
    assert.equal(sessions.data.sessions[0].agent_id, 'stub-agent');
    assert.match(sessions.command, /session list --active --limit 7/);

    const events = await (await fetch(`${b.base}/api/events?limit=9`, { headers: GUARD })).json();
    assert.equal(events.ok, true, JSON.stringify(events));
    assert.equal(events.data.events[0].type, 'session_started');
    assert.match(events.command, /event history --limit 9/);

    const calls = fs.readFileSync(argvLog, 'utf8').trim().split('\n').map(JSON.parse);
    const sessionArgs = calls.find((args) => args.includes('session') && args.includes('list'));
    const eventArgs = calls.find((args) => args.includes('event') && args.includes('history'));
    assert.ok(sessionArgs.includes('--active'));
    assert.deepEqual(sessionArgs.slice(sessionArgs.indexOf('--limit'), sessionArgs.indexOf('--limit') + 2), ['--limit', '7']);
    assert.deepEqual(eventArgs.slice(eventArgs.indexOf('--limit'), eventArgs.indexOf('--limit') + 2), ['--limit', '9']);

    for (const pathName of ['/api/sessions?limit=0', '/api/sessions?limit=101', '/api/events?limit=1.5']) {
      const invalid = await (await fetch(`${b.base}${pathName}`, { headers: GUARD })).json();
      assert.equal(invalid.ok, false);
      assert.equal(invalid.error.code, 'BadRequest');
      assert.ok(!invalid.command, 'Do not spawn a child process');
    }
  } finally {
    if (b) await b.stop();
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test('session show is forwarded with a validated id', async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'awr-argv-'));
  const argvLog = path.join(dir, 'argv.jsonl');
  let b;
  try {
    b = await startBridge({ env: { STUB_ARGV_OUT: argvLog } });
    const shown = await (await fetch(`${b.base}/api/session?id=11`, { headers: GUARD })).json();
    assert.equal(shown.ok, true, JSON.stringify(shown));
    assert.equal(shown.data.checkpoint.next_action, 'Wire the session show panel');
    assert.match(shown.command, /session show 11/);
    const calls = fs.readFileSync(argvLog, 'utf8').trim().split('\n').map(JSON.parse);
    const showArgs = calls.find((args) => args.includes('session') && args.includes('show'));
    assert.ok(showArgs);
    assert.equal(showArgs[showArgs.indexOf('show') + 1], '11');
  } finally {
    if (b) await b.stop();
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test('session show rejects a missing or invalid id before execution', async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'awr-argv-'));
  const argvLog = path.join(dir, 'argv.jsonl');
  let b;
  try {
    b = await startBridge({ env: { STUB_ARGV_OUT: argvLog } });
    const before = fs.existsSync(argvLog) ? fs.readFileSync(argvLog, 'utf8') : '';
    for (const pathName of ['/api/session', '/api/session?id=', '/api/session?id=bad%20id']) {
      const invalid = await (await fetch(`${b.base}${pathName}`, { headers: GUARD })).json();
      assert.equal(invalid.ok, false);
      assert.equal(invalid.error.code, 'BadRequest');
      assert.ok(!invalid.command, 'Do not spawn a child process');
    }
    const after = fs.existsSync(argvLog) ? fs.readFileSync(argvLog, 'utf8') : '';
    assert.equal(after, before, 'Do not spawn a child process');
  } finally {
    if (b) await b.stop();
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test('execution inspect is forwarded with a validated id and no write flags', async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'awr-argv-'));
  const argvLog = path.join(dir, 'argv.jsonl');
  let b;
  try {
    b = await startBridge({ env: { STUB_ARGV_OUT: argvLog } });
    const shown = await (await fetch(
      `${b.base}/api/execution?id=exe_4411&kill=1&retry=1`,
      { headers: GUARD }
    )).json();
    assert.equal(shown.ok, true, JSON.stringify(shown));
    assert.equal(shown.data.side_effects_performed, false);
    assert.equal(shown.data.observation.state, 'unknown');
    assert.match(shown.command, /execution inspect exe_4411/);
    assert.doesNotMatch(shown.command, /kill|retry/);
    const calls = fs.readFileSync(argvLog, 'utf8').trim().split('\n').map(JSON.parse);
    const args = calls.find((row) => row.includes('execution') && row.includes('inspect'));
    assert.ok(args);
    assert.equal(args[args.indexOf('inspect') + 1], 'exe_4411');
    assert.ok(!args.includes('kill'));
    assert.ok(!args.includes('--kill'));
    assert.ok(!args.includes('retry'));
  } finally {
    if (b) await b.stop();
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test('execution inspect rejects a missing or invalid id before execution', async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'awr-argv-'));
  const argvLog = path.join(dir, 'argv.jsonl');
  let b;
  try {
    b = await startBridge({ env: { STUB_ARGV_OUT: argvLog } });
    const before = fs.existsSync(argvLog) ? fs.readFileSync(argvLog, 'utf8') : '';
    for (const pathName of ['/api/execution', '/api/execution?id=', '/api/execution?id=bad%20id']) {
      const invalid = await (await fetch(`${b.base}${pathName}`, { headers: GUARD })).json();
      assert.equal(invalid.ok, false);
      assert.equal(invalid.error.code, 'BadRequest');
      assert.ok(!invalid.command, 'Do not spawn a child process');
    }
    const after = fs.existsSync(argvLog) ? fs.readFileSync(argvLog, 'utf8') : '';
    assert.equal(after, before, 'Do not spawn a child process');
  } finally {
    if (b) await b.stop();
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test('recovery inspect is forwarded with a validated session', async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'awr-argv-'));
  const argvLog = path.join(dir, 'argv.jsonl');
  let b;
  try {
    b = await startBridge({ env: { STUB_ARGV_OUT: argvLog } });
    const shown = await (await fetch(`${b.base}/api/recovery?session=11`, { headers: GUARD })).json();
    assert.equal(shown.ok, true, JSON.stringify(shown));
    assert.equal(shown.data.side_effects_performed, false);
    assert.equal(shown.data.executions[0].state, 'running');
    assert.match(shown.command, /recovery inspect --session 11/);
    const calls = fs.readFileSync(argvLog, 'utf8').trim().split('\n').map(JSON.parse);
    const args = calls.find((row) => row.includes('recovery') && row.includes('inspect'));
    assert.ok(args);
    assert.deepEqual(args.slice(args.indexOf('--session'), args.indexOf('--session') + 2), ['--session', '11']);
  } finally {
    if (b) await b.stop();
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test('compaction inspect is forwarded with a validated session and no write flags', async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'awr-argv-'));
  const argvLog = path.join(dir, 'argv.jsonl');
  let b;
  try {
    b = await startBridge({ env: { STUB_ARGV_OUT: argvLog } });
    const shown = await (await fetch(
      `${b.base}/api/compaction?session=11&include-observation=1`,
      { headers: GUARD }
    )).json();
    assert.equal(shown.ok, true, JSON.stringify(shown));
    assert.equal(shown.data.read_only, true);
    assert.equal(shown.data.session_switch_performed, false);
    assert.equal(shown.data.state, 'continue');
    assert.equal(shown.data.post_compaction_percent, 42.0);
    assert.equal(shown.data.observation, undefined);
    assert.match(shown.command, /session compaction inspect --session 11/);
    assert.doesNotMatch(shown.command, /include-observation/);
    assert.doesNotMatch(shown.command, /observe|defer/);
    const calls = fs.readFileSync(argvLog, 'utf8').trim().split('\n').map(JSON.parse);
    const args = calls.find((row) => row.includes('compaction') && row.includes('inspect'));
    assert.ok(args);
    assert.deepEqual(args.slice(args.indexOf('--session'), args.indexOf('--session') + 2), ['--session', '11']);
    assert.ok(!args.includes('--include-observation'));
    assert.ok(!args.includes('observe'));
    assert.ok(!args.includes('defer'));
  } finally {
    if (b) await b.stop();
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test('compaction inspect rejects a missing or invalid session before execution', async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'awr-argv-'));
  const argvLog = path.join(dir, 'argv.jsonl');
  let b;
  try {
    b = await startBridge({ env: { STUB_ARGV_OUT: argvLog } });
    const before = fs.existsSync(argvLog) ? fs.readFileSync(argvLog, 'utf8') : '';
    for (const pathName of ['/api/compaction', '/api/compaction?session=', '/api/compaction?session=bad%20id']) {
      const invalid = await (await fetch(`${b.base}${pathName}`, { headers: GUARD })).json();
      assert.equal(invalid.ok, false);
      assert.equal(invalid.error.code, 'BadRequest');
      assert.ok(!invalid.command, 'Do not spawn a child process');
    }
    const after = fs.existsSync(argvLog) ? fs.readFileSync(argvLog, 'utf8') : '';
    assert.equal(after, before, 'Do not spawn a child process');
  } finally {
    if (b) await b.stop();
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test('event show is forwarded with a validated id and a bounded --full read', async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'awr-argv-'));
  const argvLog = path.join(dir, 'argv.jsonl');
  let b;
  try {
    b = await startBridge({ env: { STUB_ARGV_OUT: argvLog } });
    const shown = await (await fetch(`${b.base}/api/event?id=21`, { headers: GUARD })).json();
    assert.equal(shown.ok, true, JSON.stringify(shown));
    assert.equal(shown.data.payload_included, true);
    assert.equal(shown.data.event.event_type, 'session_started');
    assert.match(shown.command, /event show 21 --full --max-bytes 65536/);
    const calls = fs.readFileSync(argvLog, 'utf8').trim().split('\n').map(JSON.parse);
    const args = calls.find((row) => row.includes('event') && row.includes('show'));
    assert.ok(args);
    assert.equal(args[args.indexOf('show') + 1], '21');
    assert.deepEqual(args.slice(args.indexOf('--full'), args.indexOf('--max-bytes') + 2), ['--full', '--max-bytes', '65536']);
  } finally {
    if (b) await b.stop();
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test('event show rejects a missing or invalid id before execution', async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'awr-argv-'));
  const argvLog = path.join(dir, 'argv.jsonl');
  let b;
  try {
    b = await startBridge({ env: { STUB_ARGV_OUT: argvLog } });
    const before = fs.existsSync(argvLog) ? fs.readFileSync(argvLog, 'utf8') : '';
    for (const pathName of ['/api/event', '/api/event?id=', '/api/event?id=bad%20id']) {
      const invalid = await (await fetch(`${b.base}${pathName}`, { headers: GUARD })).json();
      assert.equal(invalid.ok, false);
      assert.equal(invalid.error.code, 'BadRequest');
      assert.ok(!invalid.command, 'Do not spawn a child process');
    }
    const after = fs.existsSync(argvLog) ? fs.readFileSync(argvLog, 'utf8') : '';
    assert.equal(after, before, 'Do not spawn a child process');
  } finally {
    if (b) await b.stop();
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test('recovery inspect rejects a missing or invalid session before execution', async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'awr-argv-'));
  const argvLog = path.join(dir, 'argv.jsonl');
  let b;
  try {
    b = await startBridge({ env: { STUB_ARGV_OUT: argvLog } });
    const before = fs.existsSync(argvLog) ? fs.readFileSync(argvLog, 'utf8') : '';
    for (const pathName of ['/api/recovery', '/api/recovery?session=', '/api/recovery?session=bad%20id']) {
      const invalid = await (await fetch(`${b.base}${pathName}`, { headers: GUARD })).json();
      assert.equal(invalid.ok, false);
      assert.equal(invalid.error.code, 'BadRequest');
      assert.ok(!invalid.command, 'Do not spawn a child process');
    }
    const after = fs.existsSync(argvLog) ? fs.readFileSync(argvLog, 'utf8') : '';
    assert.equal(after, before, 'Do not spawn a child process');
  } finally {
    if (b) await b.stop();
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test('session and event routes default to limit 20', async () => {
  const sessions = await (await fetch(`${bridge.base}/api/sessions`, { headers: GUARD })).json();
  assert.equal(sessions.ok, true, JSON.stringify(sessions));
  assert.match(sessions.command, /session list --active --limit 20/);
  const events = await (await fetch(`${bridge.base}/api/events`, { headers: GUARD })).json();
  assert.equal(events.ok, true, JSON.stringify(events));
  assert.match(events.command, /event history --limit 20/);
});

test('forward pagination to status and reject invalid sizes or offsets before execution', async () => {
  const result = await (await fetch(`${bridge.base}/api/work-page?queue=ready&offset=10&limit=20`, {headers:GUARD})).json();
  assert.match(result.command, /--queue ready --offset 10 --page-size 20/);
  for (const query of ['queue=other', 'offset=-1', 'offset=1.5', 'limit=0', 'limit=101']) {
    const invalid=await (await fetch(`${bridge.base}/api/work-page?${query}`, {headers:GUARD})).json();
    assert.equal(invalid.ok,false); assert.equal(invalid.error.code,'BadRequest');
  }
});
