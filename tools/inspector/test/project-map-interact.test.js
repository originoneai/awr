/**
 * Project map interaction layer: the cards, folds, highlight, search, filter, zoom and the details panel, driven through a
 * small DOM (test/fixtures/project-map/mini-dom.js) with the real renderer markup. The pure parts (what a panel says, which
 * items form a chain, what a fold state draws) are tested without any DOM in project-map-detail.test.js.
 *
 * Run: node --test test/project-map-interact.test.js
 */

'use strict';

const { test } = require('node:test');
const assert = require('node:assert/strict');
const h = require('./fixtures/project-map/helpers.js');
const dom = require('./fixtures/project-map/mini-dom.js');
const snapshotLib = require('../public/project-map/snapshot.js');
const page = require('../public/project-map/page.js');
const interact = require('../public/project-map/interact.js');
const detail = require('../public/project-map/detail.js');
const modelLib = require('../public/project-map/model.js');

const source = h.readJson('states.json');
const config = h.readJson('config.json');

/** The fixture plus a chain of extra items in lane A, so that its "waiting" group has more cards than fit and folds into "N more". */
function grown(count = 8) {
  const next = JSON.parse(JSON.stringify(source));
  for (let i = 15; i < 15 + count; i++) {
    const key = `DEMO-A-${String(i).padStart(3, '0')}`;
    next.nodes.push({ ...next.nodes[4], id: `id-${key}`, key, title: `Extra item ${i}`, status: 'planned', ready: false, milestone: 'M1' });
    if (i > 15) next.edges.push({ dependent: key, prerequisite: `DEMO-A-${String(i - 1).padStart(3, '0')}`, required: true });
  }
  next.nodes.sort((a, b) => (a.key < b.key ? -1 : 1));
  next.edges.sort((a, b) => (`${a.dependent}\u0000${a.prerequisite}` < `${b.dependent}\u0000${b.prerequisite}` ? -1 : 1));
  next.basis.work_graph.node_count = next.nodes.length;
  next.basis.work_graph.edge_count = next.edges.length;
  return snapshotLib.seal(next, source.generated_at);
}

/** The fixture without the dependency of the cancelled item on DEMO-A-001, so that lane A's finished work collapses in the panel. */
function quiet() {
  const next = JSON.parse(JSON.stringify(source));
  next.edges = next.edges.filter((e) => !(e.prerequisite === 'DEMO-A-001' && e.dependent === 'DEMO-A-011'));
  next.basis.work_graph.edge_count = next.edges.length;
  return snapshotLib.seal(next, source.generated_at);
}

function fakeTimers() {
  const tasks = new Map();
  let next = 1;
  return {
    set(fn) { const id = next++; tasks.set(id, fn); return id; },
    clear(id) { tasks.delete(id); },
    run() { const fns = [...tasks.values()]; tasks.clear(); for (const fn of fns) fn(); },
    get pending() { return tasks.size; },
  };
}

/** Render the interactive page markup, attach it to a document and mount the layer. */
function boot({ snapshot = source, lang = 'en', cfg = config, restore, container: reuse, offsetTop, noCopy } = {}) {
  const t = h.tFor(lang);
  const result = page.renderProjectMap(snapshot, cfg, { t, lang, interactive: true });
  const { doc, container } = reuse || dom.createPage(result.body);
  if (reuse) container.innerHTML = result.body;
  const copied = [];
  const timers = fakeTimers();
  // a stand-in for the window: an event target that records scrolling and holds a clipboard
  const win = doc.createElement('window');
  win.scrolls = [];
  win.scrollBy = (x, y) => win.scrolls.push([x, y]);
  win.navigator = { clipboard: { writeText: (text) => { copied.push(text); return Promise.resolve(); } } };
  const copy = noCopy ? undefined : (text) => { copied.push(text); return Promise.resolve(); };
  const app = interact.mount({ container, snapshot, config: cfg, t, model: result.model, restore, offsetTop, copy, win, timers });
  const ctx = {
    t, result, doc, container, app, copied, timers, win,
    section: (id) => container.querySelector(`#s-${id} .scroll`),
    card: (key, id = app.state.view) => ctx.section(id).querySelector(`.pm-card[data-key="${key}"]`),
    cards: (id = app.state.view) => ctx.section(id).querySelectorAll('.pm-card').map((c) => c.getAttribute('data-key')),
    action: (name) => container.querySelector(`[data-pm-action="${name}"]`),
    fold: (selector, id = app.state.view) => ctx.section(id).querySelector(selector),
    tab: (id) => { const radio = container.querySelector(`#v-${id}`); radio.checked = true; dom.fire(radio, 'change'); },
  };
  return ctx;
}

/** A click on something inside the control, like a real pointer would hit a label or a shape. */
const hit = (el) => el.querySelector('text') || el.querySelectorAll('rect').find((r) => !r.classList.contains('pm-ring')) || el;
/** Identity check that never prints the DOM when it fails (a failing deep comparison of a document is enormous). */
const same = (actual, expected, message) => assert.ok(actual === expected, message || 'not the same element');
const absent = (el, message) => assert.ok(!el, message || 'expected nothing to be drawn');
const click = (el) => dom.fire(hit(el), 'click');
const has = (el, name) => el.classList.contains(name);
const labels = (ctx) => ctx.container.textContent;

// ---------------------------------------------------------------------- the layer starts from the static page

test('mounting turns the cards, folds and rows into labelled keyboard controls and shows the toolbar', () => {
  const ctx = boot();
  const bar = ctx.container.querySelector('.pm-toolbar');
  assert.equal(bar.hidden, false);
  assert.ok(ctx.container.classList.contains('pm-scope'));
  for (const id of interact.VIEW_IDS) {
    const section = ctx.section(id);
    assert.ok(has(section, 'pm-live'));
    assert.equal(section.querySelector('svg').getAttribute('role'), 'group', 'the drawing is a group of controls, not a picture');
    assert.ok(section.querySelector('svg').getAttribute('aria-label'));
  }
  const cards = ctx.container.querySelectorAll('.pm-card');
  assert.ok(cards.length > 40);
  for (const card of cards) {
    assert.equal(card.getAttribute('tabindex'), '0');
    assert.equal(card.getAttribute('role'), 'button');
    assert.match(card.getAttribute('aria-label'), /^DEMO-[A-C]-\d+, .+, .+\. Press Enter for details\.$/);
  }
  const folds = ctx.container.querySelectorAll('.pm-fold');
  assert.ok(folds.length >= 6);
  for (const fold of folds) {
    assert.equal(fold.getAttribute('role'), 'button');
    assert.equal(fold.getAttribute('tabindex'), '0');
    assert.match(fold.getAttribute('aria-expanded'), /^(true|false)$/);
    assert.match(fold.getAttribute('aria-label'), /^Fold or unfold \S/);
  }
  for (const row of ctx.container.querySelectorAll('tr[data-key]')) {
    assert.equal(row.getAttribute('role'), 'button');
    assert.match(row.getAttribute('aria-label'), /^Open details of DEMO-/);
  }
  // one chip per status that occurs, in a fixed order
  assert.deepEqual(bar.querySelectorAll('[data-pm-status]').map((c) => c.getAttribute('data-pm-status')), ['done', 'developing', 'stalled', 'blocked', 'ready', 'draft', 'waiting', 'cancelled']);
  assert.equal(ctx.action('clear').hidden, true);
  assert.doesNotMatch(labels(ctx), /\bmap\.(ui|detail)\./, 'no message key is shown instead of text');
});

