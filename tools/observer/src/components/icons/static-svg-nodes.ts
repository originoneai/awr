import type { ReactElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';

/*
 * Test helper: renders one icon element to static markup and reads back the root svg
 * attributes and the child draw nodes (path/circle/…) in document order. Used by the
 * registry fixture test and the display component glyph tests; never shipped, because
 * nothing outside tests imports it.
 */

export interface StaticSvgNode {
  tag: string;
  attrs: Record<string, string>;
}

export interface StaticSvg {
  attrs: Record<string, string>;
  children: StaticSvgNode[];
}

function parseAttrs(source: string): Record<string, string> {
  const attrs: Record<string, string> = {};
  const pattern = /([a-zA-Z_:][-a-zA-Z0-9_:.]*)="([^"]*)"/g;
  let match: RegExpExecArray | null;
  while ((match = pattern.exec(source))) attrs[match[1]] = match[2];
  return attrs;
}

export function staticSvgNodes(element: ReactElement): StaticSvg {
  const markup = renderToStaticMarkup(element);
  const open = /^<svg\b([^>]*)>/.exec(markup);
  if (!open) throw new Error(`not an svg element: ${markup.slice(0, 80)}`);
  const body = markup.slice(open[0].length, markup.lastIndexOf('</svg>'));
  const children: StaticSvgNode[] = [];
  const pattern = /<(path|circle|rect|polyline|line|ellipse)\b([^>]*)>/g;
  let match: RegExpExecArray | null;
  while ((match = pattern.exec(body))) children.push({ tag: match[1], attrs: parseAttrs(match[2]) });
  return { attrs: parseAttrs(open[1]), children };
}

/** One row of the prototype fixture as a comparable node ('c cx cy r' rows are circles). */
export type FixtureNode =
  | { tag: 'path'; d: string }
  | { tag: 'circle'; cx: string; cy: string; r: string };

export function fixtureNodes(rows: readonly string[]): FixtureNode[] {
  return rows.map((row) => {
    if (row.startsWith('c ')) {
      const [, cx, cy, r] = row.split(' ');
      return { tag: 'circle', cx, cy, r } as FixtureNode;
    }
    return { tag: 'path', d: row } as FixtureNode;
  });
}

/** Projects a rendered svg's children down to the fixture's comparable shape. */
export function comparableNodes(children: readonly StaticSvgNode[]): FixtureNode[] {
  return children.map((child) => {
    if (child.tag === 'circle') {
      return { tag: 'circle', cx: child.attrs.cx, cy: child.attrs.cy, r: child.attrs.r };
    }
    return { tag: 'path', d: child.attrs.d };
  });
}
