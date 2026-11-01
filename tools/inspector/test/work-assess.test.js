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

const WORK = {
  ok: true,
  work: {
    external_key: 'W1',
    title: 'Assess the export work',
    status: 'ready',
    next_action: 'Read the current assessment',
    milestone: 'goal#demo',
    active_claims: [],
    diagnostics: [],
  },
  acceptance: ['Export the reviewed format'],
  required_dependencies: [],
  missing_dependencies: [],
  evidence: [],
  decisions: [],
  dependency_cycles: [],
};

const ASSESS = {
  version: 1,
  work: 'W1',
  decision: {
    version: 1,
    mode: 'undetermined',
    reasons: [{ code: 'unresolved_dependencies', basis: 'source_projection', reference: 'W1' }],
    unknown_observations: ['single_outcome', 'bounded_scope'],
    reevaluation_signals: [],
    required_actions: [
      'preserve_identity_intent_scope_and_current_state',
      'assess_unknown_management_facts',
    ],
    optional_maintenance: [],
    completion_policy: 'unchanged_source_policy',
    execution_admission: 'not_granted_by_management_classification',
  },
  observation: null,
  observation_basis: 'host_assertion_not_independently_verified',
  admission_gaps: ['source_acceptance_missing_or_ambiguous'],
  record_required: true,
  next_action: 'repair_required_context_before_execution',
};

test('work assess expands intensity and admission gaps without treating them as completion', async () => {
  const app = loadApp();
  const $ = (id) => document.getElementById(id);
  app.state.mode = 'live';
  global.fetch = async (url) => ({
    json: async () => {
      if (String(url).includes('/api/spec-check')) {
        return { ok: true, command: 'awr spec check', data: { status: 'unknown', references: [], requirements: [], diagnostics: [] } };
      }
      return String(url).includes('/api/assess')
        ? { ok: true, command: 'awr work assess W1', data: ASSESS }
        : { ok: true, command: 'awr work show W1', data: WORK };
    },
  });
  await app.renderWorkDetail('W1');
  const text = $('workDetail').textContent;
  assert.ok(text.includes('Assess the export work'));
  assert.ok(text.includes('undetermined'));
  assert.ok(text.includes('source_acceptance_missing_or_ambiguous'));
  assert.ok(text.includes('repair_required_context_before_execution'));
  assert.ok(text.includes('not_granted_by_management_classification'));
  assert.ok(text.includes('unchanged_source_policy'));
  assert.ok(text.includes(i18n.t('ui.work_assess_not_completion')));
  assert.ok(!text.includes('awr work manage'));
  assert.equal($('workDetail').find((n) => n.tagName === 'BUTTON' && /manage/i.test(n.textContent || '')), null);
});

test('demo work assess surfaces structural gaps without fetching or recording', async () => {
  const app = loadApp();
  require('../public/demo-data.js');
  const $ = (id) => document.getElementById(id);
  app.state.mode = 'demo';
  let fetched = false;
  global.fetch = async () => {
    fetched = true;
    throw new Error('demo must not fetch');
  };
  await app.renderWorkDetail('EXAMPLE-008');
  const text = $('workDetail').textContent;
  assert.equal(fetched, false);
  assert.ok(text.includes('goal_context_missing'));
  assert.ok(text.includes('repair_required_context_before_execution'));
  assert.ok(text.includes(i18n.t('ui.work_assess_not_completion')));
  assert.ok(!text.includes('awr work manage'));
});

test('work assess Unsupported keeps work show visible', async () => {
  const app = loadApp();
  const $ = (id) => document.getElementById(id);
  app.state.mode = 'live';
  global.fetch = async (url) => ({
    json: async () => {
      if (String(url).includes('/api/spec-check')) {
        return { ok: true, command: 'awr spec check', data: { status: 'unknown', references: [], requirements: [], diagnostics: [] } };
      }
      return String(url).includes('/api/assess')
        ? {
            ok: false,
            command: 'awr work assess W1',
            error: { code: 'Unsupported', message: 'operation is not implemented' },
          }
        : { ok: true, command: 'awr work show W1', data: WORK };
    },
  });
  await app.renderWorkDetail('W1');
  const text = $('workDetail').textContent;
  assert.ok(text.includes('Assess the export work'));
  assert.ok(text.includes('Export the reviewed format'));
  assert.ok(text.includes('Unsupported'));
});