test('the same layer speaks Chinese when the page does', () => {
  const ctx = boot({ lang: 'zh-CN' });
  const bar = ctx.container.querySelector('.pm-toolbar');
  assert.match(bar.textContent, /全部折叠/);
  assert.match(bar.textContent, /适应宽度/);
  assert.match(ctx.card('DEMO-A-004').getAttribute('aria-label'), /按 Enter 查看详情/);
  click(ctx.card('DEMO-A-004'));
  const panel = ctx.container.querySelector('.pm-drawer');
  assert.equal(panel.hidden, false);
  assert.match(panel.textContent, /概要/);
  assert.match(panel.textContent, /官方接口未提供/);
  assert.doesNotMatch(labels(ctx), /\bmap\.(ui|detail)\./);
});

// ---------------------------------------------------------------------- details panel

test('clicking a card opens its details; Escape closes them and gives the focus back; a second Escape lets go of the highlight', () => {
  const ctx = boot();
  const card = ctx.card('DEMO-A-004');
  card.focus();
  click(card);
  const drawer = ctx.container.querySelector('.pm-drawer');
  assert.equal(drawer.hidden, false);
  assert.equal(ctx.app.state.drawer, 'DEMO-A-004');
  assert.equal(drawer.querySelector('#pm-drawer-title').textContent, source.nodes.find((n) => n.key === 'DEMO-A-004').title);
  assert.equal(drawer.querySelector('.pm-drawer-panel').getAttribute('aria-labelledby'), 'pm-drawer-title');
  assert.ok(ctx.container.classList.contains('pm-drawer-open'));
  same(ctx.doc.activeElement, drawer.querySelector('.pm-drawer-close'), 'the focus moves into the panel');

  const first = dom.press(ctx.doc.body, 'Escape');
  assert.equal(first.defaultPrevented, true);
  assert.equal(drawer.hidden, true);
  assert.equal(ctx.app.state.drawer, null);
  assert.ok(!ctx.container.classList.contains('pm-drawer-open'));
  same(ctx.doc.activeElement, card, 'the focus returns to the card that opened the panel');
  assert.equal(ctx.app.state.selected, 'DEMO-A-004', 'the highlighted chain stays after the panel closes');

  dom.press(ctx.doc.body, 'Escape');
  assert.equal(ctx.app.state.selected, null);
  assert.ok(![...ctx.container.querySelectorAll('.pm-card')].some((c) => has(c, 'pm-selected') || has(c, 'pm-up') || has(c, 'pm-down')));
});

test('the close button closes the panel, and clicking another card switches it without closing', () => {
  const ctx = boot();
  click(ctx.card('DEMO-A-004'));
  click(ctx.card('DEMO-A-008'));
  assert.equal(ctx.app.state.drawer, 'DEMO-A-008');
  assert.match(ctx.container.querySelector('.pm-drawer-body').textContent, /DEMO-A-008/);
  click(ctx.container.querySelector('.pm-drawer-close'));
  assert.equal(ctx.container.querySelector('.pm-drawer').hidden, true);
});

test('Enter and Space on a focused card, fold or row work like a click and do not scroll the page', () => {
  const ctx = boot();
  const card = ctx.card('DEMO-A-004');
  for (const key of ['Enter', ' ']) {
    const event = dom.press(card, key);
    assert.equal(event.defaultPrevented, true, `${JSON.stringify(key)} must not scroll`);
    assert.equal(ctx.app.state.drawer, 'DEMO-A-004');
    ctx.app.closeDetails();
  }
  const head = ctx.fold('[data-lane="ALPHA"]');
  dom.press(head, 'Enter');
  assert.equal(ctx.app.state.fold.overview.collapsed.has('ALPHA'), true);
  const row = ctx.container.querySelector('tr[data-key="DEMO-B-002"]');
  dom.press(row, 'Enter');
  assert.equal(ctx.app.state.drawer, 'DEMO-B-002');
  dom.press(ctx.doc.body, 'Enter');
  assert.equal(ctx.app.state.drawer, 'DEMO-B-002', 'Enter elsewhere does nothing');
});

test('a row of the table opens the same details as the card', () => {
  const ctx = boot();
  click(ctx.container.querySelector('tr[data-key="DEMO-A-004"] td.k'));
  const fromRow = ctx.container.querySelector('.pm-drawer-body').innerHTML;
  ctx.app.closeDetails();
  click(ctx.card('DEMO-A-004'));
  assert.equal(ctx.container.querySelector('.pm-drawer-body').innerHTML, fromRow);
  assert.ok(has(ctx.container.querySelector('tr[data-key="DEMO-A-004"]'), 'pm-selected'));
});

test('the panel lists prerequisites and dependents as buttons; following one opens that item and shows it in the graph', () => {
  const ctx = boot();
  click(ctx.card('DEMO-A-004'));
  const body = ctx.container.querySelector('.pm-drawer-body');
  const jumps = body.querySelectorAll('[data-pm-jump]').map((b) => b.getAttribute('data-pm-jump'));
  assert.deepEqual(jumps.slice(0, 2), ['DEMO-A-002', 'DEMO-A-003'], 'the prerequisites come first');
  assert.ok(jumps.includes('DEMO-A-005'));
  // DEMO-A-002 is finished work, folded away in the overview: following the link unfolds it
  absent(ctx.card('DEMO-A-002'));
  click(body.querySelector('[data-pm-jump="DEMO-A-002"]'));
  assert.equal(ctx.app.state.drawer, 'DEMO-A-002');
  assert.equal(ctx.app.state.fold.overview.doneOpen.has('ALPHA'), true);
  const target = ctx.card('DEMO-A-002');
  assert.ok(target, 'the finished work was unfolded to show the item');
  same(ctx.doc.scrolled.at(-1).el, target);
  assert.ok(has(target, 'pm-pulse'));
  ctx.timers.run();
  assert.ok(!has(target, 'pm-pulse'), 'the pulse ends');
});

