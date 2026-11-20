import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { themeConfig } from './theme';
import { PROTOTYPE_ICONS } from './components/icons/prototype-icons';

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
    expect(shell).toContain('<ChevronRightIcon size={14}');
    expect(shell).toContain('<BellIcon size={18}');
  });

  it('draws the shell from the prototype registry, not from direct icon imports', () => {
    // The shell's glyphs (including home, sliders, activity, bell, search, building,
    // book, moon, which lucide redesigned after the prototype's snapshot) all come
    // from the one registry; its fixture test pins the node data itself.
    const shell = readFileSync('src/components/AppShell.tsx', 'utf8');
    expect(shell).toContain("from './icons/prototype-icons'");
    for (const name of ['HomeIcon', 'FolderIcon', 'InboxIcon', 'BookIcon', 'UsersIcon', 'ActivityIcon', 'SlidersIcon', 'BellIcon', 'SearchIcon', 'BuildingIcon', 'MoonIcon', 'SunIcon', 'ChevronRightIcon', 'ChevronDownIcon']) {
      expect(shell).toMatch(new RegExp(`\\b${name}\\b`));
    }
    for (const protoName of ['home', 'inbox', 'book', 'users', 'activity', 'sliders', 'bell', 'search', 'building', 'moon', 'chev', 'sun', 'folder']) {
      expect(PROTOTYPE_ICONS).toHaveProperty(protoName);
    }
  });
});
