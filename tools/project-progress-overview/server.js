#!/usr/bin/env node
'use strict';

/** Local CLI bridge for the three Project Progress Overview maps. */
const http = require('node:http');
const fs = require('node:fs');
const path = require('node:path');
const { spawn, execFile } = require('node:child_process');
const { resolveAwr } = require('./awr-process');
const projectMapConfig = require('./project-map-config');

function envInt(name, fallback) {
  const value = Number(process.env[name]);
  return Number.isInteger(value) && value > 0 ? value : fallback;
}

const LIMITS = {
  stdoutBytes: 8 * 1024 * 1024,
  stderrBytes: 1024 * 1024,
  concurrent: envInt('AWR_PROGRESS_OVERVIEW_CONCURRENT', 4),
  readTimeoutMs: envInt('AWR_PROGRESS_OVERVIEW_READ_TIMEOUT_MS', 60000),
};

function parseArgs(argv) {
  const out = { project: process.cwd(), port: 7381, demo: false, open: true, mapConfig: null };
  for (let i = 0; i < argv.length; i++) {
    const argument = argv[i];
    if (argument === '--project' || argument === '-p') out.project = path.resolve(argv[++i] || '.');
    else if (argument === '--port') {
      const port = Number(argv[++i]);
      if (!Number.isInteger(port) || port < 0 || port > 65535) throw new Error('--port must be an integer from 0 to 65535');
      out.port = port;
    } else if (argument === '--demo') out.demo = true;
    else if (argument === '--no-open') out.open = false;
    else if (argument === '--map-config') out.mapConfig = path.resolve(argv[++i] || '.');
    else if (argument === '--help' || argument === '-h') {
      console.log([
        'AWR Project Progress Overview',
        'Usage: node server.js [options]',
        '',
        '  --project <dir>     AWR project directory (default: current directory)',
        '  --port <port>       Listening port (default: 7381; 0 selects a free port)',
        '  --demo              View a synthetic project without running awr',
        '  --map-config <file> Map lanes, dependency panels and thresholds (JSON)',
        '  --no-open           Keep the browser closed',
      ].join('\n'));
      process.exit(0);
    } else throw new Error(`Unknown argument: ${argument}`);
  }
  return out;
}

const ARGS = parseArgs(process.argv.slice(2));
const PUBLIC_DIR = path.join(__dirname, 'public');
const MAP_CONFIG = (() => {
  if (!ARGS.mapConfig) return null;
  try {
    return projectMapConfig.load(ARGS.mapConfig);
  } catch (error) {
    console.error(`--map-config: ${error.message}`);
    process.exit(1);
  }
})();
const AWR_RESOLVED = resolveAwr();
const runtime = {
  mode: ARGS.demo ? 'demo' : 'unknown',
  awrVersion: null,
  project: ARGS.project,
  reason: ARGS.demo ? 'Started with --demo' : null,
  jsonFlagPosition: 'global',
  running: 0,
  bulkSnapshot: false,
};

function detectAwr() {
  return new Promise((resolve) => {
    if (ARGS.demo) return resolve();
    if (!AWR_RESOLVED.exe) {
      runtime.mode = 'demo';
      runtime.reason = 'The awr command was not found. Install it and restart this tool to view your project.';
      return resolve();
    }
    const command = AWR_RESOLVED.needsNode ? process.execPath : AWR_RESOLVED.exe;
    const args = AWR_RESOLVED.needsNode ? [AWR_RESOLVED.entryPoint, '--version'] : ['--version'];
    const env = { ...process.env, AWR_LOCAL_WEB: '0' };
    execFile(command, args, { timeout: 8000, env }, (error, stdout) => {
      runtime.mode = error ? 'demo' : 'live';
      runtime.reason = error ? 'The awr command could not start. Check its installation and restart this tool.' : null;
      runtime.awrVersion = error ? null : String(stdout).trim();
      if (error) return resolve();
      const capabilities = ['capabilities', '--require', 'local.progress_snapshot', '--json'];
      const capabilityArgs = AWR_RESOLVED.needsNode ? [AWR_RESOLVED.entryPoint, ...capabilities] : capabilities;
      execFile(command, capabilityArgs, { timeout: 8000, env }, (failure) => {
        runtime.bulkSnapshot = !failure;
        resolve();
      });
    });
  });
}

