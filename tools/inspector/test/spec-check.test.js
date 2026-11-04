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
    title: 'Save the reviewed configuration',
    status: 'ready',
    next_action: 'Read the declared specification binding',
    milestone: 'goal#demo',
    active_claims: [],
    diagnostics: [],
    spec_refs: [{
      specification: 'configuration-spec',
      item: 'NEW',
      source_fingerprint: 'sha256:' + 'a'.repeat(64),
      criterion: 'Save the reviewed configuration',
    }],
  },
  acceptance: ['Save the reviewed configuration'],
  required_dependencies: [],
  missing_dependencies: [],
  evidence: [],
  decisions: [],
  dependency_cycles: [],
};

const ASSESS = {
  work: 'W1',
  decision: {
    mode: 'undetermined',
    reasons: [],
    unknown_observations: [],
    required_actions: [],
    optional_maintenance: [],
    completion_policy: 'unchanged_source_policy',
    execution_admission: 'not_granted_by_management_classification',
  },
  admission_gaps: [],
  next_action: 'record_current_assessment_with_explicit_host_observations',
};

const CHECK = {
  version: 1,
  status: 'needs_review',
  specifications: [{ key: 'configuration-spec', version: '2', status: 'current' }],
  references: [{
    work: 'W1',
    reference: { specification: 'configuration-spec', item: 'NEW' },
    status: 'current',
    reasons: [],
  }],
  requirements: [
    { specification: 'configuration-spec', item: 'NEW', status: 'covered', work_items: ['W1'] },
    { specification: 'configuration-spec', item: 'EXPORT', status: 'uncovered', work_items: [] },
  ],
  counts: { covered: 1, uncovered: 1, not_required: 0, needs_review: 0, unknown: 0 },
  diagnostics: [],
  association_diagnostics: [],
  coverage_basis: 'Explicit registered items and exact task references only; no prose scanning or inferred cancellations.',
  boundary: 'Matched/covered describe declared version bindings and task coverage, never semantic agreement, implementation, acceptance or completion. No source is modified.',
};

function liveFetch(spec) {
  return async (url) => ({
    json: async () => {
      if (String(url).includes('/api/spec-check')) {
        return spec;
      }
      if (String(url).includes('/api/assess')) {
        return { ok: true, command: 'awr work assess W1', data: ASSESS };
      }
      return { ok: true, command: 'awr work show W1', data: WORK };
    },
  });
}

test('spec check expands declared bindings without treating covered as tests', async () => {
  const app = loadApp();
  const $ = (id) => document.getElementById(id);
  app.state.mode = 'live';
  global.fetch = liveFetch({ ok: true, command: 'awr spec check', data: CHECK });
  await app.renderWorkDetail('W1');
  const text = $('workDetail').textContent;
  assert.ok(text.includes('Save the reviewed configuration'));
  assert.ok(text.includes('needs_review'));
  assert.ok(text.includes('configuration-spec'));
  assert.ok(text.includes('NEW'));
  assert.ok(text.includes('EXPORT'));
  assert.ok(text.includes(i18n.t('ui.spec_check_uncovered')));
  assert.ok(text.includes(i18n.t('ui.spec_check_not_implementation')));
  assert.equal($('workDetail').find((n) => n.tagName === 'BUTTON' && /create task/i.test(n.textContent || '')), null);
});

test('demo spec check surfaces uncovered requirements without fetching', async () => {
  const app = loadApp();
  require('../public/demo-data.js');
  const $ = (id) => document.getElementById(id);
  app.state.mode = 'demo';
  let fetched = false;
  global.fetch = async () => {
    fetched = true;
    throw new Error('demo must not fetch');
  };
  await app.renderWorkDetail('EXAMPLE-001');
  const text = $('workDetail').textContent;
  assert.equal(fetched, false);
  assert.ok(text.includes('configuration-spec'));
  assert.ok(text.includes('EXPORT'));
  assert.ok(text.includes(i18n.t('ui.spec_check_uncovered')));
  assert.ok(text.includes(i18n.t('ui.spec_check_not_implementation')));
});

test('spec check Unsupported keeps work show visible', async () => {
  const app = loadApp();
  const $ = (id) => document.getElementById(id);
  app.state.mode = 'live';
  global.fetch = liveFetch({
    ok: false,
    command: 'awr spec check',
    error: { code: 'Unsupported', message: 'operation is not implemented' },
  });
  await app.renderWorkDetail('W1');
  const text = $('workDetail').textContent;
  assert.ok(text.includes('Save the reviewed configuration'));
  assert.ok(text.includes('Unsupported'));
});
