import { createElement } from 'react';
import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { PROTOTYPE_ICONS, type PrototypeIconName } from './prototype-icons';
import { comparableNodes, fixtureNodes, staticSvgNodes } from './static-svg-nodes';

/*
 * The fixture is the prototype's own ICONS table (panorama src3/app.js), verbatim:
 * every row is the exact string the prototype draws — path data, or 'c cx cy r' for a
 * circle. Generated once by extracting the ICONS literal from app.js and writing the
 * JSON (see the commit message); this test is what makes the committed registry equal
 * to that table, glyph by glyph, node by node.
 */
const FIXTURE = JSON.parse(readFileSync('src/test-fixtures/prototype-icons.json', 'utf8')) as Record<string, string[]>;

const PROTOTYPE_SVG = {
  viewBox: '0 0 24 24',
  fill: 'none',
  stroke: 'currentColor',
  'stroke-width': '1.5',
  'stroke-linecap': 'round',
  'stroke-linejoin': 'round',
  width: '16',
  height: '16',
} as const;

describe('the prototype icon registry', () => {
  it('carries exactly the prototype ICONS table, keyed by the prototype names', () => {
    expect(Object.keys(PROTOTYPE_ICONS).sort()).toEqual(Object.keys(FIXTURE).sort());
  });

  it('renders every glyph with the prototype svg parameters', () => {
    for (const [name, Icon] of Object.entries(PROTOTYPE_ICONS)) {
      const { attrs } = staticSvgNodes(createElement(Icon, { 'aria-hidden': true }));
      // Rendered with no explicit size or stroke: the registry defaults must be the
      // prototype's own icon() parameters, not lucide's factory defaults.
      expect(attrs, `svg parameters of ${name}`).toEqual(expect.objectContaining(PROTOTYPE_SVG));
    }
  });

  describe.each(Object.keys(FIXTURE))('glyph %s', (name) => {
    it('draws the prototype node list verbatim', () => {
      const Icon = PROTOTYPE_ICONS[name as PrototypeIconName];
      expect(Icon, `registry is missing the prototype glyph ${name}`).toBeDefined();
      const { children } = staticSvgNodes(createElement(Icon, { 'aria-hidden': true }));
      expect(comparableNodes(children), `nodes of ${name}`).toEqual(fixtureNodes(FIXTURE[name]));
    });
  });
});
