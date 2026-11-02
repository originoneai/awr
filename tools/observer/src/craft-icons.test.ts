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

  it('pins navigation icons to one size and stroke without decorative fill', () => {
    const shell = readFileSync('src/components/AppShell.tsx', 'utf8');
    expect(themeConfig.components?.Menu?.iconSize).toBe(16);
    expect(shell).not.toMatch(/size=\{(14|17|18|20|24)\}/);
    expect(shell).not.toContain('strokeWidth={1.8}');
    expect(shell).not.toContain('fillOpacity');
    expect(shell).toContain('strokeWidth={1.5}');
  });
});
