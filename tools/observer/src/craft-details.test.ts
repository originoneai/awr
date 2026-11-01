import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { craftTokens, craftValueStyle, themeConfig } from './theme';

describe('craft details', () => {
  it('uses the control/card/panel radius ladder and neutral shadows', () => {
    expect(craftTokens.radius).toEqual({ control: 8, card: 12, panel: 16, nested: 4 });
    expect(themeConfig.token?.borderRadius).toBe(8);
    expect(themeConfig.token?.borderRadiusLG).toBe(12);
    expect(themeConfig.components?.Card?.borderRadiusLG).toBe(12);
    expect(themeConfig.token?.boxShadowTertiary).toContain('rgba(17, 24, 39, 0.05)');
  });

  it('uses a selected surface plus hairline and a visible keyboard focus ring', () => {
    for (const file of ['src/pages/ProjectAcceptance.tsx', 'src/pages/ProjectKnowledge.tsx']) {
      const source = readFileSync(file, 'utf8');
      expect(source).toContain('1px solid ${token.colorPrimary}');
      expect(source).toContain('token.colorFillSecondary');
    }
    const shell = readFileSync('src/global.css', 'utf8');
    expect(shell).toContain('.shell-nav .ant-menu-item-selected');
    expect(shell).toContain(':focus-visible');
    expect(shell).toContain('outline: 2px solid var(--ant-color-primary)');
  });

  it('keeps metadata quiet and tabular', () => {
    expect(craftValueStyle.color).toBe(craftTokens.color.secondary);
    expect(craftValueStyle.fontVariantNumeric).toBe('tabular-nums');
  });
});
