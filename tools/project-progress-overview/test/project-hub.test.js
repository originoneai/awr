'use strict';

const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const hub = require('../public/hub.js');
const dom = require('./fixtures/project-map/mini-dom.js');
const h = require('./fixtures/project-map/helpers.js');

const projects = [
  { key: 'a'.repeat(64), name: 'Alpha', root: '/work/alpha', available: true, last_used_at: 10000 },
  { key: 'b'.repeat(64), name: 'Beta', root: '/work/services', available: true, last_used_at: 30000 },
  { key: 'c'.repeat(64), name: 'Missing', root: '/old/project', available: false, last_used_at: 20000 },
];

function mount(load, locale = 'en', options = {}) {
  const { doc } = dom.createPage(fs.readFileSync(path.join(__dirname, '..', 'public', 'hub.html'), 'utf8'));
  doc.getElementById('hubSearch').value = '';
  const dialog = doc.getElementById('hubEnrollDialog');
  dialog.showModal = () => dialog.setAttribute('open', '');
  dialog.close = () => dialog.removeAttribute('open');
  const app = hub.createHub({ document: doc, i18n: { t: h.tFor(locale), locale }, load, ...options });
  return { doc, app, el: (id) => doc.getElementById(id) };
}

test('argv quoting keeps ordinary tokens and quotes unsafe paths', () => {
  assert.equal(hub.formatArgv(['awr', '--project', '/work/alpha', 'init', '--json']), 'awr --project /work/alpha init --json');
  assert.equal(hub.formatArgv(['awr', '--project', '/work/my project', 'init', '--json']), "awr --project '/work/my project' init --json");
});

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
    assert.equal(ctx.el('hubProjects').querySelectorAll('a.project-card-open').length, 2);
    const unavailable = ctx.el('hubProjects').querySelector('[data-available="false"]');
    assert.equal(unavailable.getAttribute('href'), null);
    assert.equal(unavailable.querySelector('.project-card-content').getAttribute('aria-disabled'), 'true');
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

test('discovered and repair cards retain identity-bound intake links and honest timestamps', async () => {
  const pending = { ...projects[0], state: 'discovered', last_used_at: 0, registered_at: 40000 };
  const repair = { ...projects[1], state: 'attention' };
  for (const locale of ['en', 'zh-CN']) {
    const ctx = mount(async () => ({ ok: true, data: { projects: [pending, repair] } }), locale);
    await ctx.app.refresh();
    for (const project of [pending, repair]) {
      const card = ctx.el('hubProjects').querySelector(`[data-project="${project.key}"]`);
      assert.equal(card.querySelector('a').getAttribute('href'), `/?project=${project.key}&lang=${locale}&intake=1`);
      assert.equal(card.getAttribute('data-state'), project.state);
    }
    const time = ctx.el('hubProjects').querySelector('[data-state="discovered"] time');
    assert.equal(time.getAttribute('datetime'), new Date(pending.registered_at).toISOString());
    assert.ok(time.getAttribute('title').includes(h.tFor(locale)('hub.first_seen')));
    assert.ok(ctx.el('hubProjects').textContent.includes(h.tFor(locale)('hub.pending')));
    assert.ok(ctx.el('hubProjects').textContent.includes(h.tFor(locale)('hub.attention')));
  }
});

test('coverage shows limits and isolated scope and quiet refresh preserves card focus and expanded details', async () => {
  const report = { state: 'partial', roots: ['/work'], exclude_names: ['node_modules'], directories_scanned: 50, connected: 1, pending: 2, attention: 1, interval_seconds: 60, issues: [{ path: '/work/private', reason: 'permission denied' }], omitted_issues: 3 };
  const ctx = mount(async () => ({ ok: true, data: { projects, discovery: report, catalog_scope: 'isolated' } }));
  await ctx.app.refresh();
  assert.equal(ctx.el('hubScope').hidden, false);
  assert.ok(ctx.el('hubCoverage').textContent.includes('/work/private'));
  assert.ok(ctx.el('hubCoverage').textContent.includes('node_modules'));
  assert.ok(ctx.el('hubCoverage').textContent.includes(h.tFor('en')('hub.discovery_partial')));
  const card = ctx.el('hubProjects').querySelector('a');
  card.focus();
  ctx.el('hubCoverage').querySelector('details').setAttribute('open', '');
  await ctx.app.refresh({ quiet: true });
  assert.equal(ctx.el('hubProjects').querySelector('a'), card);
  assert.equal(ctx.doc.activeElement, card);
  report.directories_scanned += 10;
  await ctx.app.refresh({ quiet: true });
  assert.equal(ctx.el('hubCoverage').querySelector('details').hasAttribute('open'), true);
});