test('the keyboard returns to the card that opened the panel even when the diagram was drawn again meanwhile', () => {
  const ctx = boot();
  const first = ctx.card('DEMO-A-004');
  first.focus();
  dom.press(first, 'Enter');
  click(ctx.container.querySelector('[data-pm-jump="DEMO-A-002"]')); // unfolds finished work: the overview is drawn again
  assert.ok(!ctx.section('overview').contains(first), 'the card that opened the panel is gone from the page');
  dom.press(ctx.doc.body, 'Escape');
  const back = ctx.doc.activeElement;
  assert.equal(back.getAttribute('data-key'), 'DEMO-A-004');
  assert.ok(ctx.section('overview').contains(back), 'focus is on the card as drawn now');
  // a row of the table gets the keyboard back too
  const row = ctx.container.querySelector('tr[data-key="DEMO-B-002"]');
  row.focus();
  dom.press(row, 'Enter');
  dom.press(ctx.doc.body, 'Escape');
  same(ctx.doc.activeElement, row);
});

test('"Show in graph" says so when the active view does not draw the item', () => {
  const ctx = boot();
  ctx.tab('explore');
  ctx.app.openDetails('DEMO-C-001'); // no milestone lane: the milestone view draws nothing for it
  absent(ctx.card('DEMO-C-001'));
  click(ctx.container.querySelector('[data-pm-action="reveal"]'));
  assert.equal(ctx.container.querySelector('[data-pm-status-line]').textContent, 'This item is not drawn in the current view; its details are shown here.');
  ctx.tab('overview');
  click(ctx.container.querySelector('[data-pm-action="reveal"]'));
  assert.equal(ctx.container.querySelector('[data-pm-status-line]').textContent, '', 'the message goes away once the item can be shown');
  assert.ok(ctx.card('DEMO-C-001'));
});

test('the panel starts below the host bar and follows its size when the window changes', () => {
  let bar = 57.4;
  const ctx = boot({ offsetTop: () => bar });
  const drawer = ctx.container.querySelector('.pm-drawer');
  assert.equal(drawer.style.getPropertyValue('--pm-top'), '', 'nothing is set before the panel opens');
  click(ctx.card('DEMO-A-004'));
  assert.equal(drawer.style.getPropertyValue('--pm-top'), '57px');
  bar = 96;
  dom.fire(ctx.win, 'resize');
  assert.equal(drawer.style.getPropertyValue('--pm-top'), '96px');
  ctx.app.closeDetails();
  bar = 10;
  dom.fire(ctx.win, 'resize');
  assert.equal(drawer.style.getPropertyValue('--pm-top'), '96px', 'a closed panel is left alone');
  const plain = boot();
  click(plain.card('DEMO-A-004'));
  assert.equal(plain.container.querySelector('.pm-drawer').style.getPropertyValue('--pm-top'), '', 'without a host bar nothing is set');
});

test('the clipboard of the window is used when no copy function is given', async () => {
  const ctx = boot({ noCopy: true });
  click(ctx.card('DEMO-A-004'));
  click(ctx.container.querySelector('[data-pm-action="copy-command"]'));
  assert.deepEqual(ctx.copied, ['awr work show DEMO-A-004']);
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(ctx.container.querySelector('[data-pm-status-line]').textContent, 'Copied');
});

test('the panel copies the key and the awr command', async () => {
  const ctx = boot();
  click(ctx.card('DEMO-A-004'));
  click(ctx.container.querySelector('[data-pm-action="copy-key"]'));
  click(ctx.container.querySelector('[data-pm-action="copy-command"]'));
  assert.deepEqual(ctx.copied, ['DEMO-A-004', 'awr work show DEMO-A-004']);
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(ctx.container.querySelector('[data-pm-status-line]').textContent, 'Copied');
});

// ---------------------------------------------------------------------- highlight

/**
 * The edge rule written out independently of the implementation, as a statement about both ends: an edge is on the chain when
 * it lies within the upstream cone of the item (both ends upstream, or the item itself; the block of collapsed finished work
 * counts as upstream of whatever it feeds) or within its downstream cone.
 */
function onChain(key, around, from, to) {
  const upstreamEnd = (k) => k === key || around.up.has(k);
  const downstreamEnd = (k) => k === key || around.down.has(k);
  const startsUpstream = from === 'BLOCK' ? upstreamEnd(to) : around.up.has(from);
  return (startsUpstream && upstreamEnd(to)) || (downstreamEnd(from) && around.down.has(to));
}

test('hovering or focusing a card highlights its chain through every level and dims everything else', () => {
  const ctx = boot();
  ctx.tab('mainline');
  const model = ctx.app.model;
  const classes = (k) => ['pm-self', 'pm-up', 'pm-down', 'pm-dim'].filter((c) => has(ctx.card(k), c));
  const edges = ctx.section('mainline').querySelectorAll('[data-from]');
  assert.ok(edges.length > 5);
  let deepest = 0;
  // items in different places of the graph: a middle one, one with a wide downstream, one behind cancelled work, one in another lane
  for (const key of ['DEMO-A-005', 'DEMO-A-004', 'DEMO-A-002', 'DEMO-A-012', 'DEMO-B-003']) {
    const around = detail.neighborhood(model, key);
    assert.ok(around.up.size + around.down.size > 0, key);
    dom.fire(hit(ctx.card(key)), 'mouseover');
    assert.deepEqual(classes(key), ['pm-self'], key);
    for (const k of ctx.cards()) {
      if (k === key) continue;
      const expected = around.up.has(k) ? ['pm-up'] : around.down.has(k) ? ['pm-down'] : ['pm-dim'];
      assert.deepEqual(classes(k), expected, `${key}: ${k}`);
    }
    for (const edge of edges) {
      const from = edge.getAttribute('data-from');
      const to = edge.getAttribute('data-to');
      assert.equal(has(edge, 'pm-hl'), onChain(key, around, from, to), `${key}: ${from} -> ${to}`);
      assert.equal(has(edge, 'pm-dim'), !onChain(key, around, from, to), `${key}: ${from} -> ${to}`);
    }
    assert.ok(edges.some((e) => has(e, 'pm-hl')), key);
    assert.match(ctx.container.querySelector('[data-pm-note]').textContent, new RegExp(`^Highlighting ${key}: ${around.up.size} upstream, ${around.down.size} downstream$`));
    deepest = Math.max(deepest, around.down.size, around.up.size);
    dom.fire(hit(ctx.card(key)), 'mouseout', { relatedTarget: ctx.doc.body });
    assert.deepEqual(ctx.cards().filter((k) => classes(k).length), [], `${key}: leaving clears everything`);
    assert.ok(edges.every((e) => !has(e, 'pm-hl') && !has(e, 'pm-dim')), key);
  }
  assert.ok(deepest >= 4, 'the sample covers chains of several levels in both directions');
  assert.match(ctx.container.querySelector('[data-pm-note]').textContent, /^Click a card for details/);
  // the keyboard gets the same highlight
  ctx.card('DEMO-A-005').focus();
  assert.deepEqual(classes('DEMO-A-005'), ['pm-self']);
  assert.ok(ctx.cards().some((k) => classes(k).includes('pm-up')));
  ctx.doc.setActive(null);
  assert.deepEqual(classes('DEMO-A-005'), []);
});

