#!/usr/bin/env node
/**
 * AWR Observer: observation workspace server.
 *
 * Serves the built React app (dist/) and mounts every Team bridge route from
 * tools/inspector/team-bridge.js unchanged — the bridge is the single read
 * adapter plus the login/connect/revoke entries; this process adds nothing
 * business-shaped of its own.
 *
 * Loopback binding alone is insufficient: any browser page can request
 * 127.0.0.1. The /api/* routes enforce the same request-origin boundary as
 * the Inspector (Host allowlist, Origin/Sec-Fetch-Site checks, a custom
 * header on mutations).
 *
 * Usage:
 *   node server.js [--port 7390] [--demo] [--team-url <url>] [--team-fixture-dir <dir>]
 *
 * Demo mode (fixture data, no live upstream) is the default unless --team-url
 * is given. Build the app first: npx vite build.
 */

'use strict';

const http = require('http');
const fs = require('fs');
const path = require('path');
const crypto = require('crypto');

// Arguments

function parseArgs(argv) {
  const out = {
    port: 7390,
    demo: null, // resolved after parsing: on unless --team-url
    teamUrl: null,
    teamPublicUrl: null,
    teamFixtureDir: null,
  };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === '--port') {
      const n = Number(argv[++i]);
      if (Number.isInteger(n) && n >= 0 && n <= 65535) out.port = n;
    }
    else if (a === '--demo') out.demo = true;
    else if (a === '--team-url') out.teamUrl = argv[++i] || null;
    else if (a === '--team-public-url') out.teamPublicUrl = argv[++i] || null;
    else if (a === '--team-fixture-dir') out.teamFixtureDir = path.resolve(argv[++i] || '.');
    else if (a === '--help' || a === '-h') {
      console.log([
        'Usage: node server.js [options]',
        '',
        '  --port <port>         Listening port (default: 7390; 0 picks a free port)',
        '  --demo                Serve fixture data (default when --team-url is absent)',
        '  --team-url <url>      Proxy Team Web to awr-server /v1/web entry',
        '  --team-public-url <url> Public Team service base URL for member MCP setup',
        '  --team-fixture-dir    Use on-disk team-web-loop fixtures (demo/tests)',
      ].join('\n'));
      process.exit(0);
    }
  }
  if (out.demo == null) out.demo = !out.teamUrl;
  return out;
}

const ARGS = parseArgs(process.argv.slice(2));
const DIST_DIR = path.join(__dirname, 'dist');

const { createTeamBridge } = require('../inspector/team-bridge');
const teamBridge = createTeamBridge({
  teamUrl: ARGS.teamUrl,
  teamPublicUrl: ARGS.teamPublicUrl,
  teamFixtureDir: ARGS.teamFixtureDir,
  demo: ARGS.demo,
  port: ARGS.port,
});
const routes = Object.assign({}, teamBridge.routes);

// Request-origin boundary (mirrors tools/inspector/server.js).

/** Mutations require this custom header; cross-site form POSTs cannot send it. */
const GUARD_HEADER = 'x-awr-observer';

function allowedHosts() {
  const port = server.address() ? server.address().port : ARGS.port;
  return new Set([`127.0.0.1:${port}`, `localhost:${port}`, `[::1]:${port}`]);
}

/**
 * Check whether an /api/* request comes from this local page.
 * Return null to allow it, or the error to send to the caller.
 */