test('intake promotes the same card to connected with language-preserving progress and return navigation', async () => {
  for (const locale of ['en', 'zh-CN']) {
    const project = { ...projects[0], state: 'discovered' };
    const ctx = mount(async () => ({ ok: true, data: { projects: [project] } }), locale, {
      selectedKey: project.key,
      loadIntake: async (key) => ({ ok: true, data: { project: { ...project, key } } }),
    });
    await ctx.app.refresh();
    assert.equal(ctx.el('hubIntake').hidden, false);
    assert.equal(ctx.el('hubProjects').hidden, true);
    assert.equal(ctx.el('hubControls').hidden, true);
    assert.equal(ctx.el('hubIntakeOpen').hidden, true);
    assert.equal(ctx.el('hubIntakeBack').getAttribute('href'), `/?lang=${locale}`);
    assert.ok(ctx.el('hubIntakePrompt').textContent.includes(project.root));
    assert.equal(ctx.el('hubIntakeGuidance').hidden, true);
    project.state = 'connected';
    await ctx.app.refresh({ quiet: true });
    assert.equal(ctx.el('hubIntakeState').textContent, h.tFor(locale)('hub.connected'));
    assert.equal(ctx.el('hubIntakeOpen').hidden, false);
    assert.equal(ctx.el('hubIntakeOpen').getAttribute('href'), `/?project=${project.key}&lang=${locale}`);
  }
});

test('intake shows the inspect next command and copies it with the agent prompt', async () => {
  const project = { ...projects[0], state: 'discovered', name: '<b>Alpha</b>' };
  const copied = [];
  const hostile = '/work/<script>unsafe</script>';
  const ctx = mount(async () => ({ ok: true, data: { projects: [project] } }), 'en', {
    selectedKey: project.key,
    copy: async (text) => { copied.push(text); },
    loadIntake: async (key) => ({
      ok: true,
      data: {
        project: { ...project, key },
        guidance: {
          condition: 'project_has_no_awr_runtime',
          next_action: {
            argv: ['awr', '--project', hostile, 'init', '--json'],
            instruction: 'Preview existing sources.',
          },
          recheck: { trigger: 'after initialization', argv: ['awr', '--project', '/work/alpha', 'overview', 'inspect', '--json'] },
        },
      },
    }),
  });
  await ctx.app.refresh();
  const command = `awr --project '${hostile}' init --json`;
  assert.equal(ctx.el('hubIntakeGuidance').hidden, false);
  assert.equal(ctx.el('hubIntakeCommandLabel').textContent, h.tFor('en')('hub.intake_next'));
  assert.equal(ctx.el('hubIntakeCommand').textContent, command);
  assert.equal(ctx.el('hubIntakeInstruction').textContent, 'Preview existing sources.');
  assert.equal(ctx.el('hubIntakeRecheck').textContent, h.tFor('en')('hub.intake_recheck', { trigger: 'after initialization' }));
  assert.equal(ctx.el('hubIntakeTitle').querySelector('b'), null);
  assert.equal(ctx.el('hubIntakeCommand').querySelector('script'), null);
  ctx.el('hubIntakeCopy').click();
  await Promise.resolve();
  assert.equal(copied.length, 1);
  assert.match(copied[0], /Use AWR to take over the project at \/work\/alpha/);
  assert.ok(copied[0].includes(command));
  assert.match(copied[0], /Preview existing sources/);
  assert.match(copied[0], /after initialization/);
});

test('intake rejects cross-project replies and removes stale details after disconnection', async () => {
  let returned = { ...projects[0], state: 'attention', issue: 'legacy schema requires supported migration' };
  const ctx = mount(async () => ({ ok: true, data: { projects } }), 'en', {
    selectedKey: projects[0].key,
    loadIntake: async () => ({ ok: true, data: { project: returned } }),
  });
  await ctx.app.refresh();
  assert.equal(ctx.el('hubIntakeDiagnostic').hidden, false);
  assert.equal(ctx.el('hubIntakeIssue').textContent, returned.issue);
  returned = projects[1];
  await ctx.app.refresh({ quiet: true });
  assert.ok(ctx.app.state.error);
  assert.equal(ctx.el('hubIntake').hidden, true);
  returned = projects[0];
  await ctx.app.refresh({ quiet: true });
  assert.equal(ctx.app.state.error, null);
  assert.equal(ctx.el('hubIntake').hidden, false);
});

