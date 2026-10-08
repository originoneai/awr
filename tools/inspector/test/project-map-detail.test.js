/**
 * Project map details and fold state, without any DOM: which items form a dependency chain, what the details panel says
 * about an item (everything from the snapshot, nothing guessed), that text from the project cannot inject markup, and that
 * the views are pure functions of their fold state.
 *
 * Run: node --test test/project-map-detail.test.js
 */

'use strict';

const { test } = require('node:test');
const assert = require('node:assert/strict');
const h = require('./fixtures/project-map/helpers.js');
const snapshotLib = require('../public/project-map/snapshot.js');
const modelLib = require('../public/project-map/model.js');
const viewsLib = require('../public/project-map/views.js');
const detail = require('../public/project-map/detail.js');
const page = require('../public/project-map/page.js');

const source = h.readJson('states.json');
const config = h.readJson('config.json');
const model = modelLib.loadModel(source, config);
const sorted = (set) => [...set].sort();

// ---------------------------------------------------------------------- dependency chains

test('the dependency index lists declared prerequisites and dependents, sorted', () => {
  const index = detail.dependencyIndex(model);
  assert.deepEqual(index.up.get('DEMO-A-004'), ['DEMO-A-002', 'DEMO-A-003']);
  assert.deepEqual(index.down.get('DEMO-A-002'), ['DEMO-A-004', 'DEMO-A-012', 'DEMO-B-003']);
  assert.deepEqual(index.down.get('DEMO-A-006'), []);
  assert.deepEqual(index.up.get('DEMO-A-010'), []);
});

test('a chain reaches every level upstream and downstream, and only those', () => {
  const around = detail.neighborhood(model, 'DEMO-A-005');
  assert.deepEqual(sorted(around.up), ['DEMO-A-001', 'DEMO-A-002', 'DEMO-A-003', 'DEMO-A-004']);
  assert.deepEqual(sorted(around.down), ['DEMO-A-006']);
  const wide = detail.neighborhood(model, 'DEMO-A-002');
  assert.deepEqual(sorted(wide.down), ['DEMO-A-004', 'DEMO-A-005', 'DEMO-A-006', 'DEMO-A-012', 'DEMO-A-013', 'DEMO-A-014', 'DEMO-B-003']);
  const alone = detail.neighborhood(model, 'DEMO-A-010');
  assert.equal(alone.up.size + alone.down.size, 0);
});

test('a dependency cycle does not hang the chain and an item is never its own upstream', () => {
  const next = JSON.parse(JSON.stringify(source));
  next.edges.push({ dependent: 'DEMO-A-001', prerequisite: 'DEMO-A-005', required: true });
  next.edges.sort((a, b) => (`${a.dependent}\u0000${a.prerequisite}` < `${b.dependent}\u0000${b.prerequisite}` ? -1 : 1));
  next.basis.work_graph.edge_count = next.edges.length;
  const cyclic = modelLib.loadModel(snapshotLib.seal(next, source.generated_at), config);
  const around = detail.neighborhood(cyclic, 'DEMO-A-004');
  assert.ok(!around.up.has('DEMO-A-004') && !around.down.has('DEMO-A-004'));
  assert.ok(around.up.has('DEMO-A-005') && around.down.has('DEMO-A-005'), 'both directions reach the cycle');
});

// ---------------------------------------------------------------------- what the panel knows

