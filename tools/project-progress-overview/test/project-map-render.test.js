/**
 * Project map renderer: golden output, determinism, self-containment, lanes and the unavailable-field notice.
 *
 * The goldens are byte-for-byte; after an intended rendering change refresh them with
 *   UPDATE_GOLDEN=1 node --test test/project-map-render.test.js
 * Run: node --test test/project-map-render.test.js
 */

'use strict';

const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const h = require('./fixtures/project-map/helpers.js');
const page = require('../public/project-map/page.js');
const snapshotLib = require('../public/project-map/snapshot.js');
const modelLib = require('../public/project-map/model.js');
const demo = require('../public/project-map/demo.js');
const viewsLib = require('../public/project-map/views.js');
const detail = require('../public/project-map/detail.js');

const GOLDEN = path.join(h.FIXTURES, 'golden');
const REFRESH = process.env.UPDATE_GOLDEN === '1';

function golden(name, actual) {
  const file = path.join(GOLDEN, name);
  if (REFRESH) {
    fs.writeFileSync(file, actual, 'utf8');
    return;
  }
  assert.ok(fs.existsSync(file), `missing golden file ${name}; create it with UPDATE_GOLDEN=1`);
  assert.equal(actual, fs.readFileSync(file, 'utf8'), `${name} differs from its golden file; if the change is intended run UPDATE_GOLDEN=1 node --test test/project-map-render.test.js`);
}

const CASES = [
  ['demo.en.html', () => h.renderDemo('en')],
  ['demo.zh-CN.html', () => h.renderDemo('zh-CN')],
  ['empty.en.html', () => h.renderFixture('empty', 'en')],
  ['single.en.html', () => h.renderFixture('single', 'en')],
  ['states.zh-CN.html', () => h.renderFixture('states', 'zh-CN')],
];

for (const [name, render] of CASES) {
  test(`golden: ${name}`, () => golden(name, render().document));
}

test('rendering twice gives the same bytes in every language', () => {
  for (const lang of ['en', 'zh-CN']) {
    assert.equal(h.renderDemo(lang).document, h.renderDemo(lang).document);
    assert.equal(h.renderFixture('states', lang).document, h.renderFixture('states', lang).document);
  }
});

test('canonical unknown work status remains visible and distinct from readiness in every view', () => {
  const source = h.readJson('states.json');
  const node = source.nodes.find((n) => n.key === 'DEMO-A-004');
  node.status = 'unknown';
  node.ready = false;
  const snapshot = snapshotLib.seal(source, source.generated_at);
  for (const lang of ['en', 'zh-CN']) {
    const t = h.tFor(lang);
    const out = page.renderProjectMap(snapshot, h.readJson('config.json'), { t, lang });
    const model = out.model;
    assert.equal(model.nodes.get(node.key).vis, 'unknown');
    assert.ok(out.table.includes(t('map.status.unknown')));
    for (const view of out.views) {
      assert.ok(view.svg.includes(t('map.status.unknown')), view.id);
    }
    assert.equal(snapshot.nodes.find((n) => n.key === node.key).status, 'unknown');
  }
});

test('supporting information uses consistent cards and keeps field gaps collapsed', () => {
  const out = h.renderDemo('en');
  assert.match(out.table, /class="table pm-support-card"/);
  assert.match(out.table, /class="pm-table-status"/);
  assert.match(out.notes, /class="pm-note-card"/);
  assert.match(out.notes, /<details class="pm-note-card pm-field-gaps"><summary>/);
  assert.doesNotMatch(out.notes, /<details[^>]*\bopen\b/);
  assert.ok(out.notes.includes(out.fingerprint));
});

