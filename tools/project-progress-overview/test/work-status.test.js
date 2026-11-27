/**
 * Live overview runs read-only `awr status --view action` as the Today board.
 * Queue buckets are navigation, not authorization or completion.
 *
 * Run: node --test test/work-status.test.js
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

const STATUS = {
  ok: true,
  command: 'awr status --view action',
  data: {
    view: 'action',
    schema_version: 1,
    current_total: 1,
    ready_count: 1,
    waiting_count: 1,
    blocked_count: 8,
    current: [{ key: 'DEMO-A-004', title: 'Continue claimed work', next_action: 'Prepare the selected work' }],
    ready: [{ key: 'DEMO-B-004', title: 'Claimable planned work', next_action: 'Prepare required context' }],
    waiting: [{ key: 'DEMO-A-005', title: 'Waiting on a prerequisite', next_action: 'Continue the unfinished prerequisite' }],
    blocked: [{ key: 'DEMO-A-008', title: 'Blocked work', next_action: 'Inspect the cited current gap' }],
    omissions: { current: 0, ready: 0, waiting: 0, blocked: 3 },
    guidance: {
      when: 'Current work can be continued',
      basis: 'Source structure, dependencies and recorded claims',
      next_action: 'Prepare the selected work; continue only with its owned session or explicitly resume it',
      recheck: 'Source, ownership, wait or execution outcome changes',
    },
    count_basis: 'source status; queue buckets are navigation, not execution authorization or completion proof',
  },
};

function boot(status, extra = {}) {
  const t = h.tFor('en');
  const result = page.renderProjectMap(source, config, { t, lang: 'en', interactive: true });
  const { container } = dom.createPage(result.body);
  const app = interact.mount({ container, snapshot: source, config, t, model: result.model, status, ...extra });
  return { t, container, app };
}

test('renderActionBoard shows the four queues and never treats them as authorization', () => {
  const t = h.tFor('en');
  const html = detail.renderActionBoard(STATUS, t);
  assert.match(html, /Continue claimed work/);
  assert.match(html, /Claimable planned work/);
  assert.match(html, /Waiting on a prerequisite/);
  assert.match(html, /Blocked work/);
  assert.match(html, /3 more in this queue/);
  assert.match(html, /not a claim, permission, completion/);
  assert.match(html, /Current work can be continued/);
  assert.doesNotMatch(html, /awr work manage/);
  assert.doesNotMatch(html, /<script/);
});

test('renderActionBoard escapes hostile titles and keeps valid keys only', () => {
  const t = h.tFor('en');
  const html = detail.renderActionBoard({
    ok: true,
    data: {
      current_total: 1,
      ready_count: 0,
      waiting_count: 0,
      blocked_count: 0,
      current: [{ key: 'DEMO-A-004', title: '<img src=x onerror=alert(1)>', next_action: '<script>alert(1)</script>' }],
      ready: [{ key: 'not a key', title: 'dropped' }, { title: 'no key' }],
      waiting: [],
      blocked: [],
      omissions: {},
      guidance: { when: '<b>x</b>', basis: 'y', next_action: 'z', recheck: 'w' },
    },
  }, t);
  assert.match(html, /&lt;img src=x onerror=alert\(1\)&gt;/);
  assert.match(html, /&lt;script&gt;alert\(1\)&lt;\/script&gt;/);
  assert.match(html, /&lt;b&gt;x&lt;\/b&gt;/);
  assert.doesNotMatch(html, /<img /);
  assert.doesNotMatch(html, /data-pm-queue-key="not a key"/);
  assert.doesNotMatch(html, />dropped</);
});

test('renderActionBoard names Unsupported when there is no payload', () => {
  const t = h.tFor('en');
  const html = detail.renderActionBoard({
    ok: false,
    command: 'awr status --view action',
    error: { code: 'Unsupported', message: 'operation is not implemented' },
  }, t);
  assert.match(html, /Unsupported/);
  assert.doesNotMatch(html, /data-pm-queue-key/);
});

test('the live Today board fills from status --view action and opens details from a queue item', async () => {
  const ctx = boot(async () => STATUS);
  assert.equal(ctx.container.querySelector('#v-action').checked, true);
  await ctx.app.statusWait;
  const board = ctx.container.querySelector('[data-pm-action-board]');
  assert.match(board.textContent, /Continue claimed work/);
  assert.match(board.textContent, /not a claim, permission, completion/);
  assert.equal(ctx.container.querySelector('[data-pm-action="manage"]'), null);
  const item = ctx.container.querySelector('[data-pm-queue-key="DEMO-A-004"]');
  assert.ok(item);
  dom.fire(item, 'click');
  const text = ctx.container.querySelector('.pm-drawer-body').textContent;
  assert.match(text, /DEMO-A-004/);
});

test('demo status surfaces the four queues without fetching', async () => {
  let fetched = 0;
  const t = h.tFor('en');
  const ctx = boot(() => {
    fetched += 1;
    return demo.statusAction(t);
  });
  await ctx.app.statusWait;
  assert.equal(fetched, 1);
  const text = ctx.container.querySelector('[data-pm-action-board]').textContent;
  assert.match(text, /DEMO-A-004/);
  assert.match(text, /DEMO-B-004/);
  assert.match(text, /DEMO-A-005/);
  assert.match(text, /DEMO-A-008/);
  assert.doesNotMatch(text, /awr work manage/);
});

test('exported pages keep the snapshot graphs and omit live status', () => {
  const ctx = boot();
  assert.equal(ctx.container.querySelector('[data-pm-action-board]'), null);
  assert.match(ctx.container.querySelector('[data-pm-action-slot]').textContent, /This export keeps the snapshot graphs only/);
});

test('static exports keep the action tab but do not default to it', () => {
  const t = h.tFor('en');
  const result = page.renderProjectMap(source, config, { t, lang: 'en' });
  assert.match(result.body, /id="v-action"/);
  assert.match(result.body, /This export keeps the snapshot graphs only/);
  assert.match(result.body, /id="v-overview" checked/);
  assert.doesNotMatch(result.body, /id="v-action" checked/);
});