test('an item without any chain is ringed but does not dim the rest of the diagram', () => {
  const ctx = boot();
  const lonely = ctx.card('DEMO-A-010'); // a draft with no prerequisite and no dependent
  const around = detail.neighborhood(ctx.app.model, 'DEMO-A-010');
  assert.equal(around.up.size + around.down.size, 0);
  dom.fire(hit(lonely), 'mouseover');
  assert.ok(has(lonely, 'pm-self'));
  assert.equal(ctx.container.querySelectorAll('.pm-dim').length, 0, 'nothing is dimmed when there is no chain to set apart');
  assert.match(ctx.container.querySelector('[data-pm-note]').textContent, /^Highlighting DEMO-A-010: 0 upstream, 0 downstream$/);
});

test('the smaller fold controls are named after their lane and what they say', () => {
  const ctx = boot({ snapshot: grown() });
  const name = (selector) => ctx.fold(selector).getAttribute('aria-label');
  assert.equal(name('[data-lane="ALPHA"]'), `Fold or unfold ${h.readJson('config.json').overview.lanes[0].name}`);
  assert.match(name('[data-group="ALPHA|waiting"]'), /^Fold or unfold .+ \u00b7 8 more \u00b7 Waiting$/);
  assert.match(name('[data-done-lane="ALPHA"]'), /^Fold or unfold .+ \u00b7 3 done \u00b7 A 3$/);
  click(ctx.fold('[data-group="ALPHA|waiting"]'));
  assert.match(name('[data-group="ALPHA|waiting"]'), /^Fold or unfold .+ \u00b7 Show fewer$/);
  ctx.tab('explore');
  assert.equal(name('[data-xlane="1"]'), `Fold or unfold ${h.readJson('config.json').explore_lanes[1].name}`);
});

test('the block that stands for finished work is on the chain of whatever it feeds', () => {
  const ctx = boot({ snapshot: quiet() });
  ctx.tab('mainline');
  const blockEdges = () => ctx.section('mainline').querySelectorAll('[data-from="BLOCK"]');
  assert.ok(blockEdges().length >= 2, 'the collapsed finished work of lane A feeds two items');
  const fed = blockEdges().map((e) => e.getAttribute('data-to')).sort();
  assert.deepEqual(fed, ['DEMO-A-002', 'DEMO-A-003']);
  dom.fire(hit(ctx.card('DEMO-A-005')), 'mouseover'); // downstream of both
  assert.ok(blockEdges().every((e) => has(e, 'pm-hl') && !has(e, 'pm-dim')));
  dom.fire(hit(ctx.card('DEMO-A-005')), 'mouseout', { relatedTarget: ctx.doc.body });
  dom.fire(hit(ctx.card('DEMO-A-007')), 'mouseover'); // fed by DEMO-A-003 only
  const on = blockEdges().filter((e) => has(e, 'pm-hl')).map((e) => e.getAttribute('data-to'));
  assert.deepEqual(on, ['DEMO-A-003']);
  assert.ok(blockEdges().filter((e) => e.getAttribute('data-to') === 'DEMO-A-002').every((e) => has(e, 'pm-dim')));
});

test('moving the pointer between two parts of one card does not flicker; a selection pins the highlight', () => {
  const ctx = boot();
  const card = ctx.card('DEMO-A-004');
  const parts = card.querySelectorAll('text');
  dom.fire(parts[0], 'mouseover');
  dom.fire(parts[0], 'mouseout', { relatedTarget: parts[1] });
  assert.equal(ctx.app.state.hover, 'DEMO-A-004');
  click(card);
  dom.fire(hit(ctx.card('DEMO-A-008')), 'mouseover');
  assert.equal(has(ctx.card('DEMO-A-004'), 'pm-self'), true, 'hovering another card does not take over a selection');
  assert.equal(has(ctx.card('DEMO-A-008'), 'pm-self'), false);
});

test('clicking an empty spot of the diagram lets go of the highlight, unless the panel is open', () => {
  const ctx = boot();
  ctx.app.select('DEMO-A-004');
  const svg = ctx.section('overview').querySelector('svg');
  click(ctx.card('DEMO-A-004'));
  dom.fire(svg, 'click');
  assert.equal(ctx.app.state.selected, 'DEMO-A-004', 'an open panel keeps its item selected');
  ctx.app.closeDetails();
  dom.fire(svg, 'click');
  assert.equal(ctx.app.state.selected, null);
});

// ---------------------------------------------------------------------- search and filter

