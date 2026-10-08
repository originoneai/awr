/**
 * Export: the command-line tool writes the file the page would show, built only from the official commands, with the
 * interaction layer inside (or, with --static, without any script). The fake awr answers from a snapshot; the expected
 * output is rendered in-process from the same data. The interactive file is also run against a small DOM to prove it works
 * by itself.
 *
 * Run: node --test test/project-map-export.test.js
 */

'use strict';

const { test, after } = require('node:test');
const assert = require('node:assert/strict');
const { spawnSync } = require('node:child_process');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const h = require('./fixtures/project-map/helpers.js');
const dom = require('./fixtures/project-map/mini-dom.js');
const { createRig, ROOT } = require('./fixtures/project-map/rig.js');
const { createFakeCli } = require('./fixtures/project-map/fake-cli.js');
const { extractSnapshot } = require('../public/project-map/extract.js');
const snapshotLib = require('../public/project-map/snapshot.js');
const page = require('../public/project-map/page.js');
const modelLib = require('../public/project-map/model.js');
const exportMap = require('../export-map.js');

const NOW = '2026-02-03T04:05:06Z';
const source = h.readJson('states.json');
const config = h.readJson('config.json');
const rig = createRig(source, { 'display.json': config });
after(() => rig.cleanup());

function run(args, env = {}) {
  rig.clearCalls();
  const result = spawnSync(process.execPath, [path.join(ROOT, 'export-map.js'), ...args], { cwd: rig.dir, env: rig.env(env), encoding: 'utf8' });
  return { code: result.status, stdout: result.stdout, stderr: result.stderr };
}
const base = (extra = []) => ['--project', rig.project, '--awr', rig.launcher, '--now', NOW, '--config', rig.file('display.json'), ...extra];

/** The page the Inspector would build from the same official answers (with the interaction layer unless `runtime` is false). */
async function expected(lang, { cached = false, options = {}, runtime = true } = {}) {
  const snapshot = await extractSnapshot({ call: createFakeCli(source, options).call, config, now: NOW, cached, wait: async () => {} });
  return { snapshot, result: page.renderProjectMap(snapshot, config, { t: h.tFor(lang), lang, runtime: runtime ? exportMap.loadRuntime(lang) : null }) };
}
/** The script elements of a document (the inlined sources mention the word themselves, so the elements are matched whole). */
const scripts = (html) => html.match(/<script[^>]*>[\s\S]*?<\/script>/g) || [];
const withoutScripts = (html) => html.replace(/<script[^>]*>[\s\S]*?<\/script>/g, '<script></script>');

test('the exported file is exactly what the page renderer produces from the same official answers', async () => {
  const out = rig.file('map.html');
  const r = run(base(['--out', out]));
  assert.equal(r.code, 0, r.stderr);
  const { result } = await expected('en');
  assert.equal(fs.readFileSync(out, 'utf8'), result.document);
  assert.match(r.stderr, /wrote .*map\.html \(20 work items, revision 100, snapshot [0-9a-f]{12}\)/);
});

test('--static writes the diagrams without any script', async () => {
  const out = rig.file('map.static.html');
  assert.equal(run(base(['--out', out, '--static'])).code, 0);
  const html = fs.readFileSync(out, 'utf8');
  assert.equal(html, (await expected('en', { runtime: false })).result.document);
  assert.equal(scripts(html).length, 0);
  assert.doesNotMatch(html, /pm-toolbar|pm-drawer|pm-scope/, 'no places for controls that cannot exist');
  h.assertWellFormed(html, 'static export');
  const interactive = fs.readFileSync(rig.file('map.html'), 'utf8');
  assert.ok(html.length < interactive.length, 'the interactive file is the larger one');
  // a viewer that does not run scripts sees the same diagrams and the same table in both files
  const blocks = (text, tag) => text.match(new RegExp(`<${tag}[\\s>][\\s\\S]*?</${tag}>`, 'g'));
  assert.equal(blocks(html, 'svg').length, 3);
  assert.deepEqual(blocks(withoutScripts(interactive), 'svg'), blocks(html, 'svg'));
  assert.deepEqual(blocks(withoutScripts(interactive), 'table'), blocks(html, 'table'));
  assert.match(interactive, /<div class="pm-toolbar" hidden><\/div>/, 'the controls stay hidden until the script runs');
});

test('the language of the file follows --lang', async () => {
  for (const flags of [[], ['--static']]) {
    const out = rig.file('map.zh.html');
    assert.equal(run(base(['--out', out, '--lang', 'zh-CN', ...flags])).code, 0);
    const { result } = await expected('zh-CN', { runtime: flags.length === 0 });
    const html = fs.readFileSync(out, 'utf8');
    assert.equal(html, result.document);
    assert.match(html, /<html lang="zh-CN">/);
  }
  assert.equal(run(base(['--out', rig.file('x.html'), '--lang', 'zh'])).code, 0, 'zh is accepted as zh-CN');
  const html = fs.readFileSync(rig.file('x.html'), 'utf8');
  const messages = JSON.parse(html.match(/<script type="application\/json" id="pm-data">([\s\S]*?)<\/script>/)[1]).catalogs;
  assert.deepEqual(Object.keys(messages), ['en', 'zh-CN'], 'the chosen language and English as the fallback');
});

