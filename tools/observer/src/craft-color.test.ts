import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';
import { craftTokens } from './theme';

function filesUnder(path: string): string[] {
  return readdirSync(path).flatMap((name) => {
    const file = join(path, name);
    return statSync(file).isDirectory() ? filesUnder(file) : [file];
  });
}

function channel(hex: string, index: number): number {
  return Number.parseInt(hex.slice(index, index + 2), 16) / 255;
}
function luminance(hex: string): number {
  const [r, g, b] = [1, 3, 5].map((index) => channel(hex, index)).map((value) => (
    value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4
  ));
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}
function contrast(left: string, right: string): number {
  const [high, low] = [luminance(left), luminance(right)].sort((a, b) => b - a);
  return (high + 0.05) / (low + 0.05);
}

describe('craft color tokens', () => {
  it('keeps page and component source free of literal hex colors', () => {
    const files = [
      ...filesUnder('src/pages'),
      ...filesUnder('src/components'),
      'src/global.css',
    ].filter((file) => /\.(tsx?|css)$/.test(file));
    const offenders = files.filter((file) => /#[0-9A-Fa-f]{3,8}\b/.test(readFileSync(file, 'utf8')));
    expect(offenders).toEqual([]);
  });

  const roles = ['ink', 'secondary', 'muted', 'accent', 'positive', 'negative', 'warning'] as const;
  const surfaces = ['page', 'surface', 'raised'] as const;
  it.each(roles.flatMap((role) => surfaces.map((surface) => ({ role, surface }))))(
    'keeps $role text accessible on $surface (review r1.2)',
    ({ role, surface }) => {
      expect(contrast(craftTokens.color[role], craftTokens.color[surface])).toBeGreaterThanOrEqual(4.5);
    },
  );
});