test('quiet recovery rebuilds the same catalog after a failed poll', async () => {
  let offline = false;
  const ctx = mount(() => { if (offline) throw new Error('offline'); return { ok: true, data: { projects } }; });
  await ctx.app.refresh();
  offline = true;
  await ctx.app.refresh({ quiet: true });
  assert.equal(ctx.el('hubProjects').querySelectorAll('.project-card').length, 0);
  offline = false;
  await ctx.app.refresh({ quiet: true });
  assert.equal(ctx.el('hubProjects').querySelectorAll('.project-card').length, 3);
});

test('explicit takeover creates one pending card even when a prior catalog poll is stale', async () => {
  const pending = { ...projects[0], key: 'd'.repeat(64), name: 'New app', root: '/work/new app', state: 'discovered', intake_requested: true, last_used_at: 0, registered_at: 40000 };
  for (const locale of ['en', 'zh-CN']) {
    let complete;
    const requests = [];
    const ctx = mount(async () => ({ ok: true, data: { projects: projects.slice() } }), locale, {
      enroll: (root) => { requests.push(root); return new Promise((resolve) => { complete = resolve; }); },
    });
    await ctx.app.refresh();
    dom.fire(ctx.el('hubEnroll'), 'click');
    assert.equal(ctx.el('hubEnrollDialog').hasAttribute('open'), true);
    assert.equal(ctx.doc.activeElement, ctx.el('hubEnrollPath'));
    ctx.el('hubEnrollPath').value = '  /work/new app  ';
    dom.fire(ctx.el('hubEnrollForm'), 'submit');
    dom.fire(ctx.el('hubEnrollForm'), 'submit');
    assert.deepEqual(requests, ['/work/new app']);
    assert.equal(ctx.el('hubEnrollSubmit').disabled, true);
    complete({ ok: true, data: { project: pending } });
    await new Promise(setImmediate);
    assert.equal(ctx.el('hubEnrollDialog').hasAttribute('open'), false);
    assert.equal(ctx.el('hubEnrollSubmit').disabled, false);
    const card = ctx.el('hubProjects').querySelector(`[data-project="${pending.key}"]`);
    assert.equal(card.querySelector('a').getAttribute('href'), `/?project=${pending.key}&lang=${locale}&intake=1`);
    assert.ok(card.textContent.includes(h.tFor(locale)('hub.intake_card_hint')));
    assert.ok(card.querySelector('time').getAttribute('title').includes(h.tFor(locale)('hub.first_added')));
  }
});

test('invalid takeover paths stay in the dialog for correction and cancellation creates no card', async () => {
  let calls = 0;
  const ctx = mount(async () => ({ ok: true, data: { projects: [] } }), 'zh-CN', {
    enroll: async () => { calls += 1; return { ok: false, error: { code: 'InvalidInput' } }; },
  });
  dom.fire(ctx.el('hubEnroll'), 'click');
  ctx.el('hubEnrollPath').value = 'relative/path';
  dom.fire(ctx.el('hubEnrollForm'), 'submit');
  await new Promise(setImmediate);
  assert.equal(ctx.el('hubEnrollDialog').hasAttribute('open'), true);
  assert.equal(ctx.el('hubEnrollPath').value, 'relative/path');
  assert.equal(ctx.el('hubEnrollError').hidden, false);
  assert.equal(ctx.el('hubEnrollError').textContent, h.tFor('zh-CN')('hub.enroll_failed'));
  assert.equal(ctx.app.state.projects.length, 0);
  dom.fire(ctx.el('hubEnrollCancel'), 'click');
  assert.equal(ctx.el('hubEnrollDialog').hasAttribute('open'), false);
  assert.equal(calls, 1);
});

