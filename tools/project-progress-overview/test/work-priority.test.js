/**
 * Live work-item details run read-only `awr work show` only to overlay priority.
 * The snapshot still lists node.priority as a bulk gap and is not rewritten.
 *
 * Run: node --test test/work-priority.test.js
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

const SHOW = {
  ok: true,
  command: 'awr work show DEMO-A-004',
  data: { work: { external_key: 'DEMO-A-004', priority: 'P1' } },
};

function boot(workShow) {
  const t = h.tFor('en');
  const result = page.renderProjectMap(source, config, { t, lang: 'en', interactive: true });
  const { container } = dom.createPage(result.body);
  const app = interact.mount({ container, snapshot: source, config, t, model: result.model, workShow });
  return { t, container, app, snapshot: source };
}

test('renderPriority shows the recorded priority and nothing else from work show', () => {
  const t = h.tFor('en');
  const html = detail.renderPriority(SHOW, t);
  assert.match(html, />P1</);
  assert.match(html, /on-demand work show/);
  assert.doesNotMatch(html, /acceptance/);
  assert.doesNotMatch(html, /awr work manage/);
  assert.doesNotMatch(html, /<script/);
});

test('renderPriority treats a null priority as unset', () => {
  const t = h.tFor('en');
  const html = detail.renderPriority({ ok: true, data: { work: { external_key: 'DEMO-A-004', priority: null } } }, t);
  assert.match(html, /not set/);
});

test('renderPriority names Unsupported when there is no work payload', () => {
  const t = h.tFor('en');
  const html = detail.renderPriority({
    ok: false,
    command: 'awr work show DEMO-A-004',
    error: { code: 'Unsupported', message: 'operation is not implemented' },
  }, t);
  assert.match(html, /Unsupported/);
});

test('renderPriority escapes a hostile priority string', () => {
  const t = h.tFor('en');
  const html = detail.renderPriority({
    ok: true,
    data: { work: { priority: '<img src=x onerror=alert(1)>' } },
  }, t);
  assert.match(html, /&lt;img src=x onerror=alert\(1\)&gt;/);
  assert.doesNotMatch(html, /<img src=x/);
});

test('opening details still shows snapshot facts, then the live priority', async () => {
  const ctx = boot(async () => SHOW);
  assert.equal(ctx.app.openDetails('DEMO-A-004'), true);
  const before = ctx.container.querySelector('.pm-drawer-body').textContent;
  assert.match(before, /DEMO-A-004/);
  assert.match(before, /node\.priority/);
  await ctx.app.showWait;
  const text = ctx.container.querySelector('.pm-drawer-body').textContent;
  assert.match(text, /DEMO-A-004/);
  assert.match(text, /P1/);
  assert.match(text, /node\.priority/);
  assert.equal(ctx.snapshot.unavailable.some((u) => u.field === 'node.priority'), true);
  assert.equal(ctx.app.model.raw.get('DEMO-A-004').priority, undefined);
});

test('demo work show overlays priority without fetching', async () => {
  let fetched = 0;
  const ctx = boot((key) => {
    fetched += 1;
    return demo.workShow(key);
  });
  ctx.app.openDetails('DEMO-A-004');
  await ctx.app.showWait;
  assert.equal(fetched, 1);
  const text = ctx.container.querySelector('.pm-drawer-body').textContent;
  assert.match(text, /P1/);
  assert.doesNotMatch(text, /awr work manage/);
});

test('Unsupported work show keeps the snapshot details visible', async () => {
  const ctx = boot(async () => ({
    ok: false,
    command: 'awr work show DEMO-A-004',
    error: { code: 'Unsupported', message: 'operation is not implemented' },
  }));
  ctx.app.openDetails('DEMO-A-004');
  await ctx.app.showWait;
  const text = ctx.container.querySelector('.pm-drawer-body').textContent;
  assert.match(text, /DEMO-A-004/);
  assert.match(text, /Unsupported/);
  assert.match(text, /node\.priority/);
});

test('exported details omit live priority when no reader is mounted', () => {
  const ctx = boot();
  ctx.app.openDetails('DEMO-A-004');
  const text = ctx.container.querySelector('.pm-drawer-body').textContent;
  assert.match(text, /DEMO-A-004/);
  assert.equal(ctx.container.querySelector('[data-pm-priority]'), null);
  assert.match(text, /node\.priority/);
  assert.doesNotMatch(text, /on-demand work show/);
});
