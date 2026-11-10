/**
 * Live work-item details run read-only `awr work assess`.
 * Snapshot facts stay visible; the assess card is management intensity, not completion.
 *
 * Run: node --test test/work-assess.test.js
 */

'use strict';

const { test } = require('node:test');
const assert = require('node:assert/strict');
const h = require('./fixtures/project-map/helpers.js');
const dom = require('./fixtures/project-map/mini-dom.js');
const page = require('../public/project-map/page.js');
const interact = require('../public/project-map/interact.js');
const detail = require('../public/project-map/detail.js');
const demo = require('../public/project-map/demo.js');

const source = h.readJson('states.json');
const config = h.readJson('config.json');

const ASSESS = {
  ok: true,
  command: 'awr work assess DEMO-A-004',
  data: {
    version: 1,
    work: 'DEMO-A-004',
    decision: {
      version: 1,
      mode: 'undetermined',
      reasons: [{ code: 'unresolved_dependencies', basis: 'source_projection', reference: 'DEMO-A-004' }],
      unknown_observations: ['single_outcome', 'bounded_scope'],
      required_actions: ['preserve_identity_intent_scope_and_current_state', 'assess_unknown_management_facts'],
      completion_policy: 'unchanged_source_policy',
      execution_admission: 'not_granted_by_management_classification',
    },
    observation_basis: 'host_assertion_not_independently_verified',
    admission_gaps: ['source_acceptance_missing_or_ambiguous'],
    record_required: true,
    next_action: 'repair_required_context_before_execution',
  },
};

function boot(assess) {
  const t = h.tFor('en');
  const result = page.renderProjectMap(source, config, { t, lang: 'en', interactive: true });
  const { container } = dom.createPage(result.body);
  const app = interact.mount({ container, snapshot: source, config, t, model: result.model, assess });
  return { t, container, app };
}

test('renderAssess shows intensity and never treats it as completion', () => {
  const t = h.tFor('en');
  const html = detail.renderAssess(ASSESS, t);
  assert.match(html, /undetermined/);
  assert.match(html, /not_granted_by_management_classification/);
  assert.match(html, /unchanged_source_policy/);
  assert.match(html, /source_acceptance_missing_or_ambiguous/);
  assert.match(html, /repair_required_context_before_execution/);
  assert.match(html, /unresolved_dependencies · source_projection/);
  assert.match(html, /management intensity/);
  assert.doesNotMatch(html, /awr work manage/);
  assert.doesNotMatch(html, /<script/);
});

test('renderAssess names Unsupported when there is no assessment payload', () => {
  const t = h.tFor('en');
  const html = detail.renderAssess({
    ok: false,
    command: 'awr work assess DEMO-A-004',
    error: { code: 'Unsupported', message: 'operation is not implemented' },
  }, t);
  assert.match(html, /Unsupported/);
});

test('renderAssess escapes hostile assessment text', () => {
  const t = h.tFor('en');
  const html = detail.renderAssess({
    ok: true,
    data: {
      decision: { mode: '<img src=x>', reasons: [{ code: '<script>', basis: 'alert(1)' }] },
      admission_gaps: ['<b>gap</b>'],
      next_action: 'ok',
    },
  }, t);
  assert.match(html, /&lt;img src=x&gt;/);
  assert.match(html, /&lt;script&gt; · alert\(1\)/);
  assert.match(html, /&lt;b&gt;gap&lt;\/b&gt;/);
  assert.doesNotMatch(html, /<img src=x>/);
});

test('opening details still shows snapshot facts, then the live assess card', async () => {
  const ctx = boot(async () => ASSESS);
  assert.equal(ctx.app.openDetails('DEMO-A-004'), true);
  const before = ctx.container.querySelector('.pm-drawer-body').textContent;
  assert.match(before, /DEMO-A-004/);
  await ctx.app.assessWait;
  const text = ctx.container.querySelector('.pm-drawer-body').textContent;
  assert.match(text, /DEMO-A-004/);
  assert.match(text, /undetermined/);
  assert.match(text, /source_acceptance_missing_or_ambiguous/);
  assert.equal(ctx.container.querySelector('[data-pm-action="copy-command"]').textContent.includes('manage'), false);
  assert.equal(ctx.container.querySelector('[data-pm-action="manage"]'), null);
});

test('demo work assess surfaces structural gaps without fetching', async () => {
  let fetched = 0;
  const t = h.tFor('en');
  const ctx = boot((key) => {
    fetched += 1;
    return demo.workAssess(key, t);
  });
  ctx.app.openDetails('DEMO-A-004');
  await ctx.app.assessWait;
  assert.equal(fetched, 1);
  const text = ctx.container.querySelector('.pm-drawer-body').textContent;
  assert.match(text, /goal_context_missing/);
  assert.match(text, /repair_required_context_before_execution/);
  assert.doesNotMatch(text, /awr work manage/);
  assert.equal(ctx.container.querySelector('[data-pm-action="manage"]'), null);
});

test('Unsupported assess keeps the snapshot details visible', async () => {
  const ctx = boot(async () => ({
    ok: false,
    command: 'awr work assess DEMO-A-004',
    error: { code: 'Unsupported', message: 'operation is not implemented' },
  }));
  ctx.app.openDetails('DEMO-A-004');
  await ctx.app.assessWait;
  const text = ctx.container.querySelector('.pm-drawer-body').textContent;
  assert.match(text, /DEMO-A-004/);
  assert.match(text, /Unsupported/);
});

test('exported details omit work assess when no live reader is mounted', () => {
  const ctx = boot();
  ctx.app.openDetails('DEMO-A-004');
  const text = ctx.container.querySelector('.pm-drawer-body').textContent;
  assert.match(text, /DEMO-A-004/);
  assert.equal(ctx.container.querySelector('[data-pm-assess]'), null);
  assert.doesNotMatch(text, /management intensity/);
});