test('search dims what does not match, counts matches and Enter walks through them', () => {
  const ctx = boot();
  const input = ctx.container.querySelector('[data-pm-search]');
  const type = (value) => { input.value = value; dom.fire(input, 'input'); };
  type('demo-a-00');
  const expected = source.nodes.filter((n) => /demo-a-00/i.test(n.key)).length;
  assert.equal(ctx.container.querySelector('[data-pm-count]').textContent, `${expected} matches`);
  const drawn = ctx.cards('overview');
  for (const k of drawn) assert.equal(has(ctx.card(k, 'overview'), 'pm-match'), /^DEMO-A-00/.test(k), k);
  for (const k of drawn) assert.equal(has(ctx.card(k, 'overview'), 'pm-dim'), !/^DEMO-A-00/.test(k), k);
  assert.equal(ctx.action('clear').hidden, false);

  const matched = ctx.section('overview').querySelectorAll('.pm-card.pm-match');
  const walked = [];
  for (let i = 0; i < matched.length + 1; i++) {
    dom.press(input, 'Enter');
    walked.push(ctx.doc.scrolled.at(-1).el.getAttribute('data-key'));
  }
  assert.deepEqual(walked.slice(0, matched.length), matched.map((c) => c.getAttribute('data-key')));
  assert.equal(walked.at(-1), walked[0], 'the walk wraps around');

  type('no-such-thing');
  assert.equal(ctx.container.querySelector('[data-pm-count]').textContent, 'No match');
  assert.ok(ctx.cards('overview').every((k) => has(ctx.card(k, 'overview'), 'pm-dim')));
  type('');
  assert.equal(ctx.container.querySelector('[data-pm-count]').textContent, '');
  assert.ok(ctx.cards('overview').every((k) => !has(ctx.card(k, 'overview'), 'pm-dim')));
  assert.equal(ctx.action('clear').hidden, true);
});

test('search finds a match inside finished work and Enter unfolds it', () => {
  const ctx = boot();
  const input = ctx.container.querySelector('[data-pm-search]');
  input.value = 'demo-a-001';
  dom.fire(input, 'input');
  absent(ctx.card('DEMO-A-001'), 'finished work is folded away in the overview');
  dom.press(input, 'Enter');
  assert.ok(ctx.card('DEMO-A-001'));
  assert.ok(has(ctx.card('DEMO-A-001'), 'pm-match'));
});

test('the status chips filter by status, several at once, and the clear button resets everything', () => {
  const ctx = boot();
  const chip = (vis) => ctx.container.querySelector(`[data-pm-status="${vis}"]`);
  click(chip('blocked'));
  assert.equal(chip('blocked').getAttribute('aria-pressed'), 'true');
  const visOf = (k) => ctx.card(k, 'overview').getAttribute('data-vis');
  for (const k of ctx.cards('overview')) assert.equal(has(ctx.card(k, 'overview'), 'pm-dim'), visOf(k) !== 'blocked', k);
  click(chip('stalled'));
  for (const k of ctx.cards('overview')) assert.equal(has(ctx.card(k, 'overview'), 'pm-dim'), !['blocked', 'stalled'].includes(visOf(k)), k);
  click(ctx.action('clear'));
  assert.equal(chip('blocked').getAttribute('aria-pressed'), 'false');
  assert.ok(ctx.cards('overview').every((k) => !has(ctx.card(k, 'overview'), 'pm-dim')));
});

test('an executor chip shows only that agent\'s work, and pressing it again lets go', () => {
  const ctx = boot();
  const chips = ctx.section('overview').querySelectorAll('.pm-agent');
  assert.deepEqual(chips.map((c) => c.getAttribute('data-agent')).sort(), ['demo-agent-1', 'demo-agent-3'], 'one chip per agent with an active claim on unfinished work');
  for (const chip of chips) {
    assert.equal(chip.getAttribute('role'), 'button');
    assert.equal(chip.getAttribute('tabindex'), '0');
    assert.equal(chip.getAttribute('aria-label'), `Show only the work of ${chip.getAttribute('data-agent')}`);
    assert.equal(chip.getAttribute('aria-pressed'), 'false');
  }
  const chip = chips.find((c) => c.getAttribute('data-agent') === 'demo-agent-1');
  click(chip);
  const input = ctx.container.querySelector('[data-pm-search]');
  assert.equal(input.value, 'demo-agent-1');
  assert.equal(ctx.app.state.search, 'demo-agent-1');
  const owned = ctx.app.model.nodes;
  for (const k of ctx.cards('overview')) assert.equal(has(ctx.card(k, 'overview'), 'pm-dim'), owned.get(k).owner !== 'demo-agent-1', k);
  assert.ok(has(ctx.section('overview').querySelector('.pm-agent[data-agent="demo-agent-1"]'), 'pm-on'));
  assert.equal(ctx.section('overview').querySelector('.pm-agent[data-agent="demo-agent-1"]').getAttribute('aria-pressed'), 'true');
  assert.equal(ctx.section('overview').querySelector('.pm-agent[data-agent="demo-agent-3"]').getAttribute('aria-pressed'), 'false');
  assert.equal(ctx.container.querySelector('[data-pm-count]').textContent, '1 matches');
  click(chip);
  assert.equal(input.value, '');
  assert.equal(ctx.app.state.search, '');
  assert.ok(ctx.cards('overview').every((k) => !has(ctx.card(k, 'overview'), 'pm-dim')));
  // from the keyboard, and the clear button lets go as well
  dom.press(chip, ' ');
  assert.equal(ctx.app.state.search, 'demo-agent-1');
  click(ctx.action('clear'));
  assert.equal(ctx.section('overview').querySelector('.pm-agent[data-agent="demo-agent-1"]').getAttribute('aria-pressed'), 'false');
  assert.ok(!has(ctx.section('overview').querySelector('.pm-agent[data-agent="demo-agent-1"]'), 'pm-on'));
  // a typed search that equals no agent presses no chip
  input.value = 'demo';
  dom.fire(input, 'input');
  assert.ok(chips.every((c) => !has(ctx.section('overview').querySelector(`.pm-agent[data-agent="${c.getAttribute('data-agent')}"]`), 'pm-on')));
});

test('the table under the diagrams follows the search and the status filter', () => {
  const ctx = boot();
  const rows = () => ctx.container.querySelectorAll('tr[data-key]');
  const dimmed = () => rows().filter((r) => has(r, 'pm-dim')).map((r) => r.getAttribute('data-key'));
  assert.deepEqual(dimmed(), []);
  const input = ctx.container.querySelector('[data-pm-search]');
  input.value = 'demo-b-00';
  dom.fire(input, 'input');
  assert.deepEqual(rows().filter((r) => !has(r, 'pm-dim')).map((r) => r.getAttribute('data-key')), ['DEMO-B-001', 'DEMO-B-002', 'DEMO-B-003', 'DEMO-B-004']);
  click(ctx.action('clear'));
  assert.deepEqual(dimmed(), []);
  click(ctx.container.querySelector('[data-pm-status="blocked"]'));
  assert.deepEqual(rows().filter((r) => !has(r, 'pm-dim')).map((r) => r.getAttribute('data-key')), ['DEMO-A-008']);
  click(ctx.container.querySelector('[data-pm-status="cancelled"]'));
  assert.deepEqual(rows().filter((r) => !has(r, 'pm-dim')).map((r) => r.getAttribute('data-key')), ['DEMO-A-008', 'DEMO-A-011']);
});

