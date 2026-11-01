'use strict';

const assert = require('node:assert/strict');
const http = require('node:http');
const { after, before, test } = require('node:test');
const { server, start, ARGS } = require('../server.js');

let base;

before(async () => {
  ARGS.port = 0;
  await start();
  base = `http://127.0.0.1:${server.address().port}`;
});

after(async () => {
  if (server.listening) await new Promise((resolve) => server.close(resolve));
});

function nonceFrom(response, html) {
  const csp = response.headers.get('content-security-policy') || '';
  const headerNonce = csp.match(/script-src[^;]*'nonce-([^']+)'/)?.[1];
  const htmlNonce = html.match(/window\.__CSP_NONCE__ = "([^"]+)"/)?.[1];
  assert.ok(headerNonce, 'CSP script nonce is present');
  assert.ok(htmlNonce, 'HTML bootstrap nonce is present');
  assert.match(html, new RegExp(`<script nonce="${headerNonce.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}"`));
  assert.equal(htmlNonce, headerNonce);
  assert.ok(!csp.includes("'unsafe-inline'"));
  return headerNonce;
}

test('HTML responses carry distinct nonces in both CSP and served markup', async () => {
  const first = await fetch(`${base}/`);
  const firstHtml = await first.text();
  const second = await fetch(`${base}/projects/demo`);
  const secondHtml = await second.text();
  assert.equal(first.status, 200);
  assert.equal(second.status, 200);
  const firstNonce = nonceFrom(first, firstHtml);
  const secondNonce = nonceFrom(second, secondHtml);
  assert.notEqual(firstNonce, secondNonce);
});

test('extensionless routes use the SPA index and missing assets are 404', async () => {
  const route = await fetch(`${base}/projects/demo`);
  assert.equal(route.status, 200);
  assert.match(await route.text(), /<div id="root"><\/div>/);
  const missing = await fetch(`${base}/assets/not-present.js`);
  assert.equal(missing.status, 404);
});

test('encoded and plain traversal cannot expose files outside dist', async () => {
  for (const target of ['/../package.json', '/..%2f..%2fpackage.json', '/../']) {
    const response = await fetch(`${base}${target}`);
    const body = await response.text();
    assert.ok(response.status < 500, `${target} returned ${response.status}`);
    assert.ok(!body.includes('"name": "awr-observer"'), `${target} exposed package.json`);
    assert.ok(!body.includes('READ_AUTH_FAILURES'), `${target} exposed bridge source`);
  }
});

test('API guards reject a bad Host', async () => {
  const response = await new Promise((resolve, reject) => {
    const request = http.request(`${base}/api/team/projects`, {
      headers: { host: 'attacker.example' },
    }, (incoming) => {
      let body = '';
      incoming.on('data', (chunk) => { body += chunk; });
      incoming.on('end', () => resolve({ status: incoming.statusCode, body: JSON.parse(body) }));
    });
    request.on('error', reject);
    request.end();
  });
  assert.equal(response.status, 403);
  assert.equal(response.body.error.code, 'ForbiddenHost');
});

test('API guards reject a foreign Origin', async () => {
  const response = await fetch(`${base}/api/team/projects`, { headers: { origin: 'https://attacker.example' } });
  assert.equal(response.status, 403);
  assert.equal((await response.json()).error.code, 'ForbiddenOrigin');
});

test('API guards reject cross-site fetch metadata', async () => {
  const response = await fetch(`${base}/api/team/projects`, { headers: { 'sec-fetch-site': 'cross-site' } });
  assert.equal(response.status, 403);
  assert.equal((await response.json()).error.code, 'ForbiddenSite');
});

test('POST without the observer guard header returns MissingGuardHeader', async () => {
  const response = await fetch(`${base}/api/team/logout`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: '{}',
  });
  assert.equal(response.status, 403);
  assert.equal((await response.json()).error.code, 'MissingGuardHeader');
});

test('request bodies over 64 KiB return 413', async () => {
  const response = await fetch(`${base}/api/team/login`, {
    method: 'POST',
    headers: { 'content-type': 'application/json', 'x-awr-observer': '1' },
    body: JSON.stringify({ bearer: 'x'.repeat(64 * 1024) }),
  });
  assert.equal(response.status, 413);
  assert.equal((await response.json()).error.code, 'BodyTooLarge');
});

test('index, a static asset and an API reply are all cache-control: no-store', async () => {
  const index = await fetch(`${base}/`);
  assert.equal(index.status, 200);
  assert.equal(index.headers.get('cache-control'), 'no-store');
  const html = await index.text();
  const assetPath = html.match(/src="(\/assets\/[^"]+\.js)"/)?.[1];
  assert.ok(assetPath, 'the index references a built asset');
  const asset = await fetch(`${base}${assetPath}`);
  assert.equal(asset.status, 200);
  assert.equal(asset.headers.get('cache-control'), 'no-store');
  const api = await fetch(`${base}/api/team/projects`);
  assert.equal(api.status, 200);
  assert.equal(api.headers.get('cache-control'), 'no-store');
});