test('compact cards omit suffix rollups and reduction counts while details retain declared dependencies', () => {
  const source = h.readJson('states.json');
  const template = source.nodes.find((n) => n.key === 'DEMO-A-004');
  const key = 'DEMO-A-100';
  const steps = Array.from({ length: 13 }, (_, i) => String.fromCharCode(65 + i));
  source.nodes = [{ ...template, id: key, key, title: 'Deliver the service', owner: 'review-agent', claims: [], diagnostics: [] },
    ...steps.map((step) => ({ ...template, id: key + step, key: key + step, title: `Contract step ${step}`, status: 'completed', owner: null, claims: [], diagnostics: [] }))];
  source.edges = steps.flatMap((step, i) => [
    { dependent: key, prerequisite: key + step, required: true },
    ...(i ? [{ dependent: key + step, prerequisite: key + steps[i - 1], required: true }] : []),
  ]);
  source.edges.sort((a, b) => snapshotLib.compare(`${a.dependent}\u0000${a.prerequisite}`, `${b.dependent}\u0000${b.prerequisite}`));
  source.sessions = [];
  source.doctor.findings = [];
  source.basis.work_graph.node_count = source.nodes.length;
  source.basis.work_graph.edge_count = source.edges.length;
  const config = h.readJson('config.json');
  config.mainline_panels[0].keep_done = true;

  for (const lang of ['en', 'zh-CN']) {
    const t = h.tFor(lang);
    const model = modelLib.loadModel(snapshotLib.seal(source, source.generated_at), config);
    const views = viewsLib.createViews(t);
    for (const svg of [views.renderMainline(model), views.renderExplore(model, { explore: { showDone: true } })]) {
      const card = svg.match(/<g\b[^>]*\bclass="pm-card" data-key="DEMO-A-100"[\s\S]*?<rect class="pm-ring"[^>]*\/>\s*<\/g>/);
      assert.ok(card, `${lang} compact card is visible`);
      assert.match(card[0], /Deliver the service/);
      assert.match(card[0], /review-agent/);
      assert.ok(card[0].includes(t('map.status.developing')));
      assert.doesNotMatch(card[0], /Roll-up|汇总项|[A-M]✓|Declares|声明|after reduction|约简后/);
      assert.doesNotMatch(svg, /Roll-up ⊃|汇总项 ⊃/);
    }
    const data = detail.detailData(model, key, t);
    assert.equal(data.prerequisites.length, steps.length);
    const html = detail.renderDetail(data, t, (gap) => page.gapReason(t, gap));
    for (const step of steps) {
      assert.ok(html.includes(`data-pm-jump="${key + step}"`));
      assert.ok(html.includes(`Contract step ${step}`));
    }
  }
});

test('default swimlanes follow recorded milestones and explicit empty configuration stays empty', () => {
  const source = h.readJson('states.json');
  const automatic = modelLib.loadModel(source);
  const expected = new Set(source.nodes.filter((n) => !['completed', 'cancelled'].includes(n.status)).map((n) => n.milestone).filter(Boolean));
  assert.deepEqual(new Set(automatic.cfg.explore_lanes.flatMap((l) => l.milestones)), expected);
  assert.deepEqual(modelLib.loadModel(source, { explore_lanes: [] }).cfg.explore_lanes, []);
});

test('a broad dependency level wraps within the panel while preserving cards and edges', () => {
  const layout = require('../public/project-map/layout.js');
  const kit = require('../public/project-map/svgkit.js');
  const ids = ['ROOT', ...Array.from({ length: 32 }, (_, i) => `JOB-${i}`), 'END'];
  const edges = ids.slice(1, -1).flatMap((id) => [['ROOT', id], [id, 'END']]);
  const render = () => {
    const svg = new kit.Svg(0, 0);
    const result = layout.drawDag(kit.createKit(h.tFor('en')), svg, 0, 0, ids, edges,
      (id, x, y) => `<rect data-key="${id}" x="${x}" y="${y}" width="198" height="72"/>`,
      () => 198, () => 72, { maxWidth: 1000 });
    return { svg, result };
  };
  const { svg, result } = render();
  assert.ok(result.width <= 1000, `diagram width ${result.width}`);
  assert.equal((svg.b.join('').match(/data-key=/g) || []).length, ids.length);
  assert.equal((svg.b.join('').match(/data-from=/g) || []).length, edges.length);
  for (const [pre, next] of edges) assert.ok(result.pos.get(pre)[1] < result.pos.get(next)[1]);
  const positions = [...result.pos.values()];
  for (const [i, a] of positions.entries()) for (const b of positions.slice(i + 1)) {
    assert.ok(Math.abs(a[0] - b[0]) >= 198 || Math.abs(a[1] - b[1]) >= 72);
  }
  assert.deepEqual(render().svg.b, svg.b);
});