test('the file is self-contained and well formed', () => {
  const out = rig.file('map.check.html');
  assert.equal(run(base(['--out', out])).code, 0);
  const html = fs.readFileSync(out, 'utf8');
  assert.equal(scripts(html).length, 2, 'one data block and one script');
  assert.doesNotMatch(html, /<script[^>]*\ssrc=/i, 'no script is loaded from anywhere');
  assert.doesNotMatch(html, /<!\-\-/, 'no comment opener that could change how the script is read');
  assert.doesNotMatch(html, /(?:src|href)="https?:/i);
  assert.doesNotMatch(html, /\sstyle="/, 'the content security policy of the Inspector forbids style attributes, and so do we');
  h.assertWellFormed(withoutScripts(html), 'export');
  assert.match(html, /<main class="pm-root pm-scope" id="pm-root">/, 'the page content is the one main landmark of the file');
  assert.match(html, /<div class="pm-toolbar" hidden><\/div>/, 'the toolbar is a hidden place until the script fills it');
});

test('the data block carries the snapshot, and nothing in the data can end it', async () => {
  const hostile = JSON.parse(JSON.stringify(source));
  hostile.nodes[0].title = 'x</script><img src=x onerror=alert(1)><!-- y';
  const sealed = snapshotLib.seal(hostile, NOW);
  fs.writeFileSync(rig.file('hostile.json'), JSON.stringify(sealed));
  const out = rig.file('hostile.html');
  const r = run(['--snapshot', rig.file('hostile.json'), '--config', rig.file('display.json'), '--out', out]);
  assert.equal(r.code, 0, r.stderr);
  const html = fs.readFileSync(out, 'utf8');
  assert.equal(scripts(html).length, 2);
  assert.doesNotMatch(html, /<img src=x/);
  const data = html.match(/<script type="application\/json" id="pm-data">([\s\S]*?)<\/script>/)[1];
  assert.doesNotMatch(data, /</, 'every < is escaped inside the data block');
  const parsed = JSON.parse(data);
  assert.deepEqual(parsed.snapshot, sealed);
  assert.equal(parsed.snapshot.nodes[0].title, hostile.nodes[0].title);
  assert.deepEqual(parsed.config, config);
  assert.equal(parsed.lang, 'en');
  for (const key of Object.keys(parsed.catalogs.en)) assert.ok(key.startsWith('map.'), 'only the map messages are carried');
});

test('the exported file works by itself: its own script makes the parsed page interactive', async () => {
  const out = rig.file('map.run.html');
  assert.equal(run(base(['--out', out])).code, 0);
  const html = fs.readFileSync(out, 'utf8');
  const { doc } = dom.createPage(html);
  const [data, code] = doc.documentElement.querySelectorAll('script');
  assert.equal(data.getAttribute('id'), 'pm-data');
  const sandbox = { document: doc, setTimeout, clearTimeout, console, TextEncoder, TextDecoder };
  vm.createContext(sandbox);
  vm.runInContext(code.textContent, sandbox, { filename: 'exported-map.html' });

  const root = doc.getElementById('pm-root');
  assert.equal(root.querySelector('.pm-toolbar').hidden, false);
  const { snapshot } = await expected('en');
  const present = new Set([...modelLib.loadModel(snapshot, config).nodes.values()].map((n) => n.vis));
  assert.deepEqual(root.querySelector('.pm-toolbar').querySelectorAll('[data-pm-status]').map((c) => c.getAttribute('data-pm-status')).sort(), [...present].sort(), 'one chip per status that occurs');
  const card = root.querySelector('#s-overview .pm-card[data-key="DEMO-A-004"]');
  assert.equal(card.getAttribute('tabindex'), '0');
  dom.fire(card.querySelector('text'), 'click');
  const panel = root.querySelector('.pm-drawer');
  assert.equal(panel.hidden, false);
  assert.match(panel.querySelector('h2').textContent, /\S/);
  // a fold works from the file alone: the lane is drawn again from the embedded snapshot
  const head = root.querySelector('#s-overview [data-lane="ALPHA"]');
  dom.fire(head.querySelector('text'), 'click');
  assert.equal(root.querySelector('#s-overview [data-lane="ALPHA"]').getAttribute('aria-expanded'), 'false');
  assert.ok(root.querySelectorAll('#s-overview .pm-card').every((c) => !c.getAttribute('data-key').startsWith('DEMO-A-')));
  assert.deepEqual(Object.keys(sandbox.AWR_PROJECT_MAP).sort(), ['detail', 'interact', 'layout', 'model', 'page', 'snapshot', 'svgkit', 'views']);
});

test('a saved snapshot renders to the same bytes without touching awr', async () => {
  const saved = rig.file('snapshot.saved.json');
  const first = rig.file('first.html');
  assert.equal(run(base(['--out', first, '--save-snapshot', saved])).code, 0);
  const snapshot = snapshotLib.validate(JSON.parse(fs.readFileSync(saved, 'utf8')));
  assert.equal(snapshot.generated_at, NOW);
  assert.equal(snapshot.fingerprint, (await expected('en')).snapshot.fingerprint);

  const again = rig.file('again.html');
  const r = run(['--snapshot', saved, '--config', rig.file('display.json'), '--out', again]);
  assert.equal(r.code, 0, r.stderr);
  assert.deepEqual(rig.calls(), [], 'rendering a saved snapshot starts no process');
  assert.equal(fs.readFileSync(again, 'utf8'), fs.readFileSync(first, 'utf8'));
});

test('--out - writes the page to standard output and nothing else', async () => {
  const r = run(base(['--out', '-']));
  assert.equal(r.code, 0);
  assert.equal(r.stdout, (await expected('en')).result.document);
  assert.equal(r.stderr, '');
});

test('only the official read commands run, with --project and --json, and nothing reads the project directory itself', () => {
  fs.writeFileSync(path.join(rig.project, 'work-ledger.yaml'), 'work_items: [THIS MUST NEVER BE READ');
  assert.equal(run(base(['--out', rig.file('audit.html')])).code, 0);
  const calls = rig.calls();
  assert.ok(calls.length >= 8);
  for (const argv of calls) {
    assert.deepEqual(argv.slice(0, 3), ['--project', rig.project, '--json']);
    const [a, b] = argv.slice(3);
    assert.ok(['work graph', 'nav --cached', 'search --type', 'event history', 'session list', 'doctor undefined'].includes(`${a} ${b}`), argv.join(' '));
  }
  const verbs = new Set(calls.flatMap((argv) => argv.slice(3)));
  for (const forbidden of ['reindex', 'complete', 'claim', 'append', 'start', 'end', 'progress', 'block', 'cancel', 'create', 'init']) assert.ok(!verbs.has(forbidden), forbidden);
  fs.rmSync(path.join(rig.project, 'work-ledger.yaml'));
});

test('--cached asks awr for the recorded state and says so when awr cannot', async () => {
  const out = rig.file('cached.html');
  assert.equal(run(base(['--out', out, '--cached'])).code, 0);
  assert.ok(rig.calls().some((argv) => argv[3] === 'work' && argv.includes('--cached')));
  assert.equal(fs.readFileSync(out, 'utf8'), (await expected('en', { cached: true })).result.document);

  const old = run(base(['--out', rig.file('old.html'), '--cached']), { FAKE_OPTIONS: JSON.stringify({ cachedSupported: false }) });
  assert.equal(old.code, 2);
  assert.match(old.stderr, /CachedUnsupported.*--cached/);
  assert.ok(!fs.existsSync(rig.file('old.html')), 'no file is written on failure');
});

test('failures are reported in one line and exit with 2', () => {
  const failing = (args, pattern, env) => {
    const r = run(args, env);
    assert.equal(r.code, 2, args.join(' '));
    assert.match(r.stderr, pattern);
    return r;
  };
  failing(['--bogus'], /unknown option: --bogus/);
  failing(['--out'], /--out needs a value/);
  failing(['--lang', 'fr'], /--lang must be en or zh-CN/);
  failing(['--now', 'yesterday'], /--now must look like/);
  failing(base(['--awr', path.join(rig.dir, 'no-such-awr')]).concat([]), /awr command was not found/);
  failing(['--project', rig.project, '--awr', rig.launcher, '--config', path.join(rig.dir, 'missing.json')], /cannot read missing\.json: no such file/);
  fs.writeFileSync(rig.file('bad.json'), '{"overview":{"lanes":[{"id":"A","name":"A","color":"red"}]}}');
  failing(['--project', rig.project, '--awr', rig.launcher, '--config', rig.file('bad.json')], /display configuration invalid: .*hex color/);
  failing(['--snapshot', rig.file('missing-snapshot.json')], /ENOENT|no such file/);
  fs.writeFileSync(rig.file('tampered.json'), JSON.stringify({ ...source, nodes: source.nodes.map((n, i) => (i ? n : { ...n, title: 'changed' })) }));
  failing(['--snapshot', rig.file('tampered.json')], /fingerprint does not match/);
  // awr answers nothing usable
  failing(base(['--out', rig.file('none.html')]), /NoOutput/, { FAKE_SNAPSHOT: rig.file('does-not-exist.json') });
  assert.equal(run(['--help']).code, 0);
  assert.match(run(['--help']).stdout, /Usage:/);
});

test('main() works in-process with injected streams (used by other tools)', async () => {
  const chunks = { out: '', err: '' };
  const code = await exportMap.main(['--snapshot', rig.file('snapshot.json'), '--out', '-', '--lang', 'en'], { stdout: { write: (s) => { chunks.out += s; } }, stderr: { write: (s) => { chunks.err += s; } } });
  assert.equal(code, 0, chunks.err);
  assert.equal(chunks.out, page.renderProjectMap(source, undefined, { t: h.tFor('en'), lang: 'en', runtime: exportMap.loadRuntime('en') }).document);
});
