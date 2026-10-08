'use strict';

/** Shared helpers of the project-map tests. */

const fs = require('node:fs');
const path = require('node:path');

const FIXTURES = __dirname;
const catalogs = { en: require('../../../public/locales/en.js'), 'zh-CN': require('../../../public/locales/zh-CN.js') };
const page = require('../../../public/project-map/page.js');
const demo = require('../../../public/project-map/demo.js');

const readJson = (name) => JSON.parse(fs.readFileSync(path.join(FIXTURES, name), 'utf8'));
const tFor = (lang) => page.makeT(catalogs, lang);

/** Render the built-in demonstration project in one language. */
function renderDemo(lang, configOverride) {
  const t = tFor(lang);
  const { snapshot, config } = demo.buildDemo(t);
  return page.renderProjectMap(snapshot, configOverride === undefined ? config : configOverride, { t, lang });
}

/** Render a saved snapshot fixture with the display configuration fixture. */
function renderFixture(name, lang, config) {
  return page.renderProjectMap(readJson(`${name}.json`), config === undefined ? readJson('config.json') : config, { t: tFor(lang), lang });
}

/**
 * A small well-formedness check for generated SVG/HTML fragments: tags nest and close, attributes are quoted, and text has no
 * bare `&` or `<`. (Node has no XML parser; the generators only emit this simple subset.)
 */
function assertWellFormed(source, label = 'markup') {
  // The doctype and the stylesheet text (which legitimately contains ">") are not part of the tag structure.
  const markup = source.replace(/^<!doctype html>\s*/i, '').replace(/<style>[^<]*<\/style>/, '<style></style>');
  const stack = [];
  const tag = /<(\/?)([A-Za-z][\w:-]*)((?:\s+[\w:-]+(?:="[^"<]*")?)*)\s*(\/?)>/g;
  let last = 0;
  let m;
  const voids = new Set(['meta', 'input', 'br', 'hr', 'img', 'link']);
  while ((m = tag.exec(markup))) {
    const between = markup.slice(last, m.index);
    if (/[<>]/.test(between)) throw new Error(`${label}: stray angle bracket near ${JSON.stringify(between.slice(-40))}`);
    if (/&(?!(?:amp|lt|gt|quot|#x27|#\d+);)/.test(between)) throw new Error(`${label}: unescaped ampersand near ${JSON.stringify(between.slice(-40))}`);
    last = tag.lastIndex;
    const [, closing, name, , selfClosing] = m;
    if (selfClosing || voids.has(name)) continue;
    if (closing) {
      const open = stack.pop();
      if (open !== name) throw new Error(`${label}: </${name}> closes <${open}>`);
    } else stack.push(name);
  }
  if (/[<>]/.test(markup.slice(last))) throw new Error(`${label}: stray angle bracket at the end`);
  if (stack.length) throw new Error(`${label}: unclosed <${stack.pop()}>`);
}

module.exports = { FIXTURES, catalogs, readJson, tFor, renderDemo, renderFixture, assertWellFormed };
