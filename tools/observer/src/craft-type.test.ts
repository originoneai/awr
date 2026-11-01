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

  it('uses tabular metadata and bans heavy weights, uppercase tracking and negative small-text tracking', () => {
    expect(craftValueStyle.fontVariantNumeric).toBe('tabular-nums');
    expect(craftValueStyle.letterSpacing).toBe(0);
    const files = [...filesUnder('src/pages'), ...filesUnder('src/components')].filter((file) => file.endsWith('.tsx'));
    const offenders = files.filter((file) => {
      const source = readFileSync(file, 'utf8');
      return /fontWeight:\s*[6-9]/.test(source) || /letterSpacing:\s*-/.test(source) || /textTransform:\s*'uppercase'/.test(source);
    });
    expect(offenders).toEqual([]);
  });
});
