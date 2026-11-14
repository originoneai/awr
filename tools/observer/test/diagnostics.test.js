'use strict';

const assert = require('node:assert/strict');
const { test } = require('node:test');
const { diagnosticRecord } = require('../server/diagnostics');

test('server diagnostics retain bounded correlation and failure metadata', () => {
  const record = diagnosticRecord({ requestId: 'synthetic-request', route: '/api/team/read', method: 'POST', operation: 'work.snapshot', status: 200, code: 'Unavailable', durationMs: 12.4 });
  assert.equal(record.request_id, 'synthetic-request');
  assert.equal(record.route, '/api/team/read');
  assert.equal(record.code, 'Unavailable');
  assert.equal(record.operation, 'work.snapshot');
  assert.equal(record.duration_ms, 12);
  assert.ok(Number.isFinite(Date.parse(record.timestamp)));
});

test('diagnostics omit arbitrary upstream prose, credentials and source content', () => {
  const secret = 'synthetic-secret-never-used-for-authentication';
  const record = diagnosticRecord({ requestId: 'synthetic-request', route: '/api/team/read', method: 'POST', operation: secret, status: 500, code: secret, durationMs: 1, message: secret, authorization: secret, body: { source: secret }, stack: secret });
  assert.equal(record.code, 'ObservationError');
  assert.equal(record.operation, null);
  assert.ok(!JSON.stringify(record).includes(secret));
  assert.ok(!Object.hasOwn(record, 'message'));
  assert.ok(!Object.hasOwn(record, 'body'));
});

test('diagnostics keep log injection out of error categories', () => {
  const record = diagnosticRecord({ requestId: 'synthetic-request', route: 'unrecognized', method: 'OTHER', status: 400, code: 'Forbidden\nforged=true', durationMs: -2 });
  assert.equal(record.code, 'ObservationError');
  assert.equal(record.duration_ms, 0);
});