test('card deletion is localized, separate from navigation and only removes a confirmed card', async () => {
  for (const locale of ['en', 'zh-CN']) {
    const requests = [];
    let finish;
    const ctx = mount(async () => ({ ok: true, data: { projects } }), locale, {
      remove: (key) => { requests.push(key); return new Promise((resolve) => { finish = resolve; }); },
    });
    await ctx.app.refresh();
    assert.equal(ctx.el('hubProjects').querySelectorAll('.project-delete').length, 3);
    const card = ctx.el('hubProjects').querySelector(`[data-project="${projects[0].key}"]`);
    const button = card.querySelector('.project-delete');
    assert.ok(button.parent === card);
    assert.equal(button.textContent, h.tFor(locale)('hub.remove'));
    assert.equal(button.getAttribute('aria-label'), h.tFor(locale)('hub.remove_label', { name: 'Alpha' }));
    assert.equal(button.getAttribute('title'), h.tFor(locale)('hub.remove_hint'));
    dom.fire(button, 'click');
    dom.fire(button, 'click');
    assert.deepEqual(requests, [projects[0].key]);
    assert.equal(button.disabled, true);
    assert.equal(ctx.el('hubRefresh').disabled, true);
    assert.equal(ctx.app.state.projects.length, 3);
    finish({ ok: true });
    await new Promise(setImmediate);
    assert.equal(ctx.app.state.projects.length, 2);
    assert.equal(ctx.el('hubProjects').querySelector(`[data-project="${projects[0].key}"]`), null);
    assert.equal(ctx.el('hubRefresh').disabled, false);
  }
});

test('failed deletion retains the card and allows a retry', async () => {
  let succeeded = false;
  const ctx = mount(async () => ({ ok: true, data: { projects } }), 'en', {
    remove: async () => ({ ok: succeeded }),
  });
  await ctx.app.refresh();
  const button = ctx.el('hubProjects').querySelector('.project-delete');
  dom.fire(button, 'click');
  await new Promise(setImmediate);
  assert.equal(ctx.app.state.projects.length, 3);
  assert.equal(ctx.el('hubStatus').textContent, h.tFor('en')('hub.remove_failed'));
  assert.equal(button.disabled, false);
  succeeded = true;
  dom.fire(button, 'click');
  await new Promise(setImmediate);
  assert.equal(ctx.app.state.projects.length, 2);
  assert.equal(ctx.el('hubStatus').textContent, '');
});

test('deletion waits for an in-flight list response so it cannot restore a removed card', async () => {
  let delayed = false;
  let finish;
  let calls = 0;
  const ctx = mount(() => delayed ? new Promise((resolve) => { finish = resolve; }) : { ok: true, data: { projects } }, 'en', {
    remove: async () => { calls += 1; return { ok: true }; },
  });
  await ctx.app.refresh();
  delayed = true;
  const reading = ctx.app.refresh();
  await new Promise(setImmediate);
  const button = ctx.el('hubProjects').querySelector('.project-delete');
  dom.fire(button, 'click');
  assert.equal(calls, 0);
  assert.equal(ctx.app.refresh(), reading);
  finish({ ok: true, data: { projects } });
  await reading;
  await new Promise(setImmediate);
  assert.equal(calls, 1);
  assert.equal(ctx.app.state.projects.some((project) => project.key === projects[1].key), false);
});

test('manual project refresh scans once while local search performs no requests', async () => {
  let loads = 0;
  let scans = 0;
  const ctx = mount(async () => { loads += 1; return { ok: true, data: { projects: [projects[0]] } }; }, 'en', {
    rescan: async () => { scans += 1; return { ok: true, data: { projects } }; },
  });
  await ctx.app.refresh();
  assert.equal(loads, 1);
  assert.equal(scans, 0);
  dom.fire(ctx.el('hubRefresh'), 'click');
  await new Promise(setImmediate);
  assert.equal(loads, 1);
  assert.equal(scans, 1);
  ctx.el('hubSearch').value = 'Alpha';
  dom.fire(ctx.el('hubSearch'), 'input');
  assert.equal(ctx.el('hubProjects').querySelectorAll('.project-card').length, 1);
  assert.equal(loads, 1);
  assert.equal(scans, 1);
});

test('browser hub loads once without periodic or visibility-triggered refresh', async () => {
  const { doc } = dom.createPage(fs.readFileSync(path.join(__dirname, '..', 'public', 'hub.html'), 'utf8'));
  let loads = 0;
  const context = {
    document: doc, URL, location: { href: 'http://127.0.0.1:7381/' },
    AWR_I18N: { t: h.tFor('en'), locale: 'en' },
    navigator: { clipboard: { writeText() {} } },
    fetch: async () => { loads += 1; return { json: async () => ({ ok: true, data: { projects } }) }; },
    setInterval() { throw new Error('catalog polling must stay disabled'); },
    setTimeout() { throw new Error('catalog refresh must be user-triggered'); },
  };
  vm.runInNewContext(fs.readFileSync(path.join(__dirname, '..', 'public', 'hub.js'), 'utf8'), context);
  await new Promise(setImmediate);
  assert.equal(loads, 1);
  dom.fire(doc, 'visibilitychange');
  await new Promise(setImmediate);
  assert.equal(loads, 1);
});