function guardRequest(req) {
  // 1) Host blocks DNS rebinding: the attacker-controlled hostname remains in the header.
  const host = String(req.headers.host || '').toLowerCase();
  if (!allowedHosts().has(host)) {
    return { code: 'ForbiddenHost', message: `Rejected Host: ${host || '(empty)'}` };
  }
  // 2) Origin, when supplied, must match this server.
  const origin = req.headers.origin;
  if (origin !== undefined && !allowedHosts().has(String(origin).replace(/^https?:\/\//, ''))) {
    return { code: 'ForbiddenOrigin', message: `Rejected Origin: ${origin}` };
  }
  // 3) Sec-Fetch-Site cannot be changed by page scripts. Same-origin fetches
  //    use same-origin; address-bar navigation uses none. Reject the rest.
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

// sirv owns asset lookup, MIME handling and traversal containment. The only
// custom serving path is the SPA index, whose nonce must change per response.
const sirv = require('sirv');

/** The built index.html carries these tokens; each response gets a fresh nonce. */
const NONCE_ATTR_TOKEN = '__CSP_NONCE_ATTR__';
const NONCE_VALUE_TOKEN = '__CSP_NONCE_VALUE__';

function cspHeader(nonce) {
  return [
    "default-src 'self'",
    `script-src 'self' 'nonce-${nonce}'`,
    `style-src 'self' 'nonce-${nonce}'`,
    "img-src 'self' data:",
    "connect-src 'self'",
    "object-src 'none'",
    "base-uri 'none'",
    "frame-ancestors 'none'",
  ].join('; ');
}

function securityHeaders(nonce) {
  return {
    'cache-control': 'no-store',
    'content-security-policy': cspHeader(nonce),
    'x-content-type-options': 'nosniff',
    'referrer-policy': 'no-referrer',
  };
}

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

function serveIndex(res) {
  fs.readFile(path.join(DIST_DIR, 'index.html'), (err, buf) => {
    if (err) {
      res.writeHead(503, { 'content-type': 'text/plain; charset=utf-8' });
      res.end('dist/index.html missing — run `npx vite build` first');
      return;
    }
    const nonce = crypto.randomBytes(16).toString('base64');
    const html = buf
      .toString('utf8')
      .split(NONCE_ATTR_TOKEN)
      .join(nonce)
      .split(NONCE_VALUE_TOKEN)
      .join(nonce);
    res.writeHead(200, {
      'content-type': 'text/html; charset=utf-8',
      ...securityHeaders(nonce),
    });
    res.end(html);
  });
}

const serveAsset = sirv(DIST_DIR, {
  etag: true,
  extensions: [],
  onNoMatch(_req, res) {
    res.writeHead(404, { 'content-type': 'text/plain; charset=utf-8' });
    res.end('404');
  },
  setHeaders(res) {
    const nonce = crypto.randomBytes(16).toString('base64');
    for (const [name, value] of Object.entries(securityHeaders(nonce))) {
      res.setHeader(name, value);
    }
  },
});

function serveStatic(req, res, pathname) {
  let decoded;
  try {
    decoded = decodeURIComponent(pathname);
  } catch (_) {
    res.writeHead(400, { 'content-type': 'text/plain; charset=utf-8' });
    res.end('bad path');
    return;
  }
  if (pathname === '/index.html' || !path.extname(decoded)) return serveIndex(res);
  serveAsset(req, res);
}

/** Collect request bodies as Buffers and reject overflow (64 KiB). */
const REQUEST_BYTES = 64 * 1024;

function readBody(req) {
  return new Promise((resolve) => {
    const chunks = [];
    let bytes = 0;
    let killed = false;
    req.on('data', (chunk) => {
      if (killed) return;
      bytes += chunk.length;
      if (bytes > REQUEST_BYTES) {
        killed = true;
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

// HTTP

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
    if (!url.pathname.startsWith('/api/team/')) {
      return sendJson(res, 404, {
        ok: false,
        error: { code: 'NoRoute', message: `${req.method} ${url.pathname}` },
      });
    }
    const handler = routes[`${req.method} ${url.pathname}`];
    if (!handler) {
      return sendJson(res, 404, {
        ok: false,
        error: { code: 'NoRoute', message: `${req.method} ${url.pathname}` },
      });
    }
    try {
      let body = null;
      if (req.method === 'POST') {
        const read = await readBody(req);
        if (read.tooLarge) {
          return sendJson(res, 413, {
            ok: false,
            error: { code: 'BodyTooLarge', message: `Request body exceeds ${REQUEST_BYTES} bytes.` },
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

  if (url.pathname === '/') return serveIndex(res);
  serveStatic(req, res, url.pathname);
}

// Startup

function start() {
  return new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(ARGS.port, '127.0.0.1', () => resolve(server));
  });
}

if (require.main === module) {
  if (!fs.existsSync(path.join(DIST_DIR, 'index.html'))) {
    console.error('dist/index.html missing — run `npx vite build` in tools/observer first.');
    process.exit(1);
  }
  start().then(
    () => {
      const addr = `http://127.0.0.1:${server.address().port}`;
      console.log('');
      console.log('  AWR Observer started');
      console.log('  ─────────────────────────────────────────');
      console.log(`  URL     ${addr}`);
      console.log(`  Mode    ${teamBridge.TEAM.live ? `Live (${ARGS.teamUrl})` : 'Demo (fixtures)'}`);
      console.log('  ─────────────────────────────────────────');
      console.log('  Press Ctrl+C to stop');
      console.log('');
    },
    (err) => {
      if (err && err.code === 'EADDRINUSE') {
        console.error(`Port ${ARGS.port} is in use. Try: node server.js --port ${ARGS.port + 1}`);
      } else {
        console.error(err && err.message);
      }
      process.exit(1);
    },
  );
}

module.exports = { server, start, ARGS, GUARD_HEADER };
