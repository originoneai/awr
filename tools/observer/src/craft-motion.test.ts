import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';
import { craftTokens, themeConfig } from './theme';

function filesUnder(path: string): string[] {
  return readdirSync(path).flatMap((name) => {
    const file = join(path, name);
    return statSync(file).isDirectory() ? filesUnder(file) : [file];
  });
}

describe('craft motion', () => {
  it('uses one easing curve and three bounded durations', () => {
    expect(craftTokens.motion).toEqual({ easing: 'cubic-bezier(0.22, 1, 0.36, 1)', feedbackMs: 160, overlayMs: 200, enterMs: 400 });
    expect(themeConfig.token?.motionEaseOut).toBe(craftTokens.motion.easing);
    expect(themeConfig.token?.motionDurationFast).toBe('160ms');
    expect(themeConfig.token?.motionDurationMid).toBe('200ms');
    expect(themeConfig.token?.motionDurationSlow).toBe('400ms');
  });

  it('has no catch-all transition, card scaling or unjustified animation loops', () => {
    const files = [
      ...filesUnder('src/pages'),
      ...filesUnder('src/components'),
      'src/global.css',
    ].filter((file) => /\.(tsx?|css)$/.test(file));
    const offenders = files.filter((file) => {
      const source = readFileSync(file, 'utf8');
      return /transition-all|@keyframes|animation:|animate-pulse|animate-ping|hover:scale|transform:\s*scale/.test(source);
    });
    expect(offenders).toEqual([]);
    expect(readFileSync('src/global.css', 'utf8')).toContain('color 160ms cubic-bezier(0.22, 1, 0.36, 1)');
  });
});
