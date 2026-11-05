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
    title: 'Prepare the export work',
    status: 'ready',
    next_action: 'Read the current action card',
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

const PREPARE = {
  version: 1,
  stage: 'prepared',
  work: { external_key: 'W1' },
  ready: true,
  ok: true,
  guidance: {
    when: 'Required context is complete and no higher-priority blocker is present',
    basis: 'Current work, claims and management requirements',
    next_action: 'Consume the rendered context and follow this work\'s next_action using the existing session/claim rules',
    recheck: 'Scope change, wait, uncertain result, completed compaction or handoff',
  },
  response_view: { version: 1, view: 'action' },
};

const SPEC = {
  status: 'unknown',
  references: [],
  requirements: [],
  diagnostics: [],
};

function liveFetch(prepare) {
  return async (url) => ({
    json: async () => {
      if (String(url).includes('/api/prepare')) return prepare;
      if (String(url).includes('/api/spec-check')) {
        return { ok: true, command: 'awr spec check', data: SPEC };
      }
      if (String(url).includes('/api/assess')) {
        return { ok: true, command: 'awr work assess W1', data: ASSESS };
      }
      return { ok: true, command: 'awr work show W1', data: WORK };
    },
  });
}

test('work prepare expands the action card without treating it as completion', async () => {
  const app = loadApp();
  const $ = (id) => document.getElementById(id);
  app.state.mode = 'live';
  global.fetch = liveFetch({ ok: true, command: 'awr work prepare W1 --response-view action', data: PREPARE });
  await app.renderWorkDetail('W1');
  const text = $('workDetail').textContent;
  assert.ok(text.includes('Prepare the export work'));
  assert.ok(text.includes('Required context is complete and no higher-priority blocker is present'));
  assert.ok(text.includes('Current work, claims and management requirements'));
  assert.ok(text.includes('Consume the rendered context'));
  assert.ok(text.includes('Scope change, wait, uncertain result'));
  assert.ok(text.includes(i18n.t('ui.work_prepare_not_completion')));
  assert.ok(!text.includes('awr work manage'));
  assert.equal($('workDetail').find((n) => n.tagName === 'BUTTON' && /manage/i.test(n.textContent || '')), null);
});

test('work prepare ContextIncomplete still shows the action card', async () => {
  const app = loadApp();
  const $ = (id) => document.getElementById(id);
  app.state.mode = 'live';
  global.fetch = liveFetch({
    ok: false,
    command: 'awr work prepare W1 --response-view action',
    error: { code: 'ContextIncomplete', message: 'resolve required context gaps before execution' },
    data: {
      version: 1,
      ok: false,
      guidance: {
        when: 'Required context is incomplete',
        basis: 'context.completeness.issues and source freshness',
        next_action: 'Resolve the listed required gaps for this work, then prepare again',
        recheck: 'Required sources, rules or work contract change',
      },
      response_view: { version: 1, view: 'action' },
    },
  });
  await app.renderWorkDetail('W1');
  const text = $('workDetail').textContent;
  assert.ok(text.includes('Prepare the export work'));
  assert.ok(text.includes('Required context is incomplete'));
  assert.ok(text.includes('Resolve the listed required gaps'));
  assert.ok(!text.includes('ContextIncomplete'), text);
  assert.ok(text.includes(i18n.t('ui.work_prepare_not_completion')));
});

test('demo work prepare surfaces the action card without fetching', async () => {
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
  assert.ok(text.includes('Required context is incomplete'));
  assert.ok(text.includes('Resolve the listed required gaps'));
  assert.ok(text.includes(i18n.t('ui.work_prepare_not_completion')));
  assert.ok(!text.includes('awr work manage'));
});

test('work prepare Unsupported keeps work show visible', async () => {
  const app = loadApp();
  const $ = (id) => document.getElementById(id);
  app.state.mode = 'live';
  global.fetch = liveFetch({
    ok: false,
    command: 'awr work prepare W1 --response-view action',
    error: { code: 'Unsupported', message: 'operation is not implemented' },
  });
  await app.renderWorkDetail('W1');
  const text = $('workDetail').textContent;
  assert.ok(text.includes('Prepare the export work'));
  assert.ok(text.includes('Export the reviewed format'));
  assert.ok(text.includes('Unsupported'));
});