test('detail entries and history restoration read current state using uncached project-bound requests', async () => {
  let shows = 0;
  let refreshes = 0;
  let callApi;
  const listeners = {};
  const requests = [];
  const elements = { projectHome: {}, connectionStatus: { dataset: {} } };
  const context = {
    URL, location: { href: `http://127.0.0.1:7381/?project=${projects[0].key}` },
    document: { getElementById: (id) => elements[id] },
    AWR_I18N: { t: h.tFor('en'), locale: 'en' },
    AWR_PROJECT_MAP: { ui: { createProjectMap(options) {
      callApi = options.callApi;
      return { show() { shows += 1; }, refresh() { refreshes += 1; } };
    } } },
    addEventListener: (name, listener) => { listeners[name] = listener; },
    fetch: async (url, options) => { requests.push({ url, options }); return { json: async () => ({ ok: true, data: {} }) }; },
  };
  const source = fs.readFileSync(path.join(__dirname, '..', 'public', 'app.js'), 'utf8');
  vm.runInNewContext(source, context);
  assert.equal(shows, 1);
  await callApi('/api/map/snapshot');
  listeners.pageshow({ persisted: false });
  assert.equal(refreshes, 0);
  listeners.pageshow({ persisted: true });
  assert.equal(refreshes, 1);
  vm.runInNewContext(source, context);
  assert.equal(shows, 2);
  await callApi('/api/map/snapshot');
  for (const request of requests) {
    assert.equal(request.url, `/api/projects/${projects[0].key}/map/snapshot`);
    assert.equal(request.options.cache, 'no-store');
  }
});

test('overview project switcher renders sorted localized options, states, disabled unavailable entries and XSS-safe names', async () => {
  const { createProjectSwitcher } = require('../public/app.js');
  const html = fs.readFileSync(path.join(__dirname, '..', 'public', 'index.html'), 'utf8');
  const pendingKey = 'd'.repeat(64);
  const repairKey = 'e'.repeat(64);
  const catalog = [
    { ...projects[0], name: '<img src=x onerror=alert(1)>', state: 'connected' },
    projects[1],
    { ...projects[2] },
    { key: pendingKey, name: 'Pending', root: '/work/pending', available: true, last_used_at: 0, registered_at: 40000, state: 'discovered' },
    { key: repairKey, name: 'Repair', root: '/work/repair', available: true, last_used_at: 25000, state: 'attention' },
  ];
  const before = catalog.map((p) => p.key);
  for (const locale of ['en', 'zh-CN']) {
    const t = h.tFor(locale);
    const { doc } = dom.createPage(html);
    const select = doc.getElementById('mapProjectSelect');
    assert.equal(select.hidden, true);
    const switcher = createProjectSwitcher({
      document: doc, i18n: { t, locale }, currentKey: projects[0].key,
      load: async () => ({ ok: true, data: { projects: catalog } }), navigate() {},
    });
    assert.equal(await switcher.refresh(), true);
    assert.equal(select.hidden, false);
    assert.equal(select.value, projects[0].key);
    assert.equal(select.querySelector('img'), null);
    const options = select.querySelectorAll('option');
    assert.equal(options[0].textContent, t('map.project_all', { count: 5 }));
    assert.deepEqual(options.slice(1).map((o) => o.value), [projects[1].key, repairKey, projects[2].key, projects[0].key, pendingKey]);
    assert.deepEqual(options.slice(1).map((o) => o.textContent), [
      t('map.project_option', { name: 'Beta', state: t('map.project_connected') }),
      t('map.project_option', { name: 'Repair', state: t('hub.attention') }),
      t('map.project_option', { name: 'Missing', state: t('hub.unavailable') }),
      t('map.project_option', { name: '<img src=x onerror=alert(1)>', state: t('map.project_connected') }),
      t('map.project_option', { name: 'Pending', state: t('hub.pending') }),
    ]);
    assert.equal(options[3].disabled, true);
    assert.equal(options[4].disabled, false);
  }
  assert.deepEqual(catalog.map((p) => p.key), before);
});

