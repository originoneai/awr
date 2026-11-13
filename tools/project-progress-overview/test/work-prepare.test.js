/**
 * Live work-item details run read-only `awr work prepare --response-view action`.
 * Snapshot facts stay visible; the action card is one bounded next instruction.
 *
 * Run: node --test test/work-prepare.test.js
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

const PREPARE = {
  ok: true,
  command: 'awr work prepare DEMO-A-004 --response-view action',
  data: {
    version: 1,
    stage: 'prepared',
    work: { external_key: 'DEMO-A-004' },
    ready: true,
    ok: true,
    guidance: {
      when: 'Required context is complete and no higher-priority blocker is present',
      basis: 'Current work, claims and management requirements',
      next_action: 'Consume the rendered context and follow this work next_action',
      recheck: 'Scope change, wait, uncertain result, completed compaction or handoff',
    },
    response_view: { version: 1, view: 'action' },
  },
};

function boot(prepare) {
  const t = h.tFor('en');
  const result = page.renderProjectMap(source, config, { t, lang: 'en', interactive: true });
  const { container } = dom.createPage(result.body);
  const app = interact.mount({ container, snapshot: source, config, t, model: result.model, prepare });
  return { t, container, app };
}

test('renderPrepare shows the action card and never treats it as completion', () => {
  const t = h.tFor('en');
  const html = detail.renderPrepare(PREPARE, t);
  assert.match(html, /Required context is complete/);
  assert.match(html, /Current work, claims and management requirements/);
  assert.match(html, /Consume the rendered context/);
  assert.match(html, /Scope change, wait, uncertain result/);
  assert.match(html, /bounded next instruction/);
  assert.doesNotMatch(html, /awr work manage/);
  assert.doesNotMatch(html, /<script/);
});

test('renderPrepare keeps guidance when the CLI reports ContextIncomplete', () => {
  const t = h.tFor('en');
  const html = detail.renderPrepare({
    ok: false,
    command: 'awr work prepare DEMO-A-004 --response-view action',
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
    },
  }, t);
  assert.match(html, /Required context is incomplete/);
  assert.match(html, /Resolve the listed required gaps/);
  assert.doesNotMatch(html, /ContextIncomplete/);
});

test('renderPrepare names Unsupported when there is no guidance payload', () => {
  const t = h.tFor('en');
  const html = detail.renderPrepare({
    ok: false,
    command: 'awr work prepare DEMO-A-004 --response-view action',
    error: { code: 'Unsupported', message: 'operation is not implemented' },
  }, t);
  assert.match(html, /Unsupported/);
});

test('opening details still shows snapshot facts, then the live action card', async () => {
  const ctx = boot(async () => PREPARE);
  assert.equal(ctx.app.openDetails('DEMO-A-004'), true);
  const before = ctx.container.querySelector('.pm-drawer-body').textContent;
  assert.match(before, /DEMO-A-004/);
  await ctx.app.prepareWait;
  const text = ctx.container.querySelector('.pm-drawer-body').textContent;
  assert.match(text, /DEMO-A-004/);
  assert.match(text, /Required context is complete/);
  assert.match(text, /bounded next instruction/);
  assert.equal(ctx.container.querySelector('[data-pm-action="copy-command"]').textContent.includes('manage'), false);
  assert.equal(ctx.container.querySelector('[data-pm-action="manage"]'), null);
});

test('ContextIncomplete still shows the action card on the details panel', async () => {
  const ctx = boot(async () => ({
    ok: false,
    command: 'awr work prepare DEMO-A-004 --response-view action',
    error: { code: 'ContextIncomplete', message: 'resolve required context gaps before execution' },
    data: {
      guidance: {
        when: 'Required context is incomplete',
        basis: 'context.completeness.issues and source freshness',
        next_action: 'Resolve the listed required gaps for this work, then prepare again',
        recheck: 'Required sources, rules or work contract change',
      },
    },
  }));
  ctx.app.openDetails('DEMO-A-004');
  await ctx.app.prepareWait;
  const text = ctx.container.querySelector('.pm-drawer-body').textContent;
  assert.match(text, /DEMO-A-004/);
  assert.match(text, /Required context is incomplete/);
  assert.match(text, /Resolve the listed required gaps/);
  assert.doesNotMatch(text, /ContextIncomplete/);
});

test('demo work prepare surfaces the action card without fetching', async () => {
  let fetched = 0;
  const t = h.tFor('en');
  const ctx = boot((key) => {
    fetched += 1;
    return demo.workPrepare(key, t);
  });
  ctx.app.openDetails('DEMO-A-004');
  await ctx.app.prepareWait;
  assert.equal(fetched, 1);
  const text = ctx.container.querySelector('.pm-drawer-body').textContent;
  assert.match(text, /Required context is incomplete/);
  assert.match(text, /Resolve the listed required gaps/);
  assert.doesNotMatch(text, /ContextIncomplete/);
  assert.doesNotMatch(text, /awr work manage/);
  assert.equal(ctx.container.querySelector('[data-pm-action="manage"]'), null);
});

test('Unsupported prepare keeps the snapshot details visible', async () => {
  const ctx = boot(async () => ({
    ok: false,
    command: 'awr work prepare DEMO-A-004 --response-view action',
    error: { code: 'Unsupported', message: 'operation is not implemented' },
  }));
  ctx.app.openDetails('DEMO-A-004');
  await ctx.app.prepareWait;
  const text = ctx.container.querySelector('.pm-drawer-body').textContent;
  assert.match(text, /DEMO-A-004/);
  assert.match(text, /Unsupported/);
});

test('exported details omit work prepare when no live reader is mounted', () => {
  const ctx = boot();
  ctx.app.openDetails('DEMO-A-004');
  const text = ctx.container.querySelector('.pm-drawer-body').textContent;
  assert.match(text, /DEMO-A-004/);
  assert.equal(ctx.container.querySelector('[data-pm-prepare]'), null);
  assert.doesNotMatch(text, /bounded next instruction/);
});
