#!/usr/bin/env node
/**
 * AWR Inspector: local bridge process
 *
 * Translate each HTTP request into one `awr --json` command and forward
 * the returned JSON unchanged. Do not interpret, rewrite, or cache it.
 *
 * Loopback binding alone is insufficient: any browser page can request 127.0.0.1.
 * The /api/* routes also enforce a request-origin boundary; see guardRequest().
 *
 * Usage:
 *   node server.js --project /abs/path/to/project [--port 7381] [--demo] [--allow-reindex]
 */

'use strict';

const http = require('http');
const fs = require('fs');
const path = require('path');
const { spawn, execFile } = require('child_process');

// Resource limits

/** Numeric overrides for tests; use defaults when unset. */
function envInt(name, fallback) {
  const v = Number(process.env[name]);
  return Number.isInteger(v) && v > 0 ? v : fallback;
}

const LIMITS = {
  stdoutBytes: 8 * 1024 * 1024,   // Maximum stdout bytes per command.
  stderrBytes: 1 * 1024 * 1024,
  requestBytes: 64 * 1024,        // Maximum request body size.
  concurrent: envInt('AWR_INSPECTOR_CONCURRENT', 4),          // Maximum concurrent awr child processes.
  readTimeoutMs: envInt('AWR_INSPECTOR_READ_TIMEOUT_MS', 60000),   // Read-only command timeout.
  writeTimeoutMs: envInt('AWR_INSPECTOR_WRITE_TIMEOUT_MS', 120000), // Write-command (reindex) timeout.
};

// Arguments

function parseArgs(argv) {
  const out = {
    project: process.cwd(),
    port: 7381,
    demo: false,
    open: true,
    allowReindex: false,
    teamUrl: null,
    teamPublicUrl: null,
    teamOnly: false,
    teamFixtureDir: null,
    mapConfig: null,
  };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === '--project' || a === '-p') out.project = path.resolve(argv[++i] || '.');
    else if (a === '--port') out.port = Number(argv[++i]) || out.port;
    else if (a === '--demo') out.demo = true;
    else if (a === '--no-open') out.open = false;
    else if (a === '--allow-reindex') out.allowReindex = true;
    else if (a === '--team-url') out.teamUrl = argv[++i] || null;
    else if (a === '--team-public-url') out.teamPublicUrl = argv[++i] || null;
    else if (a === '--team-only') out.teamOnly = true;
    else if (a === '--team-fixture-dir') out.teamFixtureDir = path.resolve(argv[++i] || '.');
    else if (a === '--map-config') out.mapConfig = path.resolve(argv[++i] || '.');
    else if (a === '--help' || a === '-h') {
      console.log([
        'Usage: node server.js [options]',
        '',
        '  --project <dir>    AWR project to inspect (default: current directory)',
        '  --port <port>      Listening port (default: 7381)',
        '  --demo             Use demo mode without running real commands',
        '  --allow-reindex    Enable source reindex from the UI (disabled by default)',
        '  --team-url <url>   Proxy Team Web to awr-server /v1/web entry (WS-044)',
        '  --team-public-url <url> Public Team service base URL for member MCP setup',
        '  --team-only        Serve only Team APIs and hide local Inspector navigation',
        '  --team-fixture-dir Use on-disk team-web-loop fixtures (demo/tests)',
        '  --map-config <file> Display configuration (JSON) for the Project map page: lanes, panels, thresholds',
        '  --no-open          Do not open the browser automatically',
      ].join('\n'));
      process.exit(0);
    }
  }
  return out;
}

const ARGS = parseArgs(process.argv.slice(2));
if (ARGS.teamOnly && (!ARGS.teamUrl || ARGS.demo)) {
  console.error('--team-only requires --team-url and cannot be combined with --demo');
  process.exit(1);
}
const PUBLIC_DIR = path.join(__dirname, 'public');
const projectMapConfig = require('./project-map-config');
const MAP_CONFIG = (() => {
  if (!ARGS.mapConfig) return null;
  try {
    return projectMapConfig.load(ARGS.mapConfig);
  } catch (e) {
    console.error(`--map-config: ${e.message}`);
    process.exit(1);
  }
})();
const { createTeamBridge } = require('./team-bridge');
const teamBridge = createTeamBridge({ teamUrl: ARGS.teamUrl, teamPublicUrl: ARGS.teamPublicUrl, teamFixtureDir: ARGS.teamFixtureDir, port: ARGS.port });

