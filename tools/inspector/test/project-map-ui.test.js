/**
 * Inspector page for the Project map: the controller against the real bridge and the fake awr, demo mode, errors,
 * downloads, the interaction layer on the drawn map, and the page shell. The page and the export must show the same thing.
 *
 * Run: node --test test/project-map-ui.test.js
 */

'use strict';

const { test, before, after } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const h = require('./fixtures/project-map/helpers.js');
const miniDom = require('./fixtures/project-map/mini-dom.js');
const { createRig } = require('./fixtures/project-map/rig.js');
const { createFakeCli } = require('./fixtures/project-map/fake-cli.js');
const { extractSnapshot } = require('../public/project-map/extract.js');
const snapshotLib = require('../public/project-map/snapshot.js');
const page = require('../public/project-map/page.js');
const ui = require('../public/project-map/ui.js');
const interactLib = require('../public/project-map/interact.js');
const exportMap = require('../export-map.js');
const i18n = require('../public/i18n.js');

const NOW = '2026-02-03T04:05:06Z';
const PUBLIC = path.join(__dirname, '..', 'public');
const source = h.readJson('states.json');
const config = h.readJson('config.json');
const rig = createRig(source, { 'display.json': config });
const GUARD = { 'content-type': 'application/json', 'X-AWR-Inspector': '1' };

/** A minimal DOM: the elements the controller touches, remembering every status text it showed. */
function fakeDom() {
  const ids = ['mapStatus', 'mapRefreshBtn', 'mapCachedToggle', 'mapDownloadBtn', 'mapDownloadJsonBtn', 'mapRetryBtn', 'mapBody', 'mapHeader', 'mapStage'];
  const els = {};
  for (const id of ids) {
    const e = { id, hidden: false, disabled: false, checked: false, innerHTML: '', attrs: {}, listeners: {}, statuses: [] };
    let text = '';
    Object.defineProperty(e, 'textContent', { get: () => text, set: (v) => { text = String(v); e.statuses.push(text); } });
    e.setAttribute = (k, v) => { e.attrs[k] = String(v); };
    e.getAttribute = (k) => (k in e.attrs ? e.attrs[k] : null);
    e.addEventListener = (type, fn) => { (e.listeners[type] = e.listeners[type] || []).push(fn); };
    e.click = () => Promise.all((e.listeners.click || []).map((fn) => fn({ target: e })));
    els[id] = e;
  }
  els.mapBody.hidden = true;
  els.mapRetryBtn.hidden = true;
  return { els, $: (id) => els[id] };
}

const callApiFor = (bridge) => async (route) => (await fetch(`${bridge.base}${route}`, { headers: GUARD })).json();
const saves = [];
const save = (name, mime, text) => saves.push({ name, mime, text });
// the module files the page would fetch from the bridge, read from public/ here
const fetchSource = async (url) => fs.readFileSync(path.join(PUBLIC, url), 'utf8');
// By default the controller runs without the interaction layer, so the stage is a plain element; tests of the layer pass `interact`.
const controller = (dom, callApi, extra = {}) => ui.createProjectMap({
  i18n, $: dom.$, callApi, document: {}, clock: () => NOW, save, extractOptions: { wait: async () => {} }, interact: null, catalogs: h.catalogs, fetchSource, ...extra,
});
/** A page whose stage is a real (mini) DOM element, for the interaction layer. */
function interactivePage() {
  const dom = fakeDom();
  const { doc, container } = miniDom.createPage('');
  dom.els.mapStage = container;
  return { dom, doc, container };
}
const withLocale = async (lang, fn) => {
  i18n.setLocale(lang);
  try { return await fn(); } finally { i18n.setLocale('en'); }
};

let bridge;
let demoBridge;
before(async () => {
  bridge = await rig.startBridge(['--map-config', rig.file('display.json')]);
  demoBridge = await rig.startBridge(['--demo']);
});
after(async () => {
  await bridge.stop();
  await demoBridge.stop();
  rig.cleanup();
});

async function exported(lang, options = {}) {
  const snapshot = await extractSnapshot({ call: createFakeCli(source).call, config, now: NOW, wait: async () => {} });
  return { snapshot, result: page.renderProjectMap(snapshot, config, { t: h.tFor(lang), lang, ...options }) };
}

