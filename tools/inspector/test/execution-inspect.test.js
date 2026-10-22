'use strict';
const { test } = require('node:test');
const assert = require('node:assert/strict');
const { install } = require('./fixtures/dom-stub.js');
const i18n = require('../public/i18n.js');

function loadApp() {
  install();
  delete require.cache[require.resolve('../public/app.js')];
  return require('../public/app.js');
}

function statusWithPending(items) {
  return {
    pendingOps: items,
    pendingTotal: items.length,
    gaps: [],
    gapTotal: 0,
    currentTotal: 0,
    readyCount: 0,
    waitingCount: 0,
    blockedCount: 0,
    revision: 1,
    queues: { current: { available: true, total: 0 }, ready: { available: true, total: 0 }, waiting: { available: true, total: 0 }, blocked: { available: true, total: 0 } },
  };
}

const UNKNOWN_OBSERVATION = {
  execution_id: 'exe_4411',
  operation_key: 'live-report',
  purpose: 'slow_child',
  origin_session_id: 101,
  state: 'unknown',
  verified: false,
  basis: 'supervisor_unreachable_without_completion_receipt',
  next_action: 'Inspect the registered execution before retrying',
  stdout: 'SECRET_STDOUT',
  stderr: 'SECRET_STDERR',
  receipt: 'SECRET_RECEIPT',
};

test('execution inspect expands the observation and hides bulky report fields', () => {
  const app = loadApp();
  const $ = (id) => document.getElementById(id);
  app.state.status = statusWithPending([
    { code: 'execution_outcome_unknown', kind: 'execution', id: 'exe_4411' },
  ]);
  app.applyExecutionInspect('exe_4411', {
    ok: true,
    command: 'awr execution inspect exe_4411',
    data: {
      observation: UNKNOWN_OBSERVATION,
      external_report: { body: 'SECRET_REPORT', bytes: 9001 },
      side_effects_performed: false,
    },
  });
  app.renderPending(app.state.status);
  assert.equal($('pendingPanel').hidden, false);
  assert.equal($('recoveryDetail').hidden, false);
  const text = $('recoveryDetail').textContent;
  assert.ok(text.includes('exe_4411'));
  assert.ok(text.includes('unknown'));
  assert.ok(text.includes('live-report'));
  assert.ok(text.includes('slow_child'));
  assert.ok(text.includes('101'));
  assert.ok(text.includes(i18n.t('ui.recovery_unverified')));
  assert.ok(text.includes(i18n.t('ui.execution_no_side_effects')));
  assert.ok(text.includes('Inspect the registered execution before retrying'));
  assert.ok(!text.includes('failed'));
  assert.ok(!text.includes('SECRET_STDOUT'));
  assert.ok(!text.includes('SECRET_STDERR'));
  assert.ok(!text.includes('SECRET_RECEIPT'));
  assert.ok(!text.includes('SECRET_REPORT'));
  assert.ok(!text.includes('9001'));
});

test('pending execution rows run read-only execution inspect', () => {
  const app = loadApp();
  const $ = (id) => document.getElementById(id);
  window.AWR_DEMO = {
    executionInspects: {
      exe_4411: {
        observation: UNKNOWN_OBSERVATION,
        side_effects_performed: false,
      },
    },
  };
  app.state.mode = 'demo';
  app.state.status = statusWithPending([
    { code: 'execution_outcome_unknown', kind: 'execution', id: 'exe_4411' },
  ]);
  app.renderPending(app.state.status);
  const row = $('pendingList').find((n) => n.tagName === 'LI');
  assert.ok(row);
  row.click();
  assert.equal($('recoveryDetail').hidden, false);
  assert.equal(app.state.executionDetail.command, 'awr execution inspect exe_4411');
  assert.ok($('recoveryDetail').textContent.includes('unknown'));
  assert.ok(!$('recoveryDetail').textContent.includes('SECRET_STDOUT'));
});

test('execution inspect Unsupported keeps the pending list visible', () => {
  const app = loadApp();
  const $ = (id) => document.getElementById(id);
  app.state.status = statusWithPending([
    { code: 'execution_outcome_unknown', kind: 'execution', id: 'exe_4411' },
  ]);
  app.applyExecutionInspect('exe_4411', {
    ok: false,
    command: 'awr execution inspect exe_4411',
    error: { code: 'Unsupported', message: 'operation is not implemented' },
  });
  app.renderPending(app.state.status);
  assert.equal($('pendingPanel').hidden, false);
  assert.ok($('pendingList').textContent.includes('execution_outcome_unknown'));
  assert.equal($('recoveryDetail').hidden, false);
  assert.ok($('recoveryDetail').textContent.includes('Unsupported'));
});