test('empty and deep dependency panels keep finite bounds without unnecessary wrapping', () => {
  const layout = require('../public/project-map/layout.js');
  const kit = require('../public/project-map/svgkit.js');
  const draw = (ids, edges) => layout.drawDag(kit.createKit(h.tFor('en')), new kit.Svg(0, 0), 0, 0, ids, edges,
    () => '', () => 198, () => 72, { maxWidth: 1000 });
  assert.deepEqual(draw([], []), { width: 0, height: 0, pos: new Map(), wrapped: false });
  const ids = Array.from({ length: 50 }, (_, i) => `STEP-${i}`);
  const result = draw(ids, ids.slice(1).map((id, i) => [ids[i], id]));
  assert.equal(result.wrapped, false);
  assert.equal(result.width, 198);
  assert.ok(Number.isFinite(result.height));
});

test('automatic mainline panels include every active lane in the project order', () => {
  const data = h.readJson('states.json');
  const template = data.nodes[0];
  const lanes = Array.from({ length: 6 }, (_, i) => ({ id: `L${i}`, name: `Line ${i}`, color: '#3b82f6', key_regex: `^LINE${i}-` }));
  data.nodes = lanes.flatMap((lane, i) => [0, 1].map((n) => ({ ...template, key: `LINE${i}-${n}`, id: `id-${i}-${n}`, status: 'in_progress', claims: [], diagnostics: [] })));
  data.edges = lanes.map((lane, i) => ({ prerequisite: `LINE${i}-0`, dependent: `LINE${i}-1`, required: true }));
  data.sessions = [];
  data.basis.work_graph.node_count = data.nodes.length;
  data.basis.work_graph.edge_count = data.edges.length;
  const model = modelLib.loadModel(snapshotLib.seal(data, data.generated_at), { overview: { lanes } });
  assert.deepEqual(model.cfg.mainline_panels.map((p) => p.lane), lanes.map((lane) => lane.id));
});