test('the page shows exactly what the static export shows, read through the real bridge', async () => {
  for (const lang of ['en', 'zh-CN']) {
    await withLocale(lang, async () => {
      const dom = fakeDom();
      const map = controller(dom, callApiFor(bridge));
      await map.refresh();
      const { result } = await exported(lang);
      assert.equal(dom.els.mapHeader.innerHTML, result.header);
      assert.equal(dom.els.mapStage.innerHTML, result.body);
      // the saved file is the same block inside the standalone document
      assert.ok(result.document.includes(dom.els.mapHeader.innerHTML) && result.document.includes(dom.els.mapStage.innerHTML));
      assert.equal(dom.els.mapBody.hidden, false);
      assert.equal(dom.els.mapRetryBtn.hidden, true);
      assert.equal(dom.els.mapDownloadBtn.disabled, false);
      assert.equal(map.state.demo, false);
    });
  }
});

test('progress is announced step by step and ends with what was read', async () => {
  const dom = fakeDom();
  await controller(dom, callApiFor(bridge)).refresh();
  const shown = dom.els.mapStatus.statuses;
  const order = ['Reading the work graph', 'Reading goals', 'Reading navigation', 'Resolving milestones', 'Reading the event history', 'Reading sessions', 'Running doctor', 'Drawing the graphs'];
  let at = -1;
  for (const step of order) {
    const i = shown.findIndex((s, index) => index > at && s.startsWith(step));
    assert.ok(i > at, `${step} not announced in order: ${JSON.stringify(shown)}`);
    at = i;
  }
  const last = shown[shown.length - 1];
  assert.match(last, /^Snapshot r100 · 20 work items · read in [0-9.]+ s /);
  assert.match(last, /Display configuration: display\.json/);
  assert.equal(dom.els.mapStatus.getAttribute('data-kind'), 'info');
  assert.equal(dom.els.mapRefreshBtn.disabled, false);
});

test('a project that changes while it is read is read again and the page says so', async () => {
  const bridgeCall = callApiFor(bridge);
  let navCalls = 0;
  const drifting = async (route) => {
    const envelope = await bridgeCall(route);
    if (route === '/api/map/nav' && envelope.ok && navCalls++ === 0) return { ...envelope, data: { ...envelope.data, project_revision: envelope.data.project_revision + 1 } };
    return envelope;
  };
  const dom = fakeDom();
  await controller(dom, drifting).refresh();
  assert.ok(dom.els.mapStatus.statuses.some((s) => s === 'The project changed while it was being read; reading again (attempt 2)…'), JSON.stringify(dom.els.mapStatus.statuses));
  assert.match(dom.els.mapStatus.textContent, /^Snapshot r100/);
});

test('without a configuration file the status says the lanes were derived', async () => {
  const plain = await rig.startBridge();
  try {
    const dom = fakeDom();
    await controller(dom, callApiFor(plain)).refresh();
    assert.match(dom.els.mapStatus.textContent, /Display configuration derived from the data/);
    assert.match(dom.els.mapStage.innerHTML, /No milestone lanes configured/);
  } finally {
    await plain.stop();
  }
});

test('demo mode draws the built-in project and never starts awr', async () => {
  rig.clearCalls();
  for (const lang of ['en', 'zh-CN']) {
    await withLocale(lang, async () => {
      const dom = fakeDom();
      const map = controller(dom, callApiFor(demoBridge));
      await map.refresh();
      assert.equal(map.state.demo, true);
      assert.match(dom.els.mapStatus.textContent, lang === 'en' ? /Demo data: no live project is connected\./ : /演示数据：未连接真实项目。/);
      assert.ok(dom.els.mapHeader.innerHTML.includes(lang === 'en' ? 'Demo Project' : '示例项目'));
      assert.equal(dom.els.mapBody.hidden, false);
    });
  }
  assert.deepEqual(rig.calls(), []);
});

