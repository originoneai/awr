import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';
import { craftTokens, craftValueStyle, themeConfig } from './theme';

function filesUnder(path: string): string[] {
  return readdirSync(path).flatMap((name) => {
    const file = join(path, name);
    return statSync(file).isDirectory() ? filesUnder(file) : [file];
  });
}

describe('craft typography', () => {
  it('uses the system interface family with Chinese fallbacks and two weights', () => {
    expect(craftTokens.font.interface).toContain('PingFang SC');
    expect(craftTokens.font.interface).toContain('Noto Sans SC');
    expect(themeConfig.token?.fontFamily).toBe(craftTokens.font.interface);
    expect(themeConfig.token?.fontWeightStrong).toBe(500);
    expect(craftValueStyle.fontWeight).toBe(400);
  });

  it('sets the prototype body type on the body itself: the interface stack at 400 14px/20px, antialiased', () => {
    // The prototype prints body { font: 400 14px/20px var(--font); -webkit-font-smoothing: antialiased }.
    // global.css mirrors it verbatim so no element outside the antd root falls back to the UA font.
    const css = readFileSync('src/global.css', 'utf8');
    expect(css).toContain(`font-family: ${craftTokens.font.interface};`);
    expect(css).toContain('font-size: 14px;\n  line-height: 20px;\n  font-weight: 400;');
    expect(css).toContain('-webkit-font-smoothing: antialiased;');
    expect(css).toContain('-moz-osx-font-smoothing: grayscale;');
  });

  it('uses tabular metadata and bans 700+ weights, uppercase tracking and negative small-text tracking', () => {
    expect(craftValueStyle.fontVariantNumeric).toBe('tabular-nums');
    expect(craftValueStyle.letterSpacing).toBe(0);
    const files = [...filesUnder('src/pages'), ...filesUnder('src/components')].filter((file) => file.endsWith('.tsx'));
    const offenders = files.filter((file) => {
      const source = readFileSync(file, 'utf8');
      return /fontWeight:\s*[7-9]/.test(source) || /letterSpacing:\s*-/.test(source) || /textTransform:\s*'uppercase'/.test(source);
    });
    expect(offenders).toEqual([]);
  });
});
