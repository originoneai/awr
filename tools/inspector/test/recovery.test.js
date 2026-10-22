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

test('recovery inspect expands checkpoint and execution observations from CLI JSON', () => {
  const app = loadApp();
  const $ = (id) => document.getElementById(id);
  app.state.status = statusWithPending([
    { code: 'active_session', kind: 'session', id: 11 },
  ]);
  app.applyRecoveryInspect(11, {
    ok: true,
    command: 'awr recovery inspect --session 11',
    data: {
      session: { id: 11, agent_id: 'lin', status: 'active' },
      checkpoint: {
        id: 3,
        next_action: 'Collect the report then review the failed import',
        open_loops: ['Independent report review remains'],
      },
      executions: [{
        execution_id: 44,
        state: 'running',
        verified: true,
        operation_key: 'live-report',
        basis: 'loopback supervisor',
        next_action: 'Wait for the supervisor completion event',
      }, {
        execution_id: 45,
        state: 'failed',
        verified: true,
        exit_code: 7,
        operation_key: 'failed-report',
        basis: 'supervisor completion event',
      }],
      all_execution_states_verified: true,
      source_refresh_performed: false,
      side_effects_performed: false,
      next_step: 'Use doctor, then session resume.',
    },
  });
  app.renderPending(app.state.status);
  assert.equal($('pendingPanel').hidden, false);
  assert.equal($('recoveryDetail').hidden, false);
  const text = $('recoveryDetail').textContent;
  assert.ok(text.includes('Collect the report then review the failed import'));
  assert.ok(text.includes('Independent report review remains'));
  assert.ok(text.includes('running'));
  assert.ok(text.includes('failed'));
  assert.ok(text.includes('exit=7'));
  assert.ok(text.includes(i18n.t('ui.recovery_no_side_effects')));
  assert.ok(text.includes(i18n.t('ui.recovery_all_verified')));
  assert.ok(!text.includes('session resume to refresh') || text.includes('Use doctor'));
});

test('non-session pending rows do not invent a recovery inspect command', () => {
  const app = loadApp();
  const $ = (id) => document.getElementById(id);
  app.state.status = statusWithPending([
    { code: 'checkpoint_incomplete', kind: 'checkpoint', id: 'cp_9' },
  ]);
  app.renderPending(app.state.status);
  const row = $('pendingList').find((n) => n.tagName === 'LI');
  assert.ok(row);
  row.click();
  assert.equal($('recoveryDetail').hidden, false);
  assert.ok($('recoveryDetail').textContent.includes(
    i18n.t('ui.pending_no_inspect', { kind: 'checkpoint', id: 'cp_9' })
  ));
  assert.equal(app.state.recoveryDetail.command, null);
});

test('recovery inspect Unsupported keeps the pending list visible', () => {
  const app = loadApp();
  const $ = (id) => document.getElementById(id);
  app.state.status = statusWithPending([
    { code: 'active_session', kind: 'session', id: 11 },
  ]);
  app.applyRecoveryInspect(11, {
    ok: false,
    command: 'awr recovery inspect --session 11',
    error: { code: 'Unsupported', message: 'operation is not implemented' },
  });
  app.renderPending(app.state.status);
  assert.equal($('pendingPanel').hidden, false);
  assert.ok($('pendingList').textContent.includes('active_session'));
  assert.equal($('recoveryDetail').hidden, false);
  assert.ok($('recoveryDetail').textContent.includes('Unsupported'));
});
