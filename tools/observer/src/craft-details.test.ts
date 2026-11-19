import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { craftSelectionStyle, craftTokens, craftValueStyle, themeConfig } from './theme';

describe('craft details', () => {
  it('uses the control/card/panel radius ladder and neutral shadows', () => {
    // radius.tag (6) is the prototype's status-tag corner, approved as a named token in
    // the AWR-PPO-004 round-2 review; it joins the ladder beside the shared four.
    expect(craftTokens.radius).toEqual({ control: 8, card: 12, panel: 16, nested: 4, tag: 6 });
    expect(themeConfig.token?.borderRadius).toBe(8);
    expect(themeConfig.token?.borderRadiusLG).toBe(12);
    expect(themeConfig.components?.Card?.borderRadiusLG).toBe(12);
    expect(themeConfig.token?.boxShadowTertiary).toContain('rgba(17, 24, 39, 0.05)');
  });

  it('uses accent-soft shell selection without a border and a visible keyboard focus ring', () => {
    const token = { colorFillSecondary: 'raised', colorBorderSecondary: 'hairline', borderRadius: 8 };
    expect(craftSelectionStyle(token, true)).toEqual({
      cursor: 'pointer',
      background: 'raised',
      boxShadow: 'inset 0 0 0 1px hairline',
      borderRadius: 8,
    });
    expect(craftSelectionStyle(token, false)).toEqual({ cursor: 'pointer' });
    const shell = readFileSync('src/global.css', 'utf8');
    expect(shell).toContain('background: var(--craft-color-accent-soft)');
    expect(shell).toContain('color: var(--craft-color-accent)');
    expect(shell).toContain('font-weight: 500');
    expect(shell).toContain('box-shadow: none');
    expect(themeConfig.components?.Menu?.itemSelectedBg).toBe(craftTokens.color.accentSoft);
    expect(themeConfig.components?.Menu?.itemSelectedColor).toBe(craftTokens.color.accent);
    expect(themeConfig.components?.Menu?.itemHeight).toBe(36);
    expect(themeConfig.components?.Menu?.itemBorderRadius).toBe(8);
    expect(themeConfig.components?.Menu?.itemPaddingInline).toBe(12);
    expect(themeConfig.components?.Menu?.iconMarginInlineEnd).toBe(10);
    expect(themeConfig.components?.Menu?.itemMarginInline).toBe(0);
    expect(themeConfig.components?.Menu?.itemMarginBlock).toBe(0);
    expect(shell).toContain(':focus-visible');
    expect(shell).toContain('outline: 2px solid var(--ant-color-primary)');
    expect(readFileSync('src/pages/ProjectShell.tsx', 'utf8')).not.toContain('color="blue"');
  });

  it('keeps metadata quiet and tabular', () => {
    expect(craftValueStyle.color).toBe('var(--craft-color-secondary)');
    expect(craftValueStyle.fontVariantNumeric).toBe('tabular-nums');
  });
});