test('the page is one self-contained file: no script, no external request, no inline style attribute', () => {
  for (const [name, render] of CASES) {
    const out = render().document;
    assert.doesNotMatch(out, /<script/i, name);
    assert.doesNotMatch(out, /(?:src|href)="https?:/i, name);
    assert.doesNotMatch(out, /url\(\s*["']?https?:/i, name);
    assert.doesNotMatch(out, /@import/, name);
    // The Project Progress Overview page runs under a CSP without inline styles; presentation attributes and classes are enough.
    assert.doesNotMatch(out.replace(/<style>[\s\S]*?<\/style>/, ''), /\sstyle="/, name);
    h.assertWellFormed(out, name);
  }
});

test('three SVG views with unique ids and a readable text alternative', () => {
  const r = h.renderDemo('en');
  assert.deepEqual(r.views.map((v) => v.id), ['overview', 'mainline', 'explore']);
  const ids = [...r.document.matchAll(/\bid="([^"]+)"/g)].map((m) => m[1]);
  assert.deepEqual(ids.filter((id, i) => ids.indexOf(id) !== i), []);
  for (const v of r.views) {
    assert.match(v.svg, /role="img" aria-label="[^"]+"/);
    h.assertWellFormed(v.svg, v.id);
  }
  // every url(#x) reference points at an id of the same view
  for (const v of r.views) {
    const own = new Set([...v.svg.matchAll(/\bid="([^"]+)"/g)].map((m) => m[1]));
    for (const [, ref] of v.svg.matchAll(/url\(#([^)]+)\)/g)) assert.ok(own.has(ref), `${v.id} references missing #${ref}`);
  }
});

test('theme, narrow-screen and print rules ship with the file', () => {
  const out = h.renderDemo('en').document;
  for (const needle of ['prefers-color-scheme:dark', 'overflow-x:auto', '@media (max-width:640px)', 'name="viewport"', '@media print', 'name="color-scheme"', '<html lang="en">']) {
    assert.ok(out.includes(needle), needle);
  }
  assert.match(h.renderDemo('zh-CN').document, /<html lang="zh-CN">/);
});

test('every work item is readable as text, in the table and in the card hover title', () => {
  const r = h.renderDemo('en');
  for (const node of r.model.nodes.values()) {
    assert.ok(r.document.includes(`<td class="k">${node.id}</td>`), `${node.id} missing from the table`);
  }
  const overview = r.views[0].svg;
  assert.match(overview, /<title>DEMO-A-004 · Demo: core feature \(in progress, active claim\)<\/title>/);
});

test('all states and the dependency defect are drawn, in both languages', () => {
  for (const lang of ['en', 'zh-CN']) {
    const t = h.tFor(lang);
    const out = h.renderDemo(lang).document;
    for (const key of ['developing', 'stalled', 'blocked', 'ready', 'draft', 'waiting', 'cancelled', 'done']) assert.ok(out.includes(t(`map.status.${key}`)), `${lang} ${key}`);
    assert.ok(out.includes(t('map.defect.label')), `${lang} defect`);
    assert.ok(out.includes('stroke-dasharray="2.5 3"'), 'halo of the items stuck behind the cancelled dependency');
    assert.ok(out.includes('expired_claim'), 'health chips come from the snapshot findings');
  }
});

test('the card for a stalled item says how long it has been quiet', () => {
  assert.match(h.renderDemo('en').document, /Stalled 20d/);
  assert.match(h.renderDemo('zh-CN').document, /停滞 20d/);
});

test('fields the official CLI does not provide are declared on the page, localized', () => {
  for (const lang of ['en', 'zh-CN']) {
    const out = h.renderDemo(lang).document;
    for (const entry of snapshotLib.UNAVAILABLE) {
      assert.ok(out.includes(`<code>${entry.field}</code>`), `${lang} ${entry.field}`);
      assert.ok(out.includes(page.gapReason(h.tFor(lang), entry)), `${lang} reason of ${entry.field}`);
    }
  }
  // The overview also lists them under the legend, never silently filled in.
  assert.match(h.renderDemo('en').views[0].svg, /Not provided by the official interfaces: node\.goal, node\.scope/);
  assert.equal(h.tFor('en')('map.gap.node_goal') !== 'map.gap.node_goal', true);
});

test('an empty project and a single item draw placeholders instead of failing', () => {
  const empty = h.renderFixture('empty', 'en').document;
  assert.ok(empty.includes('No work items'));
  assert.ok(empty.includes('No dependency panel to draw'));
  assert.ok(empty.includes('No unfinished work in the milestone lanes'));
  const single = h.renderFixture('single', 'en');
  assert.ok(single.document.includes('DEMO-A-001'));
  h.assertWellFormed(single.document, 'single');
});

test('default grouping derives mainlines from keys and swimlanes from recorded milestones', () => {
  const t = h.tFor('en');
  const { snapshot } = demo.buildDemo(t);
  const r = page.renderProjectMap(snapshot, undefined, { t, lang: 'en' });
  assert.deepEqual(r.model.cfg.overview.lanes.map((l) => l.name), ['DEMO-A', 'DEMO-B', 'DEMO-C']);
  assert.match(r.views[2].svg, /data-xlane="0"/);
  assert.ok(r.views[1].svg.includes('DEMO-A'));
  assert.deepEqual(r.model.cfg.mainline_panels, [{ lane: 'DEMO_A' }, { lane: 'DEMO_B' }]);
  const empty = JSON.parse(JSON.stringify(snapshot));
  for (const node of empty.nodes) node.milestone = null;
  const missing = page.renderProjectMap(snapshotLib.seal(empty, snapshot.generated_at), undefined, { t });
  assert.match(missing.views[2].svg, /data-xlane="0"/);
  assert.ok(missing.views[2].svg.includes(t('map.title.explore_mainlines')));
  assert.ok(missing.model.cfg.explore_lanes.every((lane) => lane.work_keys.length));
  assert.ok([...missing.model.nodes.values()].every((node) => node.milestone === null));
});

test('the lane configuration decides the grouping, the stale threshold decides what counts as stalled', () => {
  const t = h.tFor('en');
  const { snapshot, config } = demo.buildDemo(t);
  const render = (cfg) => page.renderProjectMap(snapshot, cfg, { t, lang: 'en' });
  const base = render(config);
  // One lane for everything: the overview collapses to a single column.
  const flat = render({ ...config, overview: { lanes: [{ id: 'ALL', name: 'Everything', color: '#3b82f6' }], match_order: [], default_lane: 'ALL' }, mainline_panels: [{ lane: 'ALL' }], explore_lanes: [] });
  assert.notEqual(flat.document, base.document);
  assert.ok(flat.views[0].svg.includes('Everything'));
  assert.ok(!flat.views[0].svg.includes('Demo lane A'));
  // DEMO-A-004 is 1 day idle: a 1-day threshold makes it stalled, the default does not.
  const strict = render({ ...config, stale_days: 1 });
  assert.equal(base.model.nodes.get('DEMO-A-004').vis, 'developing');
  assert.equal(strict.model.nodes.get('DEMO-A-004').vis, 'stalled');
  assert.notEqual(strict.document, base.document);
  // titles in the configuration replace the defaults
  assert.ok(render({ ...config, titles: { page: 'My map', overview: 'My overview' } }).document.includes('<h1>My map</h1>'));
});

test('text from the project cannot inject markup', () => {
  const t = h.tFor('en');
  const { snapshot, config } = demo.buildDemo(t);
  const hostile = JSON.parse(JSON.stringify(snapshot));
  hostile.nodes[0].title = '<img src=x onerror=alert(1)> & "quoted" \'single\'';
  hostile.nodes[0].owner = '<script>alert(1)</script>';
  hostile.project.name = '</title><script>alert(1)</script>';
  hostile.fingerprint = snapshotLib.fingerprint(hostile);
  const out = page.renderProjectMap(hostile, config, { t, lang: 'en' }).document;
  assert.doesNotMatch(out, /<img src=x/);
  assert.doesNotMatch(out, /<script/i);
  assert.doesNotMatch(out, /<\/title><script/);
  assert.ok(out.includes('&lt;img src=x onerror=alert(1)&gt; &amp; &quot;quoted&quot; &#x27;single&#x27;'));
  h.assertWellFormed(out, 'hostile');
});

test('the display configuration is validated before anything is drawn', () => {
  const { snapshot } = demo.buildDemo(h.tFor('en'));
  const bad = (cfg, pattern) => assert.throws(() => modelLib.normalizeConfig(cfg, snapshot), pattern);
  bad({ stale_days: 0 }, /stale_days/);
  bad({ stale_days: 1.5 }, /stale_days/);
  bad({ version: 2 }, /version must be 1/);
  bad({ overview: { lanes: [] } }, /1 to 12 lanes/);
  bad({ overview: { lanes: [{ id: 'A', name: 'A', color: 'red' }] } }, /hex color/);
  bad({ overview: { lanes: [{ id: 'A', name: 'A', color: '#fff', key_regex: '(' }] } }, /valid expression/);
  bad({ overview: { lanes: [{ id: 'A', name: 'A', color: '#fff' }, { id: 'A', name: 'B', color: '#fff' }] } }, /unique/);
  bad({ overview: { lanes: [{ id: 'A', name: 'A', color: '#fff' }], default_lane: 'Z' } }, /unknown lane/);
  bad({ overview: { lanes: [{ id: 'A', name: 'A', color: '#fff' }], match_order: ['A'] } }, /no key_regex/);
  bad({ mainline_panels: [{ lane: 'NOPE' }] }, /unknown lane/);
  bad({ explore_lanes: [{ name: 'x', color: '#fff', milestones: [] }] }, /at least one milestone/);
  bad({ explore_lanes: [{ name: 'x', color: '#fff', milestones: ['M'] }, { name: 'y', color: '#fff', milestones: ['M'] }] }, /more than one explore lane/);
  bad([], /must be an object/);
  assert.doesNotThrow(() => modelLib.normalizeConfig({}, snapshot));
  assert.doesNotThrow(() => modelLib.normalizeConfig(undefined, snapshot));
});

test('the documented example configuration is valid and its lanes group the keys they name', () => {
  const configLoader = require('../project-map-config.js');
  const { config } = configLoader.load(path.join(__dirname, '..', 'examples', 'project-map.config.json'));
  const t = h.tFor('en');
  const base = demo.buildDemo(t).snapshot;
  const rename = (key) => key.replace('DEMO-A-', 'EX-PLAT-').replace('DEMO-B-', 'EX-FEAT-').replace('DEMO-C-', 'EX-MISC-');
  const content = JSON.parse(JSON.stringify(base));
  for (const n of content.nodes) n.key = rename(n.key);
  for (const e of content.edges) { e.dependent = rename(e.dependent); e.prerequisite = rename(e.prerequisite); }
  for (const s of content.sessions) s.work_key = s.work_key && rename(s.work_key);
  for (const n of content.nodes) for (const d of n.diagnostics) d.work_item_key = rename(d.work_item_key);
  content.nodes.sort((a, b) => (a.key < b.key ? -1 : 1));
  content.edges.sort((a, b) => (a.dependent + a.prerequisite < b.dependent + b.prerequisite ? -1 : 1));
  const sealed = snapshotLib.seal(content, base.generated_at);
  const r = page.renderProjectMap(sealed, config, { t, lang: 'en' });
  assert.deepEqual([...new Set([...r.model.nodes.values()].map((n) => n.lane))].sort(), ['FEATURES', 'OTHER', 'PLATFORM']);
  assert.equal(r.model.nodes.get('EX-PLAT-001').short, 'PLAT-001', 'strip_prefixes shortens the keys shown on cards');
  assert.ok(r.document.includes('<h1>Example project map</h1>'));
  assert.deepEqual(r.model.cfg.mainline_panels, [{ lane: 'PLATFORM', crit_end: 'EX-PLAT-020' }, { lane: 'FEATURES', keep_done: true }]);
  h.assertWellFormed(r.document, 'example config');
});

test('the renderer sources read nothing from the project: no files, no database, no processes, no network', () => {
  const dir = path.join(__dirname, '..', 'public', 'project-map');
  for (const file of fs.readdirSync(dir)) {
    const source = fs.readFileSync(path.join(dir, file), 'utf8');
    for (const forbidden of [/require\(['"](?:node:)?(?:fs|child_process|net|http|https|sqlite3?)['"]\)/, /\bsqlite/i, /work-ledger/, /state\.db/, /\.awr\b/, /\bXMLHttpRequest\b/, /\bfetch\(/, /\beval\(/, /new Function/]) {
      assert.doesNotMatch(source, forbidden, `${file} must stay free of ${forbidden}`);
    }
  }
});