test('"/" focuses the search field', () => {
  const ctx = boot();
  const event = dom.press(ctx.card('DEMO-A-004'), '/');
  assert.equal(event.defaultPrevented, true);
  assert.equal(ctx.doc.activeElement, ctx.container.querySelector('[data-pm-search]'));
  const typed = dom.press(ctx.container.querySelector('[data-pm-search]'), '/');
  assert.equal(typed.defaultPrevented, false, 'a slash typed into the field stays a slash');
});

// ---------------------------------------------------------------------- folding

test('folding a lane header redraws the overview without that lane\'s cards and unfolding restores it exactly', () => {
  const ctx = boot();
  const before = ctx.section('overview').innerHTML;
  const head = ctx.fold('[data-lane="ALPHA"]');
  assert.equal(head.getAttribute('aria-expanded'), 'true');
  head.focus();
  click(head);
  const folded = ctx.fold('[data-lane="ALPHA"]');
  assert.equal(folded.getAttribute('aria-expanded'), 'false');
  assert.ok(ctx.cards('overview').every((k) => !k.startsWith('DEMO-A-')), 'no card of the folded lane is drawn');
  assert.ok(ctx.cards('overview').some((k) => k.startsWith('DEMO-B-')), 'the other lanes are untouched');
  assert.match(ctx.section('overview').textContent, /14 items hidden/);
  same(ctx.doc.activeElement, folded, 'the keyboard stays on the header although it was drawn again');
  assert.match(folded.getAttribute('aria-label'), /^Fold or unfold /);
  click(folded);
  assert.equal(ctx.section('overview').innerHTML, before, 'unfolding gives back exactly the markup the page started with');
  assert.equal(ctx.section('mainline').querySelectorAll('.pm-card').length, 18, 'the other views are not drawn again');
});

test('finished work in a lane unfolds into rows and folds again', () => {
  const ctx = boot();
  absent(ctx.card('DEMO-A-001'));
  click(ctx.fold('[data-done-lane="ALPHA"]'));
  assert.deepEqual(['DEMO-A-001', 'DEMO-A-002', 'DEMO-A-003'].map((k) => Boolean(ctx.card(k))), [true, true, true]);
  assert.equal(ctx.fold('[data-done-lane="ALPHA"]').getAttribute('aria-expanded'), 'true');
  click(ctx.card('DEMO-A-001'));
  assert.equal(ctx.app.state.drawer, 'DEMO-A-001', 'the unfolded rows are cards too');
  click(ctx.fold('[data-done-lane="ALPHA"]'));
  absent(ctx.card('DEMO-A-001'));
});

test('a group of more cards than fit folds into "N more" and opens from there', () => {
  const snapshot = grown();
  const ctx = boot({ snapshot });
  const waiting = [...modelLib.loadModel(snapshot, config).nodes.values()].filter((n) => n.lane === 'ALPHA' && n.vis === 'waiting').map((n) => n.id);
  const group = 'ALPHA|waiting';
  assert.ok(waiting.length > 5);
  assert.ok(ctx.app.targets.overviewGroups.includes(group));
  const drawn = () => ctx.cards('overview').filter((k) => waiting.includes(k));
  const before = drawn().length;
  assert.ok(before > 0 && before < waiting.length, 'some of the group is drawn, the rest is behind the block');
  const more = ctx.fold(`[data-group="${group}"]`);
  assert.equal(more.getAttribute('aria-expanded'), 'false');
  assert.ok(has(more, 'pm-fold'));
  assert.match(more.textContent, new RegExp(`${waiting.length - before} more`));
  click(more);
  assert.equal(drawn().length, waiting.length, 'every card of the group is drawn');
  const fewer = ctx.fold(`[data-group="${group}"]`);
  assert.ok(has(fewer, 'pm-btn'));
  assert.equal(fewer.getAttribute('aria-expanded'), 'true');
  click(fewer);
  assert.equal(drawn().length, before);
});

test('"Show in graph" opens the group that hides an item', () => {
  const ctx = boot({ snapshot: grown() });
  absent(ctx.card('DEMO-A-022'));
  ctx.app.openDetails('DEMO-A-022');
  click(ctx.container.querySelector('[data-pm-action="reveal"]'));
  assert.ok(ctx.card('DEMO-A-022'));
  assert.equal(ctx.app.state.fold.overview.groupsOpen.has('ALPHA|waiting'), true);
});

test('dependency panels fold, and each panel opens its finished work', () => {
  const ctx = boot({ snapshot: quiet() });
  ctx.tab('mainline');
  const panelCards = (prefix) => ctx.cards('mainline').filter((k) => k.startsWith(prefix));
  assert.ok(panelCards('DEMO-A-').length > 5);
  assert.ok(!panelCards('DEMO-A-').includes('DEMO-A-001'), 'finished work that unlocks nothing open is a block, not a card');
  const toggle = ctx.fold('[data-panel-done="ALPHA"]');
  assert.equal(toggle.getAttribute('aria-expanded'), 'false');
  click(toggle);
  assert.ok(panelCards('DEMO-A-').includes('DEMO-A-001'));
  assert.equal(ctx.fold('[data-panel-done="ALPHA"]').getAttribute('aria-expanded'), 'true');
  click(ctx.fold('[data-panel="ALPHA"]'));
  assert.deepEqual(panelCards('DEMO-A-'), [], 'a folded panel draws no cards');
  assert.ok(panelCards('DEMO-B-').length > 0, 'the other panel stays');
  assert.equal(ctx.fold('[data-panel="ALPHA"]').getAttribute('aria-expanded'), 'false');
  assert.match(ctx.section('mainline').textContent, /unfinished/);
  click(ctx.fold('[data-panel="ALPHA"]'));
  assert.ok(panelCards('DEMO-A-').includes('DEMO-A-001'), 'the panel comes back as it was left');
  click(ctx.fold('[data-panel-done="ALPHA"]'));
  assert.ok(!panelCards('DEMO-A-').includes('DEMO-A-001'));
});