test('overview project switcher routes selections and guards same-project, unavailable and invalid keys', async () => {
  const { createProjectSwitcher } = require('../public/app.js');
  const html = fs.readFileSync(path.join(__dirname, '..', 'public', 'index.html'), 'utf8');
  const pendingKey = 'd'.repeat(64);
  const repairKey = 'e'.repeat(64);
  const catalog = [
    ...projects,
    { key: pendingKey, name: 'Pending', root: '/work/pending', available: true, last_used_at: 0, state: 'discovered' },
    { key: repairKey, name: 'Repair', root: '/work/repair', available: true, last_used_at: 25000, state: 'attention' },
  ];
  for (const locale of ['en', 'zh-CN']) {
    const { doc } = dom.createPage(html);
    const select = doc.getElementById('mapProjectSelect');
    const navigated = [];
    const switcher = createProjectSwitcher({
      document: doc, i18n: { t: h.tFor(locale), locale }, currentKey: projects[0].key,
      load: async () => ({ ok: true, data: { projects: catalog } }),
      navigate: (href) => navigated.push(href),
    });
    await switcher.refresh();
    for (const [value, expected] of [
      [projects[0].key, null],
      ['', `/?lang=${locale}`],
      [projects[1].key, `/?project=${projects[1].key}&lang=${locale}`],
      [pendingKey, `/?project=${pendingKey}&lang=${locale}&intake=1`],
      [repairKey, `/?project=${repairKey}&lang=${locale}&intake=1`],
    ]) {
      const before = navigated.length;
      select.value = value;
      dom.fire(select, 'change');
      assert.equal(navigated.length, expected ? before + 1 : before);
      if (expected) assert.equal(navigated.at(-1), expected);
    }
    const count = navigated.length;
    for (const blocked of [projects[2].key, '../other', 'javascript:alert(1)']) {
      select.value = blocked;
      dom.fire(select, 'change');
      assert.equal(navigated.length, count);
      assert.equal(select.value, projects[0].key);
    }
  }
});

test('overview bootstrap skips switcher in standalone mode, hides on empty/bad replies, and refreshes on persisted pageshow', async () => {
  const source = fs.readFileSync(path.join(__dirname, '..', 'public', 'app.js'), 'utf8');
  const html = fs.readFileSync(path.join(__dirname, '..', 'public', 'index.html'), 'utf8');
  const runApp = async (href, reply) => {
    const { doc } = dom.createPage(html);
    const fetches = [];
    const listeners = {};
    let mapRefreshes = 0;
    vm.runInNewContext(source, {
      URL, location: { href, assign() {} }, document: doc,
      AWR_I18N: { t: h.tFor('en'), locale: 'en' },
      AWR_PROJECT_MAP: { ui: { createProjectMap: () => ({ show() {}, refresh() { mapRefreshes += 1; } }) } },
      addEventListener: (name, fn) => { listeners[name] = fn; },
      fetch: async (url, options) => { fetches.push({ url, options }); return { json: async () => reply() }; },
    });
    await new Promise(setImmediate);
    return { select: doc.getElementById('mapProjectSelect'), fetches, listeners, get mapRefreshes() { return mapRefreshes; } };
  };

  const standalone = await runApp('http://127.0.0.1:7381/?demo=1', () => ({ ok: true, data: { projects } }));
  assert.deepEqual(standalone.fetches, []);
  assert.equal(standalone.select.hidden, true);

  for (const bad of [
    () => ({ ok: true, data: { projects: [] } }),
    () => ({ ok: false }),
    () => ({ ok: true, data: { projects: [{ ...projects[0], key: '../bad' }] } }),
    () => { throw new Error('offline'); },
  ]) {
    const degraded = await runApp(`http://127.0.0.1:7381/?project=${projects[0].key}`, bad);
    assert.equal(degraded.select.hidden, true);
  }

  const live = await runApp(`http://127.0.0.1:7381/?project=${projects[0].key}`, () => ({ ok: true, data: { projects } }));
  assert.equal(live.fetches.length, 1);
  assert.equal(live.fetches[0].url, '/api/projects');
  assert.equal(live.select.hidden, false);
  live.listeners.pageshow({ persisted: false });
  await new Promise(setImmediate);
  assert.equal(live.mapRefreshes, 0);
  assert.equal(live.fetches.length, 1);
  live.listeners.pageshow({ persisted: true });
  await new Promise(setImmediate);
  assert.equal(live.mapRefreshes, 1);
  assert.equal(live.fetches.length, 2);
});
