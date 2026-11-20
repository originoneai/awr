import { describe, expect, it } from 'vitest';
import { craftTokens, themeConfig } from './theme';

describe('craft spacing tokens', () => {
  it('uses the approved app shell rhythm', () => {
    expect(themeConfig.components?.Menu?.itemHeight).toBe(craftTokens.navigationRowHeight);
    expect(craftTokens.navigationRowHeight).toBe(36);
    expect(craftTokens.controlHeight).toBe(36);
    expect(themeConfig.token?.controlHeight).toBe(craftTokens.controlHeight);
    expect(craftTokens.headerHeight).toBeGreaterThanOrEqual(56);
    expect(craftTokens.headerHeight).toBeLessThanOrEqual(64);
    // Prototype .app grid column / .side: exactly 232px (the owner's pixel-parity rule
    // overrides the earlier 240-280 unslop band).
    expect(craftTokens.siderWidth).toBe(232);
    expect(craftTokens.contentMaxWidth).toBeGreaterThanOrEqual(1440);
    expect(craftTokens.contentMaxWidth).toBeLessThanOrEqual(1600);
  });

  it('keeps every spacing token on the 4px grid', () => {
    for (const value of Object.values(craftTokens.spacing)) expect(value % 4).toBe(0);
  });
});