test('milestone lanes fold and show finished work from the toolbar', () => {
  const ctx = boot();
  ctx.tab('explore');
  const keys = () => ctx.cards('explore');
  const lane1 = keys().filter((k) => source.nodes.find((n) => n.key === k).milestone === 'M1');
  assert.ok(lane1.length > 1);
  click(ctx.fold('[data-xlane="0"]'));
  assert.ok(keys().every((k) => source.nodes.find((n) => n.key === k).milestone !== 'M1'), 'the folded lane draws no cards');
  assert.equal(ctx.fold('[data-xlane="0"]').getAttribute('aria-expanded'), 'false');
  click(ctx.fold('[data-xlane="0"]'));
  assert.deepEqual(keys().filter((k) => lane1.includes(k)), lane1);
  assert.ok(!keys().includes('DEMO-A-001'));
  click(ctx.action('toggle-done'));
  assert.ok(keys().includes('DEMO-A-001'), 'finished work of the lanes is drawn');
  assert.equal(ctx.action('toggle-done').getAttribute('aria-pressed'), 'true');
  assert.equal(ctx.action('toggle-done').textContent, 'Hide finished work');
  click(ctx.action('toggle-done'));
  assert.ok(!keys().includes('DEMO-A-001'));
});

test('fold all, unfold all and show finished work act on the active view and follow the tabs', () => {
  const ctx = boot({ snapshot: grown() });
  const open = () => ctx.section(ctx.app.state.view).querySelectorAll('[aria-expanded]').filter((e) => e.getAttribute('data-lane') || e.getAttribute('data-panel') || e.getAttribute('data-xlane')).map((e) => e.getAttribute('aria-expanded'));
  assert.deepEqual(open(), ['true', 'true', 'true']);
  click(ctx.action('collapse-all'));
  assert.deepEqual(open(), ['false', 'false', 'false']);
  assert.equal(ctx.cards('overview').length, 0);
  click(ctx.action('expand-all'));
  assert.deepEqual(open(), ['true', 'true', 'true']);
  assert.equal(ctx.app.state.fold.overview.groupsOpen.has('ALPHA|waiting'), true, 'unfold all opens the "N more" blocks too');
  click(ctx.action('toggle-done'));
  assert.ok(ctx.card('DEMO-A-001') && ctx.card('DEMO-B-001') && ctx.card('DEMO-C-002'));
  assert.equal(ctx.action('toggle-done').textContent, 'Hide finished work');
  ctx.tab('mainline');
  assert.equal(ctx.action('toggle-done').textContent, 'Show finished work', 'the button follows the view that is on screen');
  click(ctx.action('collapse-all'));
  assert.deepEqual(open(), ['false', 'false']);
  assert.equal(ctx.app.state.fold.overview.collapsed.size, 0, 'the other views keep their own folds');
});

test('buttons that have nothing to act on are disabled', () => {
  const noLanes = { ...config, explore_lanes: [] };
  const ctx = boot({ cfg: noLanes });
  assert.equal(ctx.action('collapse-all').hasAttribute('disabled'), false);
  ctx.tab('explore');
  assert.equal(ctx.action('collapse-all').hasAttribute('disabled'), true);
  assert.equal(ctx.action('expand-all').hasAttribute('disabled'), true);
  assert.equal(ctx.action('toggle-done').hasAttribute('disabled'), true);
  ctx.tab('overview');
  assert.equal(ctx.action('collapse-all').hasAttribute('disabled'), false);
});

// ---------------------------------------------------------------------- zoom and pan

const size = (ctx, id) => { const svg = ctx.section(id).querySelector('svg'); return [Number(svg.getAttribute('width')), Number(svg.getAttribute('height'))]; };
const box = (ctx, id) => ctx.section(id).querySelector('svg').getAttribute('viewBox').split(' ').map(Number);

test('zoom buttons scale the drawing of the active view only and never change its viewBox', () => {
  const ctx = boot();
  const [, , w0, h0] = box(ctx, 'overview');
  assert.deepEqual(size(ctx, 'overview'), [w0, h0]);
  click(ctx.action('zoom-in'));
  assert.deepEqual(size(ctx, 'overview'), [Math.round(w0 * 1.1), Math.round(h0 * 1.1)]);
  click(ctx.action('zoom-out'));
  click(ctx.action('zoom-out'));
  assert.deepEqual(size(ctx, 'overview'), [Math.round(w0 * 0.9), Math.round(h0 * 0.9)]);
  assert.deepEqual(box(ctx, 'overview').slice(2), [w0, h0]);
  const [, , wm, hm] = box(ctx, 'mainline');
  assert.deepEqual(size(ctx, 'mainline'), [wm, hm], 'another view keeps its own zoom');
  ctx.tab('mainline');
  click(ctx.action('zoom-out'));
  assert.deepEqual(size(ctx, 'mainline'), [Math.round(wm * 0.9), Math.round(hm * 0.9)], 'the buttons act on the view that is on screen');
  assert.deepEqual(size(ctx, 'overview'), [Math.round(w0 * 0.9), Math.round(h0 * 0.9)], 'and leave the other views alone');
  click(ctx.action('zoom-in'));
  click(ctx.action('zoom-in'));
  assert.deepEqual(size(ctx, 'mainline'), [Math.round(wm * 1.1), Math.round(hm * 1.1)]);
  assert.deepEqual(size(ctx, 'overview'), [Math.round(w0 * 0.9), Math.round(h0 * 0.9)]);
  ctx.tab('overview');
  click(ctx.action('zoom-100'));
  assert.deepEqual(size(ctx, 'overview'), [w0, h0]);
  assert.deepEqual(size(ctx, 'mainline'), [Math.round(wm * 1.1), Math.round(hm * 1.1)], 'zoom survives a switch of tabs');
  for (let i = 0; i < 20; i++) click(ctx.action('zoom-out'));
  assert.equal(ctx.app.state.zoom.overview, 0.25, 'zoom has a lower limit');
  for (let i = 0; i < 30; i++) click(ctx.action('zoom-in'));
  assert.equal(ctx.app.state.zoom.overview, 3, 'and an upper limit');
});

test('fit width scales the drawing to the room the view has', () => {
  const ctx = boot();
  const [, , w0] = box(ctx, 'overview');
  ctx.section('overview').clientWidth = Math.round(w0 / 2);
  click(ctx.action('zoom-fit'));
  assert.ok(Math.abs(ctx.app.state.zoom.overview - 0.5) < 0.011);
  assert.equal(size(ctx, 'overview')[0], Math.round(w0 * ctx.app.state.zoom.overview));
  ctx.section('overview').clientWidth = w0 * 3;
  click(ctx.action('zoom-fit'));
  assert.ok(ctx.app.state.zoom.overview >= 3 - 1e-9 || ctx.app.state.zoom.overview <= 3, 'fit never leaves the zoom limits');
});

