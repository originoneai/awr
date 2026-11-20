import { createElement } from 'react';
import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import {
  ArrowLeftIcon,
  ChevronDownIcon,
  PROTOTYPE_ICONS,
  RefreshCwIcon,
  StampIcon,
  TriangleAlertIcon,
  type PrototypeIconName,
} from './prototype-icons';
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

/*
 * The app-only glyphs: the registry's second section. Their acceptance standard is the
 * installed lucide-react 1.66.0 (there is no prototype synonym), pinned by this fixture
 * with the same verbatim rule and the same comparison as the prototype table above.
 * Generated from node_modules/lucide-react/dist/esm/icons/<name>.mjs (__iconData.node).
 */
const APP_ONLY_FIXTURE = JSON.parse(readFileSync('src/test-fixtures/app-only-icons.json', 'utf8')) as Record<string, string[]>;

const APP_ONLY_COMPONENTS = {
  'arrow-left': ArrowLeftIcon,
  'chevron-down': ChevronDownIcon,
  stamp: StampIcon,
  'refresh-cw': RefreshCwIcon,
  'triangle-alert': TriangleAlertIcon,
} as const;

describe('the app-only glyphs of the registry', () => {
  it('is exactly the set of names without a prototype synonym', () => {
    expect(Object.keys(APP_ONLY_COMPONENTS).sort()).toEqual(Object.keys(APP_ONLY_FIXTURE).sort());
  });

  it('draws the installed lucide node list verbatim', () => {
    for (const [name, Icon] of Object.entries(APP_ONLY_COMPONENTS)) {
      const { children } = staticSvgNodes(createElement(Icon, { 'aria-hidden': true }));
      expect(comparableNodes(children), `nodes of ${name}`).toEqual(fixtureNodes(APP_ONLY_FIXTURE[name]));
    }
  });

  it('leaves no registry glyph outside PROTOTYPE_ICONS and the app-only set', () => {
    // Every glyph declaration in the registry must belong to one of the two sets:
    // a page icon that renders a prototype glyph, or a named app-only glyph.
    const source = readFileSync('src/components/icons/prototype-icons.tsx', 'utf8');
    const declared = [...source.matchAll(/export const \w+ = glyph\('([\w-]+)'/g)].map((match) => match[1]);
    const rendered = new Set(Object.values(PROTOTYPE_ICONS).map((Icon) => {
      const classes = staticSvgNodes(createElement(Icon, { size: 1 })).attrs.class ?? '';
      return classes.replace(/^lucide lucide-/, '');
    }));
    expect(declared.sort()).toEqual([...new Set([...rendered, ...Object.keys(APP_ONLY_COMPONENTS)])].sort());
  });
});

