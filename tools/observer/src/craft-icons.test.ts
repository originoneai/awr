import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { themeConfig } from './theme';

describe('craft icon rules', () => {
  it('uses lucide only, including the package manifest', () => {
    const packageJson = readFileSync('package.json', 'utf8');
    expect(packageJson).not.toContain('@ant-design/icons');
    expect(packageJson).toContain('lucide-react');
    expect(readFileSync('src/components/AppShell.tsx', 'utf8')).not.toContain('@ant-design/icons');
  });

  it('pins shell icons to the prototype size set and one stroke without decorative fill', () => {
    const shell = readFileSync('src/components/AppShell.tsx', 'utf8');
    expect(themeConfig.components?.Menu?.iconSize).toBe(16);
    // The prototype shell uses exactly three icon sizes: the 16px default, 14px chevrons
    // (icon('chev', 14), breadcrumb separators) and 18px for the bell (icon('bell', 18)).
    // Any other size is drift.
    expect(shell).not.toMatch(/size=\{(13|15|17|19|20|22|24)\}/);
    expect(shell).not.toContain('strokeWidth={1.8}');
    expect(shell).not.toContain('fillOpacity');
    expect(shell).toContain('strokeWidth={1.5}');
    expect(shell).toContain('<ChevronRight size={14}');
    expect(shell).toContain('<ShellBellIcon size={18}');
  });

  it('keeps the prototype path data for the glyphs lucide redesigned', () => {
    // panorama.html harvests an older lucide snapshot; upstream later redesigned house,
    // sliders, activity, bell, search, building, book-open and moon. The prototype's own
    // path data is rebuilt through lucide's public createLucideIcon factory.
    const glyphs = readFileSync('src/components/ShellGlyphs.tsx', 'utf8');
    expect(glyphs).toContain("createLucideIcon");
    expect(glyphs).toContain("d: 'M3 10.5 12 3l9 7.5'");
    expect(glyphs).toContain("d: 'M22 12h-4l-3 9L9 3l-3 9H2'");
    expect(glyphs).toContain("d: 'M6 8a6 6 0 0 1 12 0c0 7 3 9 3 9H3s3-2 3-9'");
    expect(glyphs).toContain("d: 'm21 21-4.3-4.3'");
    expect(glyphs).toContain("{ cx: 11, cy: 11, r: 7 }");
    // Glyphs that still match the prototype stay direct lucide imports.
    const shell = readFileSync('src/components/AppShell.tsx', 'utf8');
    for (const name of ['Folder', 'ChevronRight', 'Sun']) {
      expect(shell).toMatch(new RegExp(`\\b${name}\\b`));
    }
  });
});
