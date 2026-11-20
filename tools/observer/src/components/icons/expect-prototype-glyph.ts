import { createElement } from 'react';
import { expect } from 'vitest';
import { PROTOTYPE_ICONS, type PrototypeIconName } from './prototype-icons';
import { comparableNodes, staticSvgNodes, type FixtureNode } from './static-svg-nodes';

/*
 * Test helper: assert that an svg element rendered anywhere in the app draws exactly a
 * named prototype glyph. The expectation is always PROTOTYPE_ICONS[name]'s own draw
 * nodes — never a component's own import — so a glyph swapped in a component turns the
 * assertion red. Only test files import this; the app bundle never sees it.
 */

const glyphCache = new Map<PrototypeIconName, FixtureNode[]>();

export function prototypeGlyphNodes(name: PrototypeIconName): FixtureNode[] {
  let nodes = glyphCache.get(name);
  if (!nodes) {
    nodes = comparableNodes(staticSvgNodes(createElement(PROTOTYPE_ICONS[name], { size: 1 })).children);
    glyphCache.set(name, nodes);
  }
  return nodes;
}

/** The svg's child draw nodes as comparable data (path d / circle cx cy r, in order). */
export function svgChildNodes(svg: Element | null | undefined): FixtureNode[] {
  expect(svg, 'expected an svg in this slot').toBeTruthy();
  return [...(svg as SVGSVGElement).children].map((child) => {
    if (child.tagName.toLowerCase() === 'circle') {
      return { tag: 'circle', cx: child.getAttribute('cx') ?? '', cy: child.getAttribute('cy') ?? '', r: child.getAttribute('r') ?? '' };
    }
    return { tag: 'path', d: child.getAttribute('d') ?? '' };
  });
}

/** Asserts that `svg` (found by the caller at a named UI slot) draws the prototype `name` glyph. */
export function expectSvgDrawsPrototypeGlyph(svg: Element | null | undefined, slot: string, name: PrototypeIconName) {
  expect(svg, `${slot}: expected an svg`).toBeTruthy();
  expect(svgChildNodes(svg), `${slot}: must draw the prototype ${name} glyph`).toEqual(prototypeGlyphNodes(name));
}