const GUARD_HEADER = 'x-awr-project-progress-overview';
function guardRequest(req) {
  const port = server.address() ? server.address().port : ARGS.port;
  const hosts = new Set([`127.0.0.1:${port}`, `localhost:${port}`, `[::1]:${port}`]);
  const host = String(req.headers.host || '').toLowerCase();
  if (!hosts.has(host)) return { code: 'ForbiddenHost', message: 'The request host does not match this local tool.' };
  if (req.headers.origin !== undefined && ![...hosts].some((value) => req.headers.origin === `http://${value}`)) {
    return { code: 'ForbiddenOrigin', message: 'The request origin does not match this local tool.' };
  }
  const site = req.headers['sec-fetch-site'];
  if (site !== undefined && site !== 'same-origin' && site !== 'none') {
    return { code: 'ForbiddenSite', message: 'Cross-site requests are refused.' };
  }
  if (req.method !== 'GET' && req.headers[GUARD_HEADER] !== '1') {
    return { code: 'MissingGuardHeader', message: `Requests require the ${GUARD_HEADER} header.` };
  }
  return null;
}

const COMMANDS = {
  mapSnapshot: ['overview', 'snapshot'],
  mapWorkGraph: ['work', 'graph'],
  mapNav: ['nav', '--cached'],
  mapGoals: ['search', '--type', 'goal', '--limit', '100', '--cached'],
  mapEvents: ['event', 'history'],
  mapSessions: ['session', 'list', '--active', '--limit', '100'],
  mapDoctor: ['doctor'],
  workPrepare: ['work', 'prepare'],
};

function asKey(value) {
  const key = String(value == null ? '' : value);
  return /^[A-Za-z0-9_.:#/-]{1,200}$/.test(key) ? key : null;
}

function buildArgv(commandKey, extra) {
  const args = ['--project', runtime.project];
  if (runtime.jsonFlagPosition === 'global') args.push('--json');
  args.push(...COMMANDS[commandKey], ...extra);
  if (runtime.jsonFlagPosition === 'trailing') args.push('--json');
  return args;
}

function execAwr(argv) {
  return new Promise((resolve) => {
    const command = AWR_RESOLVED.needsNode ? process.execPath : AWR_RESOLVED.exe;
    const args = AWR_RESOLVED.needsNode ? [AWR_RESOLVED.entryPoint, ...argv] : argv;
    const child = spawn(command, args, { stdio: 'pipe', windowsHide: true, env: { ...process.env, AWR_LOCAL_WEB: '0' } });
    runtime.running += 1;
    let released = false;
    const release = () => {
      if (released) return;
      released = true;
      runtime.running -= 1;
    };
    const stdout = [];
    const stderr = [];
    let stdoutBytes = 0;
    let stderrBytes = 0;
    let truncated = false;
    let settled = false;
    const finish = (result) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      resolve({ truncated, ...result });
    };
    const timer = setTimeout(() => {
      child.kill('SIGTERM');
      const hardStop = setTimeout(() => child.kill('SIGKILL'), 5000);
      hardStop.unref();
      finish({ timedOut: true });
    }, LIMITS.readTimeoutMs);
    timer.unref();
    child.stdout.on('data', (chunk) => {
      stdoutBytes += chunk.length;
      if (stdoutBytes > LIMITS.stdoutBytes) {
        truncated = true;
        child.kill('SIGKILL');
      } else stdout.push(chunk);
    });
    child.stderr.on('data', (chunk) => {
      stderrBytes += chunk.length;
      if (stderrBytes > LIMITS.stderrBytes) truncated = true;
      else stderr.push(chunk);
    });
    child.on('error', (error) => {
      release();
      finish({ code: -1, stderr: error.message });
    });
    child.on('close', (code) => {
      release();
      finish({ code, stdout: Buffer.concat(stdout).toString('utf8'), stderr: Buffer.concat(stderr).toString('utf8') });
    });
  });
}

async function runCommand(commandKey, extra) {
  if (runtime.running >= LIMITS.concurrent) {
    return { ok: false, command: null, error: { code: 'BridgeBusy', message: 'The CLI concurrency limit was reached. Retry after an active read finishes.' } };
  }
  let argv = buildArgv(commandKey, extra);
  let result = await execAwr(argv);
  if (result.code !== 0 && runtime.jsonFlagPosition === 'global' && mentionsUnknownJsonFlag(result.stderr, result.stdout)) {
    runtime.jsonFlagPosition = 'trailing';
    argv = buildArgv(commandKey, extra);
    result = await execAwr(argv);
  }
  const command = 'awr ' + argv.map(quoteForDisplay).join(' ');
  if (result.timedOut) return { ok: false, command, error: { code: 'BridgeTimeout', message: 'The read timed out and was terminated. Retry to read the current state.' } };
  if (result.code === -1) return { ok: false, command, error: { code: 'BridgeSpawnFailed', message: result.stderr } };
  if (result.truncated) return { ok: false, command, error: { code: 'OutputTooLarge', message: 'The CLI response exceeded the output limit. Use the displayed command to inspect it directly.' } };
  const parsed = tryParseJson(result.stdout);
  if (result.code !== 0) {
    const errorJson = (parsed && (parsed.code || parsed.error) ? parsed : null) || tryParseJson(result.stderr);
    const error = errorJson && (errorJson.error || errorJson);
    return {
      ok: false, command, exitCode: result.code,
      error: error && error.code ? error : { code: 'CommandFailed', message: (result.stderr || result.stdout || '').trim() },
      data: carriesPayload(parsed) ? parsed : null,
      raw: errorJson || null,
    };
  }
  if (!parsed) return { ok: false, command, error: { code: 'NotJson', message: 'The CLI response could not be read as JSON.' } };
  return { ok: true, command, data: parsed };
}