test('an error is explained in the page language and can be retried', async () => {
  const failing = { current: { ok: false, error: { code: 'RevisionDrift', message: 'moving' } } };
  const callApi = async (route) => (route === '/api/map/config' ? { ok: true, data: { config: null, source: null } } : failing.current);
  const dom = fakeDom();
  const map = controller(dom, callApi);
  await map.refresh();
  assert.match(dom.els.mapStatus.textContent, /The project map could not be read\. The project kept changing while it was being read/);
  assert.equal(dom.els.mapStatus.getAttribute('data-kind'), 'error');
  assert.equal(dom.els.mapRetryBtn.hidden, false);
  assert.equal(dom.els.mapBody.hidden, true, 'nothing stale is shown when nothing was ever loaded');
  assert.equal(dom.els.mapRefreshBtn.disabled, false);

  await withLocale('zh-CN', async () => {
    const zh = fakeDom();
    await controller(zh, callApi).refresh();
    assert.match(zh.els.mapStatus.textContent, /项目地图读取失败。/);
  });

  // an unknown code still shows the code and the message
  failing.current = { ok: false, error: { code: 'Weird', message: 'something odd' } };
  await map.refresh();
  assert.match(dom.els.mapStatus.textContent, /Weird: something odd/);

  // retry: the bridge recovers, the error clears and the page appears
  let down = true;
  const flaky = async (route) => (down ? { ok: false, error: { code: 'BridgeUnreachable', message: 'connection refused' } } : callApiFor(bridge)(route));
  const dom2 = fakeDom();
  const map2 = controller(dom2, flaky);
  await map2.refresh();
  assert.match(dom2.els.mapStatus.textContent, /BridgeUnreachable: connection refused/);
  assert.equal(dom2.els.mapRetryBtn.hidden, false);
  down = false;
  await dom2.els.mapRetryBtn.click();
  assert.equal(dom2.els.mapRetryBtn.hidden, true);
  assert.equal(dom2.els.mapBody.hidden, false);
  assert.match(dom2.els.mapStatus.textContent, /^Snapshot r100/);
});

test('refresh is single-flight: a second click while reading does not start another read', async () => {
  let requests = 0;
  const slow = async (route) => {
    requests += 1;
    await new Promise((resolve) => setTimeout(resolve, 20));
    return callApiFor(bridge)(route);
  };
  const dom = fakeDom();
  const map = controller(dom, slow);
  const first = map.refresh();
  const second = map.refresh();
  assert.equal(first, second);
  await first;
  const once = requests;
  assert.ok(once >= 8);
  await map.refresh();
  assert.equal(requests, once * 2, 'a later refresh reads again');
});

test('show() reads the project the first time only', async () => {
  let requests = 0;
  const counting = async (route) => { requests += 1; return callApiFor(bridge)(route); };
  const dom = fakeDom();
  const map = controller(dom, counting);
  await map.show();
  const once = requests;
  assert.ok(once > 0);
  await map.show();
  assert.equal(requests, once);
});

test('recorded-state mode asks the bridge for work graph --cached and explains an awr that cannot', async () => {
  rig.clearCalls();
  const dom = fakeDom();
  dom.els.mapCachedToggle.checked = true;
  await controller(dom, callApiFor(bridge)).refresh();
  assert.ok(rig.calls().some((argv) => argv[3] === 'work' && argv.includes('--cached')));
  assert.match(dom.els.mapStatus.textContent, /Recorded state: sources were not refreshed for this read\./);

  const old = await rig.startBridge([], { FAKE_OPTIONS: JSON.stringify({ cachedSupported: false }) });
  try {
    const oldDom = fakeDom();
    oldDom.els.mapCachedToggle.checked = true;
    await controller(oldDom, callApiFor(old)).refresh();
    assert.match(oldDom.els.mapStatus.textContent, /This awr version has no work graph --cached\. Turn off “Recorded state only”/);
    // without the toggle the same awr reads with a refresh and works
    const ok = fakeDom();
    await controller(ok, callApiFor(old)).refresh();
    assert.match(ok.els.mapStatus.textContent, /^Snapshot r100/);
  } finally {
    await old.stop();
  }
});

test('the downloads are the file the export writes and the snapshot behind it', async () => {
  saves.length = 0;
  const dom = fakeDom();
  const map = controller(dom, callApiFor(bridge));
  await map.refresh();
  await dom.els.mapDownloadBtn.click();
  await dom.els.mapDownloadJsonBtn.click();
  const { result, snapshot } = await exported('en');
  assert.equal(saves.length, 2);
  assert.equal(saves[0].name, 'awr-project-map-r100.html');
  assert.match(saves[0].mime, /^text\/html/);
  assert.equal(saves[0].text, result.document, 'without the interaction layer the page offers the static file');
  assert.equal(saves[1].name, 'awr-project-map-r100.json');
  const saved = snapshotLib.validate(JSON.parse(saves[1].text));
  assert.equal(saved.fingerprint, snapshot.fingerprint);
});