test('the facts of an item come from the snapshot: status, claims, sessions, health findings and the chain', () => {
  const d = detail.detailData(model, 'DEMO-A-004', h.tFor('en'));
  assert.equal(d.vis, 'developing');
  assert.equal(d.status, 'in_progress');
  assert.equal(d.owner, 'demo-agent-1');
  assert.equal(d.milestone, 'M1');
  assert.equal(d.lane, 'ALPHA' === d.lane ? 'ALPHA' : d.lane, 'the lane is named by the display configuration');
  assert.deepEqual(d.prerequisites.map((n) => n.key), ['DEMO-A-002', 'DEMO-A-003']);
  assert.deepEqual(d.dependents.map((n) => n.key), ['DEMO-A-005']);
  assert.equal(d.upstream, 3);
  assert.equal(d.downstream, 2);
  assert.deepEqual(d.claims, [{ agent: 'demo-agent-1', session: 'sess-demo-agent-1', acquiredAt: 1768348800000, expiresAt: 1768446000000 }]);
  assert.deepEqual(d.sessions.map((s) => [s.id, s.status]), [['sess-demo-agent-1', 'active']]);
  assert.deepEqual(d.findings.map((f) => f.code), ['active_session']);
  assert.equal(d.stuck, false);
  assert.equal(d.generatedAt, source.generated_at ? Date.parse(source.generated_at) : d.generatedAt);
  assert.equal(detail.detailData(model, 'NO-SUCH-ITEM', h.tFor('en')), null);
});

test('a dependency on cancelled work is explained, with the cancelled items named', () => {
  const d = detail.detailData(model, 'DEMO-A-012', h.tFor('en'));
  assert.equal(d.stuck, true);
  assert.deepEqual(d.cancelledAbove, ['DEMO-A-011']);
  assert.deepEqual(d.diagnostics, [{ code: 'dependency_not_completed', detail: 'required dependency has source status cancelled', work: 'DEMO-A-011' }]);
  const html = detail.renderDetail(d, h.tFor('en'), (g) => page.gapReason(h.tFor('en'), g));
  assert.match(html, /Behind a cancelled dependency/);
  assert.match(html, /Cancelled upstream: DEMO-A-011/);
  assert.match(html, /dependency_not_completed/);
});

test('blockers, next actions and waits appear when the snapshot has them and are left out when it has not', () => {
  const t = h.tFor('en');
  const blocked = detail.renderDetail(detail.detailData(model, 'DEMO-A-008', t), t, (g) => g.reason);
  assert.match(blocked, /<h3>Blocker<\/h3><p>waiting_for_user: /);
  const plain = detail.renderDetail(detail.detailData(model, 'DEMO-A-010', t), t, (g) => g.reason);
  for (const absent of ['Blocker', 'Next action', 'Waits', 'Readiness diagnostics', 'Health findings', 'Sessions', 'Behind a cancelled dependency']) assert.doesNotMatch(plain, new RegExp(`<h3>${absent}`), absent);
  assert.match(plain, /<h3>Needs \(0\)<\/h3><p class="pm-d-none">None<\/p>/);

  const next = JSON.parse(JSON.stringify(source));
  const target = next.nodes.find((n) => n.key === 'DEMO-A-007');
  target.next_action = 'Review the draft with <b>the team</b>';
  target.waits = [{ kind: 'approval', summary: 'Waiting for sign-off', release_condition: 'sign-off recorded' }];
  const richer = modelLib.loadModel(snapshotLib.seal(next, source.generated_at), config);
  const html = detail.renderDetail(detail.detailData(richer, 'DEMO-A-007', t), t, (g) => g.reason);
  assert.match(html, /<h3>Next action<\/h3><p>Review the draft with &lt;b&gt;the team&lt;\/b&gt;<\/p>/);
  assert.match(html, /<h3>Waits<\/h3>.*<code>approval<\/code> Waiting for sign-off<br><small>sign-off recorded<\/small>/);
});

test('the fields the official commands do not provide are listed, with the reason from the snapshot or the catalog', () => {
  const d = detail.detailData(model, 'DEMO-A-004', h.tFor('en'));
  assert.deepEqual(d.gaps.map((g) => g.field), ['node.goal', 'node.scope', 'node.priority', 'node.milestone']);
  assert.ok(d.gaps.every((g) => g.reason.length > 20));
  const en = detail.renderDetail(d, h.tFor('en'), (g) => page.gapReason(h.tFor('en'), g));
  const zh = detail.renderDetail(detail.detailData(model, 'DEMO-A-004', h.tFor('zh-CN')), h.tFor('zh-CN'), (g) => page.gapReason(h.tFor('zh-CN'), g));
  assert.match(en, /<code>node\.priority<\/code>: /);
  assert.match(zh, /官方接口未提供/);
  assert.notEqual(en.match(/<code>node\.priority<\/code>: ([^<]*)/)[1], zh.match(/<code>node\.priority<\/code>: ([^<]*)/)[1], 'the reason is translated');
  assert.doesNotMatch(zh, /project\.scenarios|release_funnel/, 'only fields that belong to work items are listed there');
});

