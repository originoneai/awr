/** Verify message completeness, language selection, and project-content boundaries. */
'use strict';
const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const en = require('../public/locales/en.js');
const zh = require('../public/locales/zh-CN.js');
const source = fs.readFileSync(path.join(__dirname, '../public/i18n.js'), 'utf8');

function browser({ url = 'http://localhost/?demo=1#work', saved, languages = ['en-US'], blocked = false } = {}) {
  const events = {};
  const picker = { value: '', addEventListener: (event, handler) => { events[event] = handler; } };
  const text = { textContent: '', getAttribute: () => 'language.label' };
  const attribute = { getAttribute: () => 'language.label', setAttribute: (name, value) => { attribute[name] = value; } };
  const storage = new Map(saved ? [['awr-inspector-language', saved]] : []);
  let assigned;
  const context = vm.createContext({
    URL, AWR_LOCALES: { en, 'zh-CN': zh },
    location: { href: url, assign: (value) => { assigned = value; } },
    navigator: { languages },
    localStorage: {
      getItem: (key) => { if (blocked) throw Error('blocked'); return storage.get(key); },
      setItem: (key, value) => { if (blocked) throw Error('blocked'); storage.set(key, value); },
    },
    document: {
      documentElement: {}, getElementById: () => picker,
      querySelectorAll: (selector) => selector === '[data-i18n]' ? [text]
        : selector === '[data-i18n-aria-label]' ? [attribute] : [],
    },
  });
  vm.runInContext(source, context);
  return { api: context.AWR_I18N, context, text, attribute, picker, events, storage, get assigned() { return assigned; } };
}

test('catalogs have matching keys and interpolation parameters', () => {
  assert.deepEqual(Object.keys(en).sort(), Object.keys(zh).sort());
  const params = (value) => [...value.matchAll(/\{([A-Za-z0-9_]+)\}/g)].map((m) => m[1]).sort();
  for (const key of Object.keys(en)) {
    assert.equal(typeof en[key], 'string', key);
    assert.ok(en[key].trim(), key);
    assert.ok(zh[key].trim(), key);
    assert.deepEqual(params(en[key]), params(zh[key]), key);
    assert.doesNotMatch(en[key], /\p{Script=Han}/u, key);
  }
});

test('all static translation references resolve', () => {
  for (const name of ['app.js', 'index.html']) {
    const content = fs.readFileSync(path.join(__dirname, '../public', name), 'utf8');
    const keys = [...content.matchAll(/\bt\(['"]([^'"]+)['"]/g), ...content.matchAll(/data-i18n(?:-[a-z-]+)?="([^"]+)"/g)];
    assert.ok(keys.length, name);
    for (const [, key] of keys) assert.ok(Object.hasOwn(en, key), `${name}: ${key}`);
    assert.doesNotMatch(content, /\p{Script=Han}/u, `${name}: interface text belongs in catalogs`);
  }
});

test('URL overrides storage, then supported browser preferences, then English', () => {
  assert.equal(browser({ url: 'http://localhost/?lang=zh-CN', saved: 'en' }).api.locale, 'zh-CN');
  assert.equal(browser({ saved: 'en', languages: ['zh-CN'] }).api.locale, 'en');
  assert.equal(browser({ languages: ['fr', 'zh-CN'] }).api.locale, 'zh-CN');
  assert.equal(browser({ saved: 'unsupported', languages: ['fr'] }).api.locale, 'en');
  assert.equal(browser({ blocked: true, languages: ['zh-CN'] }).api.locale, 'zh-CN');
});

test('language switch updates storage and preserves query parameters and view', () => {
  const page = browser();
  page.picker.value = 'zh-CN';
  page.events.change();
  assert.equal(page.storage.get('awr-project-progress-overview-language'), 'zh-CN');
  const next = new URL(page.assigned);
  assert.equal(next.searchParams.get('demo'), '1');
  assert.equal(next.searchParams.get('lang'), 'zh-CN');
  assert.equal(next.hash, '#work');
  const reloaded = browser({ url: next.href });
  assert.equal(reloaded.context.document.documentElement.lang, 'zh-CN');
  assert.equal(reloaded.text.textContent, zh['language.label']);
  assert.equal(reloaded.attribute['aria-label'], zh['language.label']);
  assert.equal(reloaded.picker.value, 'zh-CN');
});

test('switch still works when local storage is blocked', () => {
  const page = browser({ blocked: true });
  page.picker.value = 'zh-CN';
  page.events.change();
  assert.equal(browser({ blocked: true, url: page.assigned }).api.locale, 'zh-CN');
});

test('interpolation preserves multilingual source content and replacement metacharacters', () => {
  const page = browser();
  const key = 'map.page.title';
  const value = '项目 α 🧭 $& <script>';
  for (const locale of ['en', 'zh-CN']) {
    page.api.setLocale(locale);
    assert.ok(page.api.t(key, { name: value }).includes(value));
    assert.equal(page.api.t('unknown.key'), 'unknown.key');
  }
});


test('missing Chinese messages fall back to English', () => {
  const page = browser({ url: 'http://localhost/?lang=zh-CN' });
  page.context.AWR_LOCALES['zh-CN'] = { ...zh };
  delete page.context.AWR_LOCALES['zh-CN']['language.label'];
  assert.equal(page.api.t('language.label'), en['language.label']);
});