test('with the interaction layer the page mounts it on the drawn map, exactly as it would be mounted on the exported body', async () => {
  const { dom, container } = interactivePage();
  const map = controller(dom, callApiFor(bridge), { interact: interactLib });
  await map.refresh();
  assert.ok(map.mounted, 'the layer is running');
  assert.equal(container.querySelector('.pm-toolbar').hidden, false);
  assert.ok(container.querySelectorAll('.pm-card').every((card) => card.getAttribute('tabindex') === '0'));

  const { snapshot, result } = await exported('en', { interactive: true });
  const twin = miniDom.createPage(result.body);
  interactLib.mount({ container: twin.container, snapshot, config, t: h.tFor('en'), model: result.model });
  assert.equal(container.innerHTML, twin.container.innerHTML, 'the page and a mount on the export body are the same DOM');

  // and it works: a click on a card opens the details, which come from the same snapshot
  miniDom.fire(container.querySelector('#s-overview .pm-card[data-key="DEMO-A-004"] text'), 'click');
  assert.equal(container.querySelector('.pm-drawer').hidden, false);
  assert.match(container.querySelector('.pm-drawer-body').textContent, /DEMO-A-004/);
});

test('refreshing the map keeps what the reader had folded, zoomed and searched, and leaves no listener behind', async () => {
  const { dom, container } = interactivePage();
  const map = controller(dom, callApiFor(bridge), { interact: interactLib });
  await map.refresh();
  miniDom.fire(container.querySelector('#s-overview [data-lane="BETA"] text'), 'click');
  miniDom.fire(container.querySelector('[data-pm-action="zoom-in"]'), 'click');
  const input = container.querySelector('[data-pm-search]');
  input.value = 'demo-a';
  miniDom.fire(input, 'input');
  const first = map.mounted;
  await map.refresh();
  assert.notEqual(map.mounted, first, 'a new mount for the new snapshot');
  assert.equal(container.querySelector('#s-overview [data-lane="BETA"]').getAttribute('aria-expanded'), 'false');
  assert.equal(map.mounted.state.zoom.overview, 1.1);
  assert.equal(container.querySelector('[data-pm-search]').value, 'demo-a');
  assert.equal(container.listenerCount('click'), 1);
  assert.equal(container.listenerCount('keydown'), 1);
});

test('the interactive page speaks the page language', async () => {
  await withLocale('zh-CN', async () => {
    const { dom, container } = interactivePage();
    await controller(dom, callApiFor(bridge), { interact: interactLib }).refresh();
    assert.match(container.querySelector('.pm-toolbar').textContent, /全部折叠/);
    miniDom.fire(container.querySelector('#s-overview .pm-card[data-key="DEMO-A-004"] text'), 'click');
    assert.match(container.querySelector('.pm-drawer-body').textContent, /概要/);
  });
});

test('downloading the interactive page gives the same bytes as the export command', async () => {
  saves.length = 0;
  for (const lang of ['en', 'zh-CN']) {
    await withLocale(lang, async () => {
      const { dom } = interactivePage();
      const map = controller(dom, callApiFor(bridge), { interact: interactLib });
      await map.refresh();
      await dom.els.mapDownloadBtn.click();
      const { result } = await exported(lang, { runtime: exportMap.loadRuntime(lang) });
      const file = saves.at(-1);
      assert.equal(file.name, 'awr-project-map-r100.html');
      assert.equal(file.text, result.document);
      assert.equal(file.text.match(/<script[^>]*>[\s\S]*?<\/script>/g).length, 2);
    });
  }
});

test('when the module files cannot be read the download falls back to the static page and says so', async () => {
  saves.length = 0;
  const { dom } = interactivePage();
  const map = controller(dom, callApiFor(bridge), { interact: interactLib, fetchSource: async () => { throw new Error('offline'); } });
  await map.refresh();
  await dom.els.mapDownloadBtn.click();
  const { result } = await exported('en');
  assert.equal(saves.at(-1).text, result.document);
  assert.doesNotMatch(saves.at(-1).text, /<script/);
  assert.match(dom.els.mapStatus.textContent, /could not be loaded, so the static page was saved instead/);
});

test('an invalid display configuration is reported instead of drawing something wrong', async () => {
  const callApi = async (route) => (route === '/api/map/config' ? { ok: true, data: { config: { stale_days: 0 }, source: 'bad.json' } } : callApiFor(bridge)(route));
  const dom = fakeDom();
  await controller(dom, callApi).refresh();
  assert.match(dom.els.mapStatus.textContent, /The display configuration is not valid: .*stale_days/);
  assert.equal(dom.els.mapBody.hidden, true);
});