test('jump buttons: one per prerequisite and dependent, each naming the item and its status', () => {
  const t = h.tFor('en');
  const d = detail.detailData(model, 'DEMO-A-012', t);
  const html = detail.renderDetail(d, t, (g) => g.reason);
  const jumps = [...html.matchAll(/data-pm-jump="([^"]+)"/g)].map((m) => m[1]);
  assert.deepEqual(jumps, [...d.prerequisites, ...d.dependents].map((n) => n.key));
  assert.match(html, /<button type="button" class="pm-link" data-pm-jump="DEMO-A-011"><code>[^<]*A-011<\/code> <span class="pm-link-title">[^<]+<\/span> <span class="pm-badge" data-vis="cancelled">Cancelled<\/span><\/button>/);
  assert.match(html, /<section class="pm-d-sec"><h3>Needs \(2\)<\/h3>/);
  assert.match(html, /<section class="pm-d-sec"><h3>Unlocks \(1\)<\/h3>/);
});

test('the details of every item render in both languages without a missing message, "undefined" or "NaN"', () => {
  for (const lang of ['en', 'zh-CN']) {
    const t = h.tFor(lang);
    for (const key of model.nodes.keys()) {
      const html = detail.renderDetail(detail.detailData(model, key, t), t, (g) => page.gapReason(t, g));
      assert.doesNotMatch(html, /\bmap\.(ui|detail|status|gap)\./, `${lang} ${key}: message key shown`);
      assert.doesNotMatch(html, /undefined|NaN|\[object|>null</, `${lang} ${key}`);
      h.assertWellFormed(html, `${lang} ${key}`);
      assert.ok(html.includes(`<code>${key}</code>`), `${lang} ${key}`);
    }
  }
});

test('text from the project cannot inject markup into the panel', () => {
  const hostile = '<img src=x onerror=alert(1)> & "quoted" \'single\'';
  const next = JSON.parse(JSON.stringify(source));
  const target = next.nodes.find((n) => n.key === 'DEMO-A-008');
  Object.assign(target, { title: hostile, blocker: hostile, next_action: hostile, owner: hostile, milestone: hostile });
  target.diagnostics = [{ code: hostile, detail: hostile, work_item_key: 'DEMO-A-008' }];
  target.waits = [{ kind: hostile, summary: hostile, release_condition: hostile }];
  const evil = modelLib.loadModel(snapshotLib.seal(next, source.generated_at), config);
  const t = h.tFor('en');
  const html = detail.renderDetail(detail.detailData(evil, 'DEMO-A-008', t), t, (g) => g.reason);
  assert.doesNotMatch(html, /<img/i);
  assert.doesNotMatch(html, /onerror=alert\(1\)>/);
  assert.match(html, /&lt;img src=x onerror=alert\(1\)&gt; &amp; &quot;quoted&quot; &#x27;single&#x27;/);
  h.assertWellFormed(html, 'hostile panel');
  // jump buttons of the neighbours of such an item are escaped too
  const around = detail.renderDetail(detail.detailData(evil, 'DEMO-A-003', t), t, (g) => g.reason);
  assert.doesNotMatch(around, /<img/i);
});

test('the awr command for an item quotes keys that a shell would split', () => {
  assert.equal(detail.commandFor('DEMO-A-004'), 'awr work show DEMO-A-004');
  assert.equal(detail.commandFor('weird key'), "awr work show 'weird key'");
  assert.equal(detail.commandFor("it's"), "awr work show 'it'\\''s'");
  assert.equal(detail.commandFor('a;rm -rf x'), "awr work show 'a;rm -rf x'");
});

// ---------------------------------------------------------------------- fold state is a pure input of the views

const views = viewsLib.createViews(h.tFor('en'));
const cardsOf = (svg) => [...svg.matchAll(/class="pm-card" data-key="([^"]+)"/g)].map((m) => m[1]);
const state = (patch) => ({ overview: {}, mainline: {}, explore: {}, ...patch });

test('without a fold state the views draw the default; the same state always draws the same bytes', () => {
  assert.equal(views.renderOverview(model), views.renderOverview(model, {}));
  assert.equal(views.renderOverview(model), views.renderOverview(model, state({ overview: { collapsed: new Set(), doneOpen: new Set(), groupsOpen: new Set() } })));
  const folded = state({ overview: { collapsed: new Set(['ALPHA']) } });
  assert.equal(views.renderOverview(model, folded), views.renderOverview(model, folded));
  assert.notEqual(views.renderOverview(model, folded), views.renderOverview(model));
  assert.equal(views.renderMainline(model), views.renderMainline(model, {}));
  assert.equal(views.renderExplore(model), views.renderExplore(model, {}));
});

test('overview: a folded lane keeps its header and counts, loses its cards, and the other lanes are drawn as before', () => {
  const all = cardsOf(views.renderOverview(model));
  const folded = cardsOf(views.renderOverview(model, state({ overview: { collapsed: new Set(['ALPHA']) } })));
  assert.deepEqual(folded, all.filter((k) => !k.startsWith('DEMO-A-')));
  const svg = views.renderOverview(model, state({ overview: { collapsed: new Set(['ALPHA']) } }));
  assert.match(svg, /data-lane="ALPHA" data-open="false"/);
  assert.match(svg, /14 items hidden/);
  const open = views.renderOverview(model);
  assert.match(open, /data-lane="ALPHA" data-open="true"/);
});

test('overview: opening finished work adds one row per finished item', () => {
  const rows = (svg) => cardsOf(svg).filter((k) => model.nodes.get(k) && model.nodes.get(k).vis === 'done');
  assert.deepEqual(rows(views.renderOverview(model)), []);
  const open = rows(views.renderOverview(model, state({ overview: { doneOpen: new Set(['ALPHA']) } })));
  assert.deepEqual(open, ['DEMO-A-001', 'DEMO-A-002', 'DEMO-A-003']);
  assert.match(views.renderOverview(model, state({ overview: { doneOpen: new Set(['ALPHA']) } })), /data-done-lane="ALPHA" data-open="true"/);
});

test('overview: a group is cut after four cards and "N more" opens it', () => {
  const next = JSON.parse(JSON.stringify(source));
  for (let i = 15; i < 23; i++) {
    const key = `DEMO-A-${String(i).padStart(3, '0')}`;
    next.nodes.push({ ...next.nodes[4], id: `id-${key}`, key, title: `Extra ${i}`, status: 'planned', ready: false, milestone: 'M1' });
    if (i > 15) next.edges.push({ dependent: key, prerequisite: `DEMO-A-${String(i - 1).padStart(3, '0')}`, required: true });
  }
  next.nodes.sort((a, b) => (a.key < b.key ? -1 : 1));
  next.edges.sort((a, b) => (`${a.dependent}\u0000${a.prerequisite}` < `${b.dependent}\u0000${b.prerequisite}` ? -1 : 1));
  next.basis.work_graph.node_count = next.nodes.length;
  next.basis.work_graph.edge_count = next.edges.length;
  const big = modelLib.loadModel(snapshotLib.seal(next, source.generated_at), config);
  const waiting = [...big.nodes.values()].filter((n) => n.lane === 'ALPHA' && n.vis === 'waiting').map((n) => n.id);
  const bigViews = viewsLib.createViews(h.tFor('en'));
  const closed = bigViews.renderOverview(big);
  assert.equal(cardsOf(closed).filter((k) => waiting.includes(k)).length, 4);
  assert.match(closed, /data-group="ALPHA\|waiting" data-open="false"/);
  const opened = bigViews.renderOverview(big, state({ overview: { groupsOpen: new Set(['ALPHA|waiting']) } }));
  assert.equal(cardsOf(opened).filter((k) => waiting.includes(k)).length, waiting.length);
  assert.match(opened, /data-group="ALPHA\|waiting" data-open="true"/);
  assert.match(opened, /Show fewer/);
});

test('dependency panels: a folded panel keeps its header and an unfolded finished block shows its cards', () => {
  const next = JSON.parse(JSON.stringify(source));
  next.edges = next.edges.filter((e) => !(e.prerequisite === 'DEMO-A-001' && e.dependent === 'DEMO-A-011'));
  next.basis.work_graph.edge_count = next.edges.length;
  const quiet = modelLib.loadModel(snapshotLib.seal(next, source.generated_at), config);
  const plain = views.renderMainline(quiet);
  assert.ok(!cardsOf(plain).includes('DEMO-A-001'));
  const withDone = views.renderMainline(quiet, state({ mainline: { showDone: new Set(['ALPHA']) } }));
  assert.ok(cardsOf(withDone).includes('DEMO-A-001'));
  assert.match(withDone, /data-panel-done="ALPHA" data-open="true"/);
  const folded = views.renderMainline(quiet, state({ mainline: { collapsed: new Set(['ALPHA']) } }));
  assert.ok(cardsOf(folded).every((k) => !k.startsWith('DEMO-A-')));
  assert.ok(cardsOf(folded).some((k) => k.startsWith('DEMO-B-')));
  assert.match(folded, /data-panel="ALPHA" data-open="false"/);
  assert.match(folded, /unfinished/);
});

test('milestone lanes: a folded lane draws no cards and finished work can be added', () => {
  const plain = cardsOf(views.renderExplore(model));
  assert.ok(plain.length > 5 && !plain.includes('DEMO-A-001'));
  const folded = cardsOf(views.renderExplore(model, state({ explore: { collapsed: new Set([0]) } })));
  assert.ok(folded.length < plain.length && folded.every((k) => model.nodes.get(k).milestone !== 'M1'));
  const withDone = cardsOf(views.renderExplore(model, state({ explore: { showDone: true } })));
  assert.ok(withDone.includes('DEMO-A-001'));
  assert.match(views.renderExplore(model, state({ explore: { collapsed: new Set([0]) } })), /data-xlane="0" data-open="false"/);
});

test('executor chips name their agent and the work it holds', () => {
  const svg = views.renderOverview(model);
  const chips = [...svg.matchAll(/<g class="pm-agent" data-agent="([^"]+)"><title>([^<]*)<\/title>/g)].map((m) => [m[1], m[2]]);
  assert.deepEqual(chips.map(([agent]) => agent).sort(), ['demo-agent-1', 'demo-agent-3']);
  assert.match(chips.find(([agent]) => agent === 'demo-agent-1')[1], /^demo-agent-1 · .*A-004/);
  assert.match(chips.find(([agent]) => agent === 'demo-agent-3')[1], /^demo-agent-3 · .*B-002/);
  const hostile = JSON.parse(JSON.stringify(source));
  const claimed = hostile.nodes.find((n) => n.key === 'DEMO-A-004');
  claimed.claims = [{ ...claimed.claims[0], agent_id: 'a"><img src=x>' }];
  const evil = modelLib.loadModel(snapshotLib.seal(hostile, source.generated_at), config);
  const markup = views.renderOverview(evil);
  assert.doesNotMatch(markup, /<img/i, 'an agent name cannot inject markup');
  assert.match(markup, /data-agent="a&quot;&gt;&lt;img src=x&gt;"/);
});

test('edges carry their endpoints so the highlight can follow a chain', () => {
  const svg = views.renderMainline(model);
  const edges = [...svg.matchAll(/<path d="[^"]+"[^>]* data-from="([^"]+)" data-to="([^"]+)"/g)].map((m) => [m[1], m[2]]);
  assert.ok(edges.length > 8);
  for (const [from, to] of edges) {
    if (from === 'BLOCK') continue;
    assert.ok(model.nodes.get(to).deps.includes(from), `${from} -> ${to}: data-from is the prerequisite, data-to the dependent`);
  }
});