// awr executable resolution

/**
 * Extract the Node.js entry point from a .cmd wrapper.
 * npm wrapper format:`"%_prog%" "<entry_point>" %*`
 * Test wrapper format:`"node" "<entry_point>" %*`
 * Return the path after expanding %dp0%, or null when unavailable.
 */
function parseCmdEntryPoint(cmdPath) {
  try {
    const content = fs.readFileSync(cmdPath, 'utf8');
    const match = content.match(/"[^"]+"\s+"([^"]+)"\s+%\*/);
    if (match) {
      let entryPoint = match[1].replace(/%dp0%/g, path.dirname(cmdPath));
      if (fs.existsSync(entryPoint)) return entryPoint;
    }
  } catch {}
  return null;
}

/**
 * Resolve awr through PATH and PATHEXT without a shell.
 *
 * Return { exe, needsNode, entryPoint? }. Native executables and POSIX scripts
 * run directly; Node wrappers run entryPoint through process.execPath.
 * If resolution fails, exe is null and the caller enters demo mode.
 */
function resolveAwr() {
  const sep = process.platform === 'win32' ? ';' : ':';
  const exts = process.platform === 'win32'
    ? (process.env.PATHEXT || '.COM;.EXE;.BAT;.CMD').split(';').map(e => e.toUpperCase())
    : [''];
  const dirs = (process.env.PATH || '').split(sep);

  for (const dir of dirs) {
    for (const ext of exts) {
      const candidate = path.join(dir, 'awr' + ext);
      try {
        const st = fs.statSync(candidate, { throwIfNoEntry: false });
        if (st && st.isFile()) {
          const extUpper = path.extname(candidate).toUpperCase();
          // Spawn native executables and extensionless POSIX scripts directly.
          if (extUpper === '.EXE' || extUpper === '') {
            return { exe: candidate, needsNode: false };
          }
          // Resolve the Node entry point from npm command wrappers.
          if (extUpper === '.CMD' || extUpper === '.BAT') {
            const entryPoint = parseCmdEntryPoint(candidate);
            if (entryPoint) return { exe: candidate, needsNode: true, entryPoint };
          }
        }
      } catch {}
    }
  }
  return { exe: null, needsNode: false };
}

const AWR_RESOLVED = resolveAwr();

// awr detection

const runtime = {
  mode: ARGS.demo ? 'demo' : 'unknown', // 'live' | 'demo'
  awrVersion: null,
  project: ARGS.project,
  reason: ARGS.demo ? 'Started with --demo' : null,
  allowReindex: ARGS.allowReindex,
  // Place --json globally. Retry at the end only if AWR explicitly rejects that flag;
  // other argument errors must not change process-wide behavior.
  jsonFlagPosition: 'global',
  running: 0,
};

function detectAwr() {
  return new Promise((resolve) => {
    if (ARGS.demo) return resolve();
    if (!AWR_RESOLVED.exe) {
      runtime.mode = 'demo';
      runtime.reason = 'The awr command was not found. Install it and restart this process to view live data.';
      return resolve();
    }
    const args = AWR_RESOLVED.needsNode ? [AWR_RESOLVED.entryPoint, '--version'] : ['--version'];
    const cmd = AWR_RESOLVED.needsNode ? process.execPath : AWR_RESOLVED.exe;
    execFile(cmd, args, { timeout: 8000 }, (err, stdout) => {
      if (err) {
        runtime.mode = 'demo';
        runtime.reason = 'The awr command was not found. Install it and restart this process to view live data.';
        return resolve();
      }
      runtime.awrVersion = String(stdout).trim();
      runtime.mode = 'live';
      resolve();
    });
  });
}

// Request-origin boundary

const ALLOWED_HOSTS = new Set([
  `127.0.0.1:${ARGS.port}`,
  `localhost:${ARGS.port}`,
  `[::1]:${ARGS.port}`,
]);
const ALLOWED_ORIGINS = new Set([
  `http://127.0.0.1:${ARGS.port}`,
  `http://localhost:${ARGS.port}`,
  `http://[::1]:${ARGS.port}`,
]);

/** Mutations require this custom header; third-party pages need a CORS preflight, which we reject. */
const GUARD_HEADER = 'x-awr-inspector';

/**
 * Check whether an /api/* request comes from this local page.
 * Return null to allow it, or the error to send to the caller.
 */
function guardRequest(req) {
  // 1) Host blocks DNS rebinding: the attacker-controlled hostname remains in the header.
  const host = String(req.headers.host || '').toLowerCase();
  if (!ALLOWED_HOSTS.has(host)) {
    return { code: 'ForbiddenHost', message: `Rejected Host: ${host || '(empty)'}` };
  }

  // 2) Origin, when supplied, must match this server. Reject null origins from sandboxed/file pages.
  const origin = req.headers.origin;
  if (origin !== undefined && !ALLOWED_ORIGINS.has(String(origin))) {
    return { code: 'ForbiddenOrigin', message: `Rejected Origin: ${origin}` };
  }

  // 3) Sec-Fetch-Site is supplied by the browser and cannot be changed by page scripts.
  //    Same-origin fetches use same-origin; address-bar navigation uses none. Reject the rest.
  const site = req.headers['sec-fetch-site'];
  if (site !== undefined && site !== 'same-origin' && site !== 'none') {
    return { code: 'ForbiddenSite', message: `Rejected Sec-Fetch-Site: ${site}` };
  }

  // 4) Mutations require a custom header that cross-site form POSTs cannot send.
  if (req.method !== 'GET' && req.headers[GUARD_HEADER] !== '1') {
    return {
      code: 'MissingGuardHeader',
      message: `Mutation requests require the ${GUARD_HEADER}: 1 header`,
    };
  }

  return null;
}

// Execute awr

// Allowlist: frontend action names map to fixed subcommands.
// The frontend can select only these commands and append validated arguments.
const COMMANDS = {
  status: { argv: ['status'], write: false },
  ready: { argv: ['ready'], write: false },
  workShow: { argv: ['work', 'show'], write: false },
  mainlineNav: { argv: ['nav'], write: false },
  search: { argv: ['search'], write: false },
  intakeInspect: { argv: ['intake', 'inspect'], write: false },
  contextCompile: { argv: ['context', 'compile'], write: false },
  sourceReindex: { argv: ['source', 'reindex'], write: true },
  sessionList: { argv: ['session', 'list', '--active'], write: false },
  eventHistory: { argv: ['event', 'history'], write: false },
  // Project map: the official read-only commands behind the snapshot (see public/project-map/extract.js).
  mapWorkGraph: { argv: ['work', 'graph'], write: false },
  mapNav: { argv: ['nav', '--cached'], write: false },
  mapGoals: { argv: ['search', '--type', 'goal', '--limit', '100', '--cached'], write: false },
  mapEvents: { argv: ['event', 'history'], write: false },
  mapSessions: { argv: ['session', 'list', '--active', '--limit', '100'], write: false },
  mapDoctor: { argv: ['doctor'], write: false },
};

// Validate each field according to its semantics instead of one broad ASCII expression.
// Argument arrays without a shell prevent injection; these checks validate values.

/** AWR keys such as EXAMPLE-001, goal#demo, and plan#intake. */
const KEY_RE = /^[A-Za-z0-9_.:#/-]{1,200}$/;

/** Branch names. */
const BRANCH_RE = /^[A-Za-z0-9_./-]{1,200}$/;

function asKey(value) {
  const s = String(value == null ? '' : value);
  return KEY_RE.test(s) ? s : null;
}

function asBranch(value) {
  const s = String(value == null ? '' : value);
  return BRANCH_RE.test(s) ? s : null;
}

/** Bounded page size shared with ready / work-page. Missing values use the default. */
function asLimit(value, fallback) {
  if (value == null || value === '') return fallback;
  const n = Number(value);
  if (!Number.isInteger(n) || n < 1 || n > 100) return null;
  return n;
}

/**
 * Free text for search and intent. Unicode, including Chinese search, is supported.
 * Reject control characters and NUL, and bound the length.
 */
function asText(value, maxLength) {
  const s = String(value == null ? '' : value);
  if (!s || s.length > (maxLength || 500)) return null;
  for (let i = 0; i < s.length; i++) {
    const c = s.charCodeAt(i);
    // Reject control characters, including NUL, tabs, newlines, and carriage returns.
    if (c < 0x20 || c === 0x7f) return null;
  }
  return s;
}

function buildArgv(commandKey, extra) {
  const spec = COMMANDS[commandKey];
  if (!spec) throw new Error(`Command not allowed: ${commandKey}`);
  const base = ['--project', runtime.project];
  if (runtime.jsonFlagPosition === 'global') base.push('--json');
  const argv = base.concat(spec.argv, extra || []);
  if (runtime.jsonFlagPosition === 'trailing') argv.push('--json');
  return argv;
}

/**
 * Run one awr command.
 *
 * Collect stdout/stderr as Buffers and decode once; decoding each chunk can split
 * a multibyte UTF-8 character and introduce U+FFFD.
 *
 * Read-only commands may be killed on timeout. source reindex is the only write;
 * stop waiting without killing it, because its outcome may already be durable.
 */
function execAwr(argv, opts) {
  const write = Boolean(opts && opts.write);
  const timeoutMs = write ? LIMITS.writeTimeoutMs : LIMITS.readTimeoutMs;

  return new Promise((resolve) => {
    const cmd = AWR_RESOLVED.needsNode ? process.execPath : AWR_RESOLVED.exe;
    const args = AWR_RESOLVED.needsNode ? [AWR_RESOLVED.entryPoint, ...argv] : argv;
    const child = spawn(cmd, args, { stdio: 'pipe', windowsHide: true });

    // Concurrency slots follow child lifetimes, not HTTP responses. A timed-out child
    // must retain its slot until it exits, or the concurrency limit is ineffective.
    runtime.running += 1;
    let released = false;
    const release = () => {
      if (released) return;
      released = true;
      runtime.running -= 1;
    };

    const out = [];
    const err = [];
    let outBytes = 0;
    let errBytes = 0;
    let truncated = false;
    let settled = false;

    const finish = (result) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      resolve(Object.assign({ truncated, write }, result));
    };

    const timer = setTimeout(() => {
      if (write) {
        // Let the write finish and report its outcome as unknown.
        // Release the slot on close, not when returning the timeout response.
        finish({ code: null, timedOut: true, outcomeUnknown: true, stdout: '', stderr: '' });
      } else {
        // For read-only commands, send SIGTERM, then SIGKILL after five seconds.
        child.kill('SIGTERM');
        const hard = setTimeout(() => child.kill('SIGKILL'), 5000);
        hard.unref();
        finish({ code: null, timedOut: true, outcomeUnknown: false, stdout: '', stderr: '' });
      }
    }, timeoutMs);
    // The timeout must not keep the process alive.
    timer.unref();

    child.stdout.on('data', (chunk) => {
      outBytes += chunk.length;
      if (outBytes > LIMITS.stdoutBytes) {
        truncated = true;
        // Read-only commands may be killed, but interrupting a reindex would leave
        // an unknown write outcome. Excess output is not a reason to kill a writer.
        // Keep draining the pipe while discarding bytes beyond the limit.
        if (!write) child.kill('SIGKILL');
        return;
      }
      out.push(chunk);
    });
    child.stderr.on('data', (chunk) => {
      errBytes += chunk.length;
      if (errBytes > LIMITS.stderrBytes) {
        truncated = true;
        return;
      }
      err.push(chunk);
    });

    child.on('error', (e) => {
      release();
      finish({ code: -1, stdout: '', stderr: String(e.message) });
    });
    child.on('close', (code) => {
      release();
      finish({
        code,
        stdout: Buffer.concat(out).toString('utf8'),
        stderr: Buffer.concat(err).toString('utf8'),
      });
    });
  });
}

/**
 * Run a command and return the frontend response envelope.
 * Always include command, even on failure, so users can copy it into a terminal.
 */
async function runCommand(commandKey, extra) {
  const spec = COMMANDS[commandKey];

  // execAwr owns slots for each child lifetime; this check only controls admission.
  if (runtime.running >= LIMITS.concurrent) {
    return {
      ok: false,
      command: null,
      error: {
        code: 'BridgeBusy',
        message: 'The awr concurrency limit has been reached. Wait for a child process to exit before retrying.',
      },
    };
  }

  {
    let argv = buildArgv(commandKey, extra);
    let result = await execAwr(argv, spec);

    // Retry --json at another position only when AWR explicitly rejects the flag.
    if (
      result.code !== 0 &&
      runtime.jsonFlagPosition === 'global' &&
      mentionsUnknownJsonFlag(result.stderr, result.stdout)
    ) {
      runtime.jsonFlagPosition = 'trailing';
      argv = buildArgv(commandKey, extra);
      result = await execAwr(argv, spec);
    }

    const command = 'awr ' + argv.map(quoteForDisplay).join(' ');

    if (result.timedOut) {
      return {
        ok: false,
        command,
        error: result.outcomeUnknown
          ? {
              code: 'OutcomeUnknown',
              message:
                'The command timed out but was not terminated. It may have taken effect. ' +
                'Inspect the current state with awr before proceeding. Do not retry blindly.',
            }
          : {
              code: 'BridgeTimeout',
              message: 'The read-only command timed out and was terminated. It is safe to retry.',
            },
      };
    }

    if (result.code === -1) {
      return { ok: false, command, error: { code: 'BridgeSpawnFailed', message: result.stderr } };
    }

    if (result.truncated) {
      // The write was not killed, but its output is incomplete. Do not recommend a blind retry.
      return spec.write
        ? {
            ok: false,
            command,
            error: {
              code: 'OutcomeUnknown',
              message:
                `Output exceeded the ${LIMITS.stdoutBytes}-byte limit and is incomplete. The command was not terminated. ` +
                'It may have taken effect. Inspect the current state with awr before proceeding. Do not retry blindly.',
            },
          }
        : {
            ok: false,
            command,
            error: {
              code: 'OutputTooLarge',
              message: `awr output exceeded the ${LIMITS.stdoutBytes}-byte limit. Run this command directly in a terminal.`,
            },
          };
    }

    const parsed = tryParseJson(result.stdout);

    if (result.code !== 0) {
      // AWR errors are JSON with code/message and may arrive on stdout or stderr.
      const errJson =
        (parsed && (parsed.code || parsed.error) ? parsed : null) || tryParseJson(result.stderr);
      const domain = errJson && (errJson.error || errJson);

      // A nonzero exit can still include a usable report. Preserve context
      // completeness, issues and evidence gaps for the interface to render.
      const payload = carriesPayload(parsed) ? parsed : null;

      return {
        ok: false,
        command,
        exitCode: result.code,
        error: domain && domain.code
          ? domain
          : { code: 'CommandFailed', message: (result.stderr || result.stdout || '').trim() },
        data: payload,
        raw: errJson || null,
      };
    }

    if (!parsed) {
      return {
        ok: false,
        command,
        error: { code: 'NotJson', message: 'awr returned non-JSON output. See raw for the original output.' },
        raw: result.stdout.slice(0, 20000),
      };
    }

    return { ok: true, command, data: parsed };
  }
}

/**
 * Distinguish result payloads from envelopes containing only error metadata.
 */
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

// Routes

const routes = {
  'GET /api/health': async () => ({
    ok: true,
    data: {
      mode: ARGS.teamOnly ? 'team' : runtime.mode,
      teamOnly: ARGS.teamOnly,
      awrVersion: runtime.awrVersion,
      project: ARGS.teamOnly ? null : runtime.project,
      reason: runtime.reason,
      allowReindex: runtime.allowReindex,
      bridgeVersion: '1.1.0',
    },
  }),

  'GET /api/status': async (url) => {
    const extra = [];
    const view = asKey(url.searchParams.get('view'));
    if (view === 'full' || view === 'summary') extra.push('--view', view);
    return runCommand('status', extra);
  },


  'GET /api/mainline-nav': async (url) => {
    const extra = [];
    const workstream = url.searchParams.get('workstream');
    const goal = url.searchParams.get('goal');
    const milestone = url.searchParams.get('milestone');
    const works = url.searchParams.getAll('work');
    if (workstream) extra.push('--workstream', workstream);
    if (goal) extra.push('--goal', goal);
    if (milestone) extra.push('--milestone', milestone);
    for (const work of works) extra.push('--work', work);
    return runCommand('mainlineNav', extra);
  },
  'GET /api/work-page': async (url) => {
    const queue = url.searchParams.get('queue') || 'all';
    const offset = Number(url.searchParams.get('offset') || '0');
    const limit = Number(url.searchParams.get('limit') || '10');
    if (!['all', 'current', 'ready', 'waiting', 'blocked'].includes(queue) ||
        !Number.isSafeInteger(offset) || offset < 0 ||
        !Number.isInteger(limit) || limit < 1 || limit > 100) {
      return { ok: false, error: { code: 'BadRequest', message: 'Invalid pagination parameters' } };
    }
    return runCommand('status', ['--queue', queue, '--offset', String(offset), '--page-size', String(limit)]);
  },

  'GET /api/ready': async (url) => {
    const extra = [];
    const limit = Number(url.searchParams.get('limit'));
    if (Number.isInteger(limit) && limit >= 1 && limit <= 100) extra.push('--limit', String(limit));
    return runCommand('ready', extra);
  },

  'GET /api/work': async (url) => {
    const key = asKey(url.searchParams.get('key'));
    if (!key) return { ok: false, error: { code: 'BadRequest', message: 'A valid key parameter is required' } };
    return runCommand('workShow', [key]);
  },

  'GET /api/search': async (url) => {
    // AWR 0.4.0 uses `awr search [OPTIONS] [TEXT]`: search text is positional, not --text.
    const text = asText(url.searchParams.get('text'), 200);
    if (!text) {
      return { ok: false, error: { code: 'BadRequest', message: 'A valid text parameter is required' } };
    }
    const extra = [];
    const limit = Number(url.searchParams.get('limit'));
    if (Number.isInteger(limit) && limit >= 1 && limit <= 100) extra.push('--limit', String(limit));
    // Use -- before positional text so a leading hyphen is not interpreted as an option.
    extra.push('--', text);
    return runCommand('search', extra);
  },

  'GET /api/sources': async () => runCommand('intakeInspect', []),

  'GET /api/sessions': async (url) => {
    const limit = asLimit(url.searchParams.get('limit'), 20);
    if (limit == null) {
      return { ok: false, error: { code: 'BadRequest', message: 'limit must be an integer from 1 to 100' } };
    }
    return runCommand('sessionList', ['--limit', String(limit)]);
  },

  'GET /api/events': async (url) => {
    const limit = asLimit(url.searchParams.get('limit'), 20);
    if (limit == null) {
      return { ok: false, error: { code: 'BadRequest', message: 'limit must be an integer from 1 to 100' } };
    }
    return runCommand('eventHistory', ['--limit', String(limit)]);
  },

  // Project map data. Each route is one allowlisted read-only command with validated arguments.
  'GET /api/map/config': async () => ({
    ok: true,
    data: { config: MAP_CONFIG ? MAP_CONFIG.config : null, source: MAP_CONFIG ? path.basename(MAP_CONFIG.file) : null },
  }),

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

  'POST /api/context/compile': async (_url, body) => {
    const extra = [];
    const work = asKey(body.work);
    if (!work) return { ok: false, error: { code: 'BadRequest', message: 'A valid work key is required' } };
    extra.push('--work', work);

    const goal = asKey(body.goal);
    if (goal) extra.push('--goal', goal);

    // Match the AWR limit and reject invalid budgets explicitly instead of
    // silently falling back to the CLI default.
    if (body.budget !== undefined && body.budget !== null && body.budget !== '') {
      const budget = Number(body.budget);
      if (!Number.isInteger(budget) || budget < 500 || budget > 100000) {
        return {
          ok: false,
          error: { code: 'BadRequest', message: 'budget must be an integer from 500 to 100000' },
        };
      }
      extra.push('--budget', String(budget));
    }

    const branch = asBranch(body.branch);
    if (branch) extra.push('--branch', branch);

    const intent = asText(body.intent, 500);
    if (intent) extra.push('--intent', intent);

    return runCommand('contextCompile', extra);
  },

  'POST /api/source/reindex': async () => {
    if (!runtime.allowReindex) {
      return {
        ok: false,
        error: {
          code: 'ReindexNotAllowed',
          message: 'Reindexing is disabled by default. Restart with --allow-reindex to enable it.',
        },
      };
    }
    return runCommand('sourceReindex', []);
  },
};
Object.assign(routes, teamBridge.routes);


// ───────────────────────── HTTP ─────────────────────────

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

/** Collect request bodies as Buffers and reject overflow; string concatenation can split multibyte characters. */
function readBody(req) {
  return new Promise((resolve) => {
    const chunks = [];
    let bytes = 0;
    let killed = false;
    req.on('data', (chunk) => {
      if (killed) return;
      bytes += chunk.length;
      if (bytes > LIMITS.requestBytes) {
        killed = true;
        // Do not destroy the connection before sending 413; discard the remaining data.
        req.resume();
        resolve({ tooLarge: true });
        return;
      }
      chunks.push(chunk);
    });
    req.on('end', () => {
      if (killed) return;
      const raw = Buffer.concat(chunks).toString('utf8');
      try {
        resolve({ body: raw ? JSON.parse(raw) : {} });
      } catch (_) {
        resolve({ body: {} });
      }
    });
    req.on('error', () => {
      if (!killed) resolve({ body: {} });
    });
  });
}

/**
 * Wrap every request in this handler.
 *
 * new URL() throws for malformed targets such as GET // HTTP/1.1. This callback
 * is async, so an uncaught error would reject a Promise and terminate the process.
 * One malformed request must not bring down the server.
 */
const server = http.createServer((req, res) => {
  handleRequest(req, res).catch((err) => {
    try {
      sendJson(res, 500, {
        ok: false,
        error: { code: 'BridgeError', message: String((err && err.message) || err) },
      });
    } catch (_) {
      // The response has already started; abandon this request while keeping the server alive.
    }
  });
});

async function handleRequest(req, res) {
  let url;
  try {
    url = new URL(req.url, 'http://127.0.0.1');
  } catch (_) {
    return sendJson(res, 400, {
      ok: false,
      error: { code: 'BadRequestTarget', message: 'The request target could not be parsed.' },
    });
  }

  if (url.pathname.startsWith('/api/')) {
    const denial = guardRequest(req);
    if (denial) return sendJson(res, 403, { ok: false, error: denial });
    if (ARGS.teamOnly && url.pathname !== '/api/health' && !url.pathname.startsWith('/api/team/')) {
      return sendJson(res, 403, { ok: false, error: {
        code: 'TeamOnlyDeployment', message: 'This entry serves Team projects; local filesystem APIs are unavailable.',
      } });
    }

    const handler = routes[`${req.method} ${url.pathname}`];
    if (!handler) {
      return sendJson(res, 404, {
        ok: false,
        error: { code: 'NoRoute', message: `${req.method} ${url.pathname}` },
      });
    }

    // In demo mode, only /api/health and Team Web fixture routes run; other live CLI routes stay blocked.
    if (
      runtime.mode === 'demo' &&
      url.pathname !== '/api/health' &&
      !url.pathname.startsWith('/api/team/')
    ) {
      return sendJson(res, 200, {
        ok: false,
        error: { code: 'DemoMode', message: runtime.reason || 'Demo mode is active; no live project is connected.' },
      });
    }

    try {
      let body = null;
      if (req.method === 'POST') {
        const read = await readBody(req);
        if (read.tooLarge) {
          return sendJson(res, 413, {
            ok: false,
            error: { code: 'BodyTooLarge', message: `Request body exceeds ${LIMITS.requestBytes} bytes.` },
          });
        }
        body = read.body;
      }
      return sendJson(res, 200, await handler(url, body, req, res));
    } catch (err) {
      return sendJson(res, 200, {
        ok: false,
        error: { code: 'BridgeError', message: String(err.message) },
      });
    }
  }

  serveStatic(req, res, url.pathname);
}

// Startup

function start() {
  return detectAwr().then(
    () =>
      new Promise((resolve, reject) => {
        server.once('error', reject);
        server.listen(ARGS.port, '127.0.0.1', () => resolve(server));
      })
  );
}

if (require.main === module) {
  start().then(
    () => {
      const addr = `http://127.0.0.1:${ARGS.port}`;
      console.log('');
      console.log('  AWR Inspector started');
      console.log('  ─────────────────────────────────────────');
      console.log(`  URL     ${addr}`);
      console.log(`  Project ${runtime.project}`);
      if (runtime.mode === 'live') {
        console.log(`  Mode    Live data (${runtime.awrVersion || 'awr'})`);
      } else {
        console.log('  Mode    Demo');
        console.log(`  Reason  ${runtime.reason}`);
      }
      console.log(`  Reindex ${runtime.allowReindex ? 'enabled' : 'disabled (enable with --allow-reindex)'}`);
      console.log('  ─────────────────────────────────────────');
      console.log('  Press Ctrl+C to stop');
      console.log('');
      if (ARGS.open) {
        const opener =
          process.platform === 'darwin' ? 'open'
            : process.platform === 'win32' ? 'explorer' : 'xdg-open';
        spawn(opener, [addr], { stdio: 'ignore', detached: true }).on('error', () => {});
      }
    },
    (err) => {
      if (err && err.code === 'EADDRINUSE') {
        console.error(`Port ${ARGS.port} is in use. Try: node server.js --port ${ARGS.port + 1}`);
      } else {
        console.error(err && err.message);
      }
      process.exit(1);
    }
  );
}

module.exports = { server, start, runtime, LIMITS, GUARD_HEADER, ARGS };