test('keys + - 0 zoom, ctrl+wheel zooms around the pointer, a plain wheel is left alone', () => {
  const ctx = boot();
  const svg = ctx.section('overview').querySelector('svg');
  dom.press(ctx.card('DEMO-A-004'), '+');
  assert.equal(ctx.app.state.zoom.overview, 1.1);
  dom.press(ctx.card('DEMO-A-004'), '-');
  dom.press(ctx.card('DEMO-A-004'), '-');
  assert.equal(ctx.app.state.zoom.overview, 0.9);
  dom.press(ctx.card('DEMO-A-004'), '0');
  assert.equal(ctx.app.state.zoom.overview, 1);
  const plain = dom.makeEvent('wheel', { deltaY: -100, clientX: 300 });
  dom.fire(svg, 'wheel', plain);
  assert.equal(plain.defaultPrevented, false);
  assert.equal(ctx.app.state.zoom.overview, 1);
  ctx.section('overview').scrollLeft = 200;
  const zoom = dom.makeEvent('wheel', { deltaY: -100, clientX: 300, ctrlKey: true });
  svg.dispatchEvent(zoom);
  assert.equal(zoom.defaultPrevented, true);
  assert.equal(ctx.app.state.zoom.overview, 1.1);
  assert.equal(Math.round(ctx.section('overview').scrollLeft), 250, 'the point under the pointer stays where it was');
  const input = ctx.container.querySelector('[data-pm-search]');
  dom.press(input, '-');
  assert.equal(ctx.app.state.zoom.overview, 1.1, 'typing in the search field does not zoom');
});

test('dragging the background pans the diagram and the click that ends the drag is swallowed', () => {
  const ctx = boot();
  const scroll = ctx.section('overview');
  const svg = scroll.querySelector('svg');
  scroll.scrollLeft = 400;
  ctx.app.select('DEMO-A-004');
  dom.fire(svg, 'mousedown', { button: 0, clientX: 500, clientY: 300 });
  dom.fire(ctx.doc.body, 'mousemove', { clientX: 470, clientY: 280 });
  assert.equal(scroll.scrollLeft, 430);
  assert.deepEqual(ctx.win.scrolls, [[0, 20]], 'dragging up scrolls the page down by the same distance');
  assert.ok(has(scroll, 'pm-panning'));
  dom.fire(ctx.doc.body, 'mouseup', { clientX: 470, clientY: 280 });
  assert.ok(!has(scroll, 'pm-panning'));
  dom.fire(svg, 'click');
  assert.equal(ctx.app.state.selected, 'DEMO-A-004', 'the click that ends a drag does not let go of the selection');
  dom.fire(svg, 'click');
  assert.equal(ctx.app.state.selected, null, 'the next click is an ordinary click');
  // a press on a card is a click, not a drag
  dom.fire(hit(ctx.card('DEMO-A-004')), 'mousedown', { button: 0, clientX: 10, clientY: 10 });
  dom.fire(ctx.doc.body, 'mousemove', { clientX: 90, clientY: 10 });
  assert.equal(scroll.scrollLeft, 430);
  // movement under the threshold is no drag
  dom.fire(svg, 'mousedown', { button: 0, clientX: 10, clientY: 10 });
  dom.fire(ctx.doc.body, 'mousemove', { clientX: 12, clientY: 11 });
  dom.fire(ctx.doc.body, 'mouseup', {});
  assert.equal(scroll.scrollLeft, 430);
  ctx.timers.run();
});

// ---------------------------------------------------------------------- lifecycle

test('a refresh keeps the folds, zoom, filters, search and tab of the earlier mount', () => {
  const first = boot();
  click(first.fold('[data-lane="BETA"]'));
  click(first.action('zoom-in'));
  click(first.container.querySelector('[data-pm-status="blocked"]'));
  const input = first.container.querySelector('[data-pm-search]');
  input.value = 'demo';
  dom.fire(input, 'input');
  first.tab('mainline');
  click(first.fold('[data-panel="ALPHA"]'));
  const saved = first.app.state;
  first.app.destroy();

  const second = boot({ restore: saved, container: { doc: first.doc, container: first.container } });
  assert.equal(second.app.state.view, 'mainline');
  assert.equal(second.container.querySelector('#v-mainline').checked, true);
  assert.equal(second.fold('[data-panel="ALPHA"]', 'mainline').getAttribute('aria-expanded'), 'false');
  assert.equal(second.fold('[data-lane="BETA"]', 'overview').getAttribute('aria-expanded'), 'false');
  assert.equal(second.app.state.zoom.overview, 1.1);
  assert.equal(second.container.querySelector('[data-pm-search]').value, 'demo');
  assert.equal(second.container.querySelector('[data-pm-status="blocked"]').getAttribute('aria-pressed'), 'true');
  assert.equal(second.container.querySelector('[data-pm-count]').textContent.length > 0, true);
});

test('destroying the layer removes every listener it added', () => {
  const ctx = boot();
  const count = () => ['click', 'keydown', 'input', 'change', 'mouseover', 'mouseout', 'focusin', 'focusout', 'mousedown', 'wheel'].reduce((s, type) => s + ctx.container.listenerCount(type), 0) + ctx.doc.listenerCount('keydown') + ctx.win.listenerCount('resize');
  assert.ok(count() >= 12);
  dom.fire(ctx.section('overview').querySelector('svg'), 'mousedown', { button: 0, clientX: 1, clientY: 1 });
  assert.equal(ctx.doc.listenerCount('mousemove'), 1);
  ctx.app.openDetails('DEMO-A-004');
  click(ctx.card('DEMO-A-004'));
  ctx.app.reveal('DEMO-A-004');
  assert.ok(ctx.timers.pending > 0);
  ctx.app.destroy();
  assert.equal(count(), 0);
  assert.equal(ctx.doc.listenerCount('mousemove'), 0);
  assert.equal(ctx.timers.pending, 0);
  const before = ctx.container.innerHTML;
  click(ctx.card('DEMO-A-008'));
  assert.equal(ctx.container.innerHTML, before, 'a destroyed layer ignores events');
});

test('Escape is ignored while the map is not on screen', () => {
  const ctx = boot();
  ctx.app.openDetails('DEMO-A-004');
  const wrapper = ctx.doc.createElement('section');
  wrapper.setAttribute('hidden', '');
  ctx.doc.body.appendChild(wrapper);
  wrapper.appendChild(ctx.container);
  dom.press(ctx.doc.body, 'Escape');
  assert.equal(ctx.app.state.drawer, 'DEMO-A-004');
});