function carriesPayload(value) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return false;
  const shell = ['code', 'message', 'error', 'details', 'ok'];
  return Object.keys(value).some((k) => shell.indexOf(k) < 0);
}

/** Match only an unrecognized --json flag, not other argument errors. */
function mentionsUnknownJsonFlag(stderr, stdout) {
  const s = (String(stderr) + String(stdout)).toLowerCase();
  if (!s.includes('--json')) return false;
  return (
    s.includes('unexpected argument') || s.includes('unknown') || s.includes('unrecognized')
  );
}

function tryParseJson(text) {
  const t = String(text || '').trim();
  if (!t) return null;
  try {
    return JSON.parse(t);
  } catch (_) {
    // Some commands print human-readable lines before JSON; retry from the first opening brace.
    const i = t.indexOf('{');
    if (i > 0) {
      try {
        return JSON.parse(t.slice(i));
      } catch (_) {
        return null;
      }
    }
    return null;
  }
}

function quoteForDisplay(arg) {
  return /[^A-Za-z0-9_@.:#/=-]/.test(arg) ? `'${String(arg).replace(/'/g, `'\\''`)}'` : arg;
}

const routes = {
  'GET /api/health': async () => ({
    ok: true,
    data: { mode: runtime.mode, awrVersion: runtime.awrVersion, project: runtime.project, reason: runtime.reason, tool: 'Project Progress Overview' },
  }),
  'GET /api/map/config': async () => ({
    ok: true,
    data: { config: MAP_CONFIG ? MAP_CONFIG.config : null, source: MAP_CONFIG ? path.basename(MAP_CONFIG.file) : null,
      ...(runtime.bulkSnapshot ? { bulk_snapshot: true } : {}) },
  }),

  'GET /api/map/snapshot': async () => runCommand('mapSnapshot', []),
  'POST /api/map/snapshot': async () => runCommand('mapSnapshot', ['--refresh']),

  'GET /api/map/work-graph': async (url) => {
    const limit = url.searchParams.get('limit');
    const n = limit == null || limit === '' ? 100 : Number(limit);
    if (!Number.isInteger(n) || n < 1 || n > 1000) {
      return { ok: false, error: { code: 'BadRequest', message: 'limit must be an integer from 1 to 1000' } };
    }
    const extra = ['--limit', String(n)];
    if (url.searchParams.get('cached') === '1') extra.push('--cached');
    return runCommand('mapWorkGraph', extra);
  },

  'GET /api/map/nav': async (url) => {
    const extra = [];
    const milestone = url.searchParams.get('milestone');
    if (milestone != null && milestone !== '') {
      const key = asKey(milestone);
      if (!key) return { ok: false, error: { code: 'BadRequest', message: 'milestone must be a valid key' } };
      extra.push('--milestone', key);
    }
    return runCommand('mapNav', extra);
  },

  'GET /api/map/goals': async () => runCommand('mapGoals', []),

  'GET /api/map/events': async (url) => {
    const limit = url.searchParams.get('limit');
    const n = limit == null || limit === '' ? 1000 : Number(limit);
    if (!Number.isInteger(n) || n < 1 || n > 1000) {
      return { ok: false, error: { code: 'BadRequest', message: 'limit must be an integer from 1 to 1000' } };
    }
    const extra = ['--limit', String(n)];
    const through = url.searchParams.get('through');
    if (through != null && through !== '') {
      const r = Number(through);
      if (!Number.isSafeInteger(r) || r < 0) return { ok: false, error: { code: 'BadRequest', message: 'through must be a non-negative integer' } };
      extra.push('--through-revision', String(r));
    }
    const cursor = url.searchParams.get('cursor');
    if (cursor != null && cursor !== '') {
      const parsed = projectMapConfig.parseEventCursor(cursor);
      if (!parsed) return { ok: false, error: { code: 'BadRequest', message: 'cursor must be the next_cursor object of the previous page' } };
      extra.push('--cursor', JSON.stringify(parsed));
    }
    return runCommand('mapEvents', extra);
  },

  'GET /api/map/sessions': async () => runCommand('mapSessions', []),

  'GET /api/map/doctor': async () => runCommand('mapDoctor', []),

  'GET /api/map/prepare': async (url) => {
    const key = asKey(url.searchParams.get('key'));
    if (!key) return { ok: false, error: { code: 'BadRequest', message: 'key must be a valid work key' } };
    return runCommand('workPrepare', [key, '--response-view', 'action']);
  },

};

const MIME = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
  '.json': 'application/json; charset=utf-8',
  '.svg': 'image/svg+xml',
  '.ico': 'image/x-icon',
};

