import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { craftSelectionStyle, craftTokens, craftValueStyle, themeConfig } from './theme';

describe('craft details', () => {
  it('uses the control/card/panel radius ladder and neutral shadows', () => {
    expect(craftTokens.radius).toEqual({ control: 8, card: 12, panel: 16, nested: 4 });
    expect(themeConfig.token?.borderRadius).toBe(8);
    expect(themeConfig.token?.borderRadiusLG).toBe(12);
    expect(themeConfig.components?.Card?.borderRadiusLG).toBe(12);
    expect(themeConfig.token?.boxShadowTertiary).toContain('rgba(17, 24, 39, 0.05)');
  });

  it('uses a selected surface plus hairline and a visible keyboard focus ring', () => {
    const token = { colorFillSecondary: 'raised', colorBorderSecondary: 'hairline', borderRadius: 8 };
    expect(craftSelectionStyle(token, true)).toEqual({
      cursor: 'pointer',
      background: 'raised',
      boxShadow: 'inset 0 0 0 1px hairline',
      borderRadius: 8,
    });
    expect(craftSelectionStyle(token, false)).toEqual({ cursor: 'pointer' });
    const shell = readFileSync('src/global.css', 'utf8');
    expect(shell).toContain('box-shadow: inset 0 0 0 1px var(--ant-color-border-secondary)');
    expect(shell).toContain(':focus-visible');
    expect(shell).toContain('outline: 2px solid var(--ant-color-primary)');
    expect(readFileSync('src/pages/ProjectShell.tsx', 'utf8')).not.toContain('color="blue"');
  });

  it('keeps metadata quiet and tabular', () => {
    expect(craftValueStyle.color).toBe(craftTokens.color.secondary);
    expect(craftValueStyle.fontVariantNumeric).toBe('tabular-nums');
  });
});
