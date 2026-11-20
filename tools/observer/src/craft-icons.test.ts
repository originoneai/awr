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
    expect(shell).toContain('<Bell size={18}');
  });
});
