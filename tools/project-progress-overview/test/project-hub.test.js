'use strict';

const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const hub = require('../public/hub.js');
const dom = require('./fixtures/project-map/mini-dom.js');
const h = require('./fixtures/project-map/helpers.js');

const projects = [
  { key: 'a'.repeat(64), name: 'Alpha', root: '/work/alpha', available: true, last_used_at: 10000 },
  { key: 'b'.repeat(64), name: 'Beta', root: '/work/services', available: true, last_used_at: 30000 },
  { key: 'c'.repeat(64), name: 'Missing', root: '/old/project', available: false, last_used_at: 20000 },
];

function mount(load, locale = 'en') {
  const { doc } = dom.createPage('<input id="hubSearch"><button id="hubRefresh"></button><p id="hubStatus"></p><span id="hubCount"></span><section id="hubProjects"></section><section id="hubMessage"><h2 id="hubMessageTitle"></h2><p id="hubMessageText"></p></section>');
  doc.getElementById('hubSearch').value = '';
  const app = hub.createHub({ document: doc, i18n: { t: h.tFor(locale), locale }, load });
  return { doc, app, el: (id) => doc.getElementById(id) };
}

test('project search matches names and directories and keeps reverse chronology without mutating inputs', () => {
  assert.deepEqual(hub.visibleProjects(projects).map((p) => p.name), ['Beta', 'Missing', 'Alpha']);
  assert.deepEqual(hub.visibleProjects(projects, '  ALPHA  ').map((p) => p.name), ['Alpha']);
  assert.deepEqual(hub.visibleProjects(projects, 'services').map((p) => p.name), ['Beta']);
  assert.deepEqual(projects.map((p) => p.name), ['Alpha', 'Beta', 'Missing']);
  assert.deepEqual(hub.visibleProjects(projects, 'unknown'), []);
});

test('project links bind the registered key and language and reject arbitrary addresses', () => {
  assert.equal(hub.projectHref(projects[0].key, 'zh-CN'), `/?project=${projects[0].key}&lang=zh-CN`);
  assert.equal(hub.projectHref(projects[1].key, 'unsupported'), `/?project=${projects[1].key}&lang=en`);
  for (const value of ['../other', 'javascript:alert(1)', '', 'f'.repeat(63)]) assert.throws(() => hub.projectHref(value, 'en'));
});

test('cards show names, full path metadata, timestamps and explicit unavailable entries in both languages', async () => {
  for (const locale of ['en', 'zh-CN']) {
    const ctx = mount(async () => ({ ok: true, data: { projects } }), locale);
    await ctx.app.refresh();
    assert.deepEqual(ctx.el('hubProjects').querySelectorAll('.project-title').map((el) => el.textContent), ['Beta', 'Missing', 'Alpha']);
    assert.equal(ctx.el('hubProjects').querySelectorAll('a.project-card').length, 2);
    const unavailable = ctx.el('hubProjects').querySelector('[data-available="false"]');
    assert.equal(unavailable.getAttribute('href'), null);
    assert.equal(unavailable.getAttribute('aria-disabled'), 'true');
    assert.ok(unavailable.textContent.includes(h.tFor(locale)('hub.unavailable')));
    assert.equal(ctx.el('hubProjects').querySelector('.project-path').getAttribute('title'), '/work/services');
    assert.equal(ctx.el('hubProjects').querySelector('time').getAttribute('datetime'), new Date(30000).toISOString());
    assert.equal(ctx.el('hubCount').textContent, h.tFor(locale)('hub.count', { count: 3 }));
    ctx.el('hubSearch').value = 'alpha';
    dom.fire(ctx.el('hubSearch'), 'input');
    assert.equal(ctx.el('hubProjects').querySelectorAll('.project-card').length, 1);
  }
});

test('project names and paths are text even when they contain markup', async () => {
  const hostile = [{ ...projects[0], name: '<img src=x onerror=alert(1)>', root: '/work/<script>unsafe</script>' }];
  const ctx = mount(async () => ({ ok: true, data: { projects: hostile } }));
  await ctx.app.refresh();
  assert.equal(ctx.el('hubProjects').querySelector('img'), null);
  assert.equal(ctx.el('hubProjects').querySelector('script'), null);
  assert.equal(ctx.el('hubProjects').querySelector('.project-title').textContent, hostile[0].name);
  assert.equal(ctx.el('hubProjects').querySelector('.project-path').textContent, hostile[0].root);
});

test('empty catalogs, unmatched searches and load failures remain distinguishable and retryable', async () => {
  const empty = mount(async () => ({ ok: true, data: { projects: [] } }));
  await empty.app.refresh();
  assert.equal(empty.el('hubMessageTitle').textContent, h.tFor('en')('hub.empty'));
  const filtered = mount(async () => ({ ok: true, data: { projects } }));
  await filtered.app.refresh();
  filtered.el('hubSearch').value = 'unmatched';
  filtered.app.render();
  assert.equal(filtered.el('hubMessageTitle').textContent, h.tFor('en')('hub.no_matches'));
  let broken = true;
  const retry = mount(() => { if (broken) throw new Error('offline'); return { ok: true, data: { projects } }; });
  await retry.app.refresh();
  assert.equal(retry.el('hubStatus').getAttribute('data-kind'), 'error');
  assert.equal(retry.el('hubMessage').hidden, true);
  broken = false;
  await retry.app.refresh();
  assert.equal(retry.el('hubProjects').querySelectorAll('.project-card').length, 3);
  assert.equal(retry.app.state.error, null);
});

test('overlapping refreshes share one request and malformed entries are reported', async () => {
  let resolve;
  let calls = 0;
  const pending = new Promise((done) => { resolve = done; });
  const ctx = mount(() => { calls += 1; return pending; });
  const first = ctx.app.refresh();
  assert.equal(ctx.app.refresh(), first);
  await Promise.resolve();
  assert.equal(calls, 1);
  resolve({ ok: true, data: { projects } });
  await first;
  const invalid = mount(async () => ({ ok: true, data: { projects: [{ ...projects[0], key: '../outside' }] } }));
  await invalid.app.refresh();
  assert.ok(invalid.app.state.error);
  assert.equal(invalid.el('hubProjects').querySelectorAll('.project-card').length, 0);
});

test('the hub and single-project shells expose localized search and return navigation', () => {
  const publicDir = path.join(__dirname, '..', 'public');
  const html = fs.readFileSync(path.join(publicDir, 'hub.html'), 'utf8');
  for (const key of ['hub.title', 'hub.search_label', 'hub.registry_hint']) {
    assert.ok(html.includes(key));
    for (const lang of ['en', 'zh-CN']) assert.notEqual(h.tFor(lang)(key), key);
  }
  assert.match(fs.readFileSync(path.join(publicDir, 'index.html'), 'utf8'), /id="projectHome"[^>]*data-i18n="hub.back"/);
  assert.match(fs.readFileSync(path.join(publicDir, 'app.js'), 'utf8'), /\/api\/projects\/\$\{encodeURIComponent\(projectKey\)\}/);
  assert.doesNotMatch(html, /src="https?:|href="https?:/);
});