// Load only local assets: no external fonts, inline scripts, or external connections.
const CSP = [
  "default-src 'self'",
  "script-src 'self'",
  "style-src 'self'",
  "connect-src 'self'",
  "font-src 'self'",
  "img-src 'self' data:",
  "object-src 'none'",
  "base-uri 'none'",
  "form-action 'self'",
  "frame-ancestors 'none'",
].join('; ');

function sendJson(res, status, payload) {
  const headers = {
    'content-type': 'application/json; charset=utf-8',
    'cache-control': 'no-store',
    'x-content-type-options': 'nosniff',
  };
  const cookie = res.getHeader('set-cookie');
  if (cookie) headers['set-cookie'] = cookie;
  res.writeHead(status, headers);
  res.end(JSON.stringify(payload));
}

function serveStatic(req, res, pathname) {
  const rel = pathname === '/' ? 'index.html' : pathname.replace(/^\/+/, '');
  const file = path.join(PUBLIC_DIR, rel);
  // Prevent traversal: the resolved path must stay inside public.
  if (!file.startsWith(PUBLIC_DIR + path.sep) && file !== path.join(PUBLIC_DIR, 'index.html')) {
    res.writeHead(403, { 'content-type': 'text/plain; charset=utf-8' });
    res.end('forbidden');
    return;
  }
  fs.readFile(file, (err, buf) => {
    if (err) {
      res.writeHead(404, { 'content-type': 'text/plain; charset=utf-8' });
      res.end('404');
      return;
    }
    res.writeHead(200, {
      'content-type': MIME[path.extname(file)] || 'application/octet-stream',
      // Disable caching so edits under public appear on refresh.
      'cache-control': 'no-store',
      'content-security-policy': CSP,
      'x-content-type-options': 'nosniff',
      'referrer-policy': 'no-referrer',
    });
    res.end(buf);
  });
}


const server = http.createServer((req, res) => {
  handleRequest(req, res).catch((error) => {
    if (!res.headersSent) sendJson(res, 500, { ok: false, error: { code: 'BridgeError', message: error.message } });
  });
});

async function handleRequest(req, res) {
  let url;
  try {
    url = new URL(req.url, 'http://127.0.0.1');
  } catch (_) {
    return sendJson(res, 400, { ok: false, error: { code: 'BadRequestTarget', message: 'The request target could not be parsed.' } });
  }
  if (!url.pathname.startsWith('/api/')) return serveStatic(req, res, url.pathname);
  const denial = guardRequest(req);
  if (denial) return sendJson(res, 403, { ok: false, error: denial });
  const handler = routes[`${req.method} ${url.pathname}`];
  if (!handler) return sendJson(res, 404, { ok: false, error: { code: 'NoRoute', message: `${req.method} ${url.pathname}` } });
  if (runtime.mode === 'demo' && url.pathname !== '/api/health') {
    return sendJson(res, 200, { ok: false, error: { code: 'DemoMode', message: runtime.reason } });
  }
  return sendJson(res, 200, await handler(url));
}

async function start() {
  await detectAwr();
  return new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(ARGS.port, '127.0.0.1', () => resolve(server));
  });
}

if (require.main === module) {
  start().then(() => {
    const address = `http://127.0.0.1:${server.address().port}`;
    console.log(`AWR Project Progress Overview started\n${address}\nProject ${runtime.project}\nMode ${runtime.mode}`);
    if (runtime.reason) console.log(runtime.reason);
    if (ARGS.open) {
      const opener = process.platform === 'darwin' ? 'open' : process.platform === 'win32' ? 'explorer' : 'xdg-open';
      spawn(opener, [address], { stdio: 'ignore', detached: true }).on('error', () => {});
    }
  }, (error) => {
    console.error(error.message);
    process.exitCode = 1;
  });
}

module.exports = { server, start, runtime, LIMITS, GUARD_HEADER, ARGS };