test('the page shell: navigation entry, section, controls, stylesheet and scripts in dependency order', () => {
  const html = fs.readFileSync(path.join(PUBLIC, 'index.html'), 'utf8');
  assert.match(html, /<a href="#map" data-view="map">/);
  assert.match(html, /<section class="view" id="view-map" hidden>/);
  for (const id of ['mapRefreshBtn', 'mapCachedToggle', 'mapDownloadBtn', 'mapDownloadJsonBtn', 'mapStatus', 'mapRetryBtn', 'mapBody', 'mapHeader', 'mapStage']) assert.match(html, new RegExp(`id="${id}"`), id);
  assert.match(html, /<link rel="stylesheet" href="project-map\/interact\.css">\n<link rel="stylesheet" href="project-map\.css">/, 'the layer\'s stylesheet comes first, the page maps its tokens after it');
  assert.match(html, /<div id="mapStage" class="pm-scope"><\/div>/);
  const scripts = [...html.matchAll(/<script src="([^"]+)"><\/script>/g)].map((m) => m[1]);
  const mapScripts = scripts.filter((s) => s.startsWith('project-map/'));
  assert.deepEqual(mapScripts, ['project-map/snapshot.js', 'project-map/model.js', 'project-map/svgkit.js', 'project-map/layout.js', 'project-map/views.js', 'project-map/detail.js', 'project-map/page.js', 'project-map/interact.js', 'project-map/extract.js', 'project-map/demo.js', 'project-map/ui.js']);
  // the page loads the modules the exported file inlines, in the same order
  assert.deepEqual(page.RUNTIME_FILES.map((name) => `project-map/${name}`), mapScripts.filter((s) => !/extract|demo|ui/.test(s)));
  assert.ok(scripts.indexOf('project-map/ui.js') < scripts.indexOf('app.js'), 'the controller is loaded before the app starts');
  for (const s of scripts) assert.ok(fs.existsSync(path.join(PUBLIC, s)), s);
  // the six existing views keep their order, and the map sits before Sources
  assert.deepEqual([...html.matchAll(/<a href="#(\w+)" data-view="\w+"/g)].map((m) => m[1]), ['overview', 'work', 'context', 'mainline', 'map', 'sources', 'team']);
  assert.doesNotMatch(html, /project-map[^>]*(?:http:|https:)/);
});

test('app.js knows the new view and refreshes it through the global button', () => {
  const app = fs.readFileSync(path.join(PUBLIC, 'app.js'), 'utf8');
  assert.match(app, /const VIEWS = \['overview', 'work', 'context', 'mainline', 'map', 'sources', 'team'\];/);
  assert.match(app, /view === 'map' && projectMap\) projectMap\.show\(\)/);
  assert.match(app, /state\.view === 'map' && projectMap\) await projectMap\.refresh\(\)/);
  assert.match(app, /createProjectMap\(\{[^}]*fetchSource: fetchAsset/, 'the page reads its own module files for the download');
});

test('interface texts of the map live in the catalogs: every key resolves, no Han characters in code', () => {
  const dir = path.join(PUBLIC, 'project-map');
  for (const file of fs.readdirSync(dir)) {
    const content = fs.readFileSync(path.join(dir, file), 'utf8');
    assert.doesNotMatch(content, /\p{Script=Han}/u, `${file}: texts belong in the catalogs`);
    for (const [, key] of [...content.matchAll(/\bt\(\s*['"](map\.[^'"]+)['"]/g), ...content.matchAll(/\bt\(\s*`(map\.[^`$]+)`/g)]) {
      assert.ok(Object.hasOwn(h.catalogs.en, key), `${file}: ${key} missing in en`);
      assert.ok(Object.hasOwn(h.catalogs['zh-CN'], key), `${file}: ${key} missing in zh-CN`);
    }
  }
  // templated keys: every status, group, progress step and error code the code can build exists too
  const needed = [];
  for (const vis of ['done', 'developing', 'stalled', 'blocked', 'ready', 'cancelled', 'draft', 'waiting']) needed.push(`map.status.${vis}`);
  for (const vis of ['developing', 'stalled', 'blocked', 'ready', 'draft', 'waiting']) needed.push(`map.group.${vis}`);
  for (const step of ['workGraph', 'goals', 'nav', 'milestones', 'events', 'sessions', 'doctor', 'render', 'retry']) needed.push(`map.progress.${step}`);
  for (const code of ['RevisionDrift', 'CachedUnsupported', 'ProjectTooLarge']) needed.push(`map.error.${code}`);
  for (const entry of snapshotLib.UNAVAILABLE) needed.push(`map.gap.${entry.field.replace(/\./g, '_')}`);
  for (const key of needed) for (const lang of ['en', 'zh-CN']) assert.ok(Object.hasOwn(h.catalogs[lang], key), `${lang}: ${key}`);
});
