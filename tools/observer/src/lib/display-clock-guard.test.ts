import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';

function filesUnder(path: string): string[] {
  return readdirSync(path).flatMap((name) => {
    const file = join(path, name);
    return statSync(file).isDirectory() ? filesUnder(file) : [file];
  });
}

/** The page surface: every page and rendering component, tests excluded. */
function surfaceFiles(): string[] {
  return [...filesUnder('src/pages'), ...filesUnder('src/components')]
    .filter((file) => /\.(?:ts|tsx)$/.test(file) && !/\.test\.[jt]sx?$/.test(file));
}

describe('pages read the clock only through the shared display clock', () => {
  it('contains no Date.now() call outside src/lib/display-clock.tsx', () => {
    // AWR-PPO-079: displayed ages and calendar labels come from the read's own clock
    // walked forward (readClockOffsetUnixMs/referenceNowUnixMs) and from the deployment's
    // pinned display zone (useDisplayTimeZone). A page calling Date.now() directly would
    // silently go back to the browser clock, so the call itself is forbidden here.
    const violations = surfaceFiles()
      .map((file) => ({ file, source: readFileSync(file, 'utf8') }))
      .filter(({ source }) => source.includes('Date.now('))
      .map(({ file }) => file);
    expect(violations).toEqual([]);
  });

  it('would flag a synthetic page that reads the clock itself', () => {
    // The scan is a source scan: prove the pattern it catches is exactly the one a page
    // would write, so the guard fails when someone re-adds it.
    const synthetic = [
      'export function Page() {',
      '  const nowUnixMs = Date.now();',
      '  return nowUnixMs;',
      '}',
    ].join('\n');
    expect(synthetic.includes('Date.now(')).toBe(true);
    expect('const now = Date.parse("2026-11-19");'.includes('Date.now(')).toBe(false);
  });
});
