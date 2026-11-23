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
    .filter((file) => /\.(?:ts|tsx)$/.test(file) && !/\.test\.[jt]sx?$/.test(file))
    .map((file) => file.replaceAll('\\', '/'));
}

/**
 * Every way a source file reads the browser clock itself: `Date.now` (called or passed on),
 * the argument-less `new Date()` and `performance.now`. A date built from a given moment
 * (`new Date(unixMs)`, `Date.parse`, `Date.UTC`) formats a fact and is fine.
 */
const CLOCK_READ = /\bDate\s*\.\s*now\b|\bnew\s+Date\s*\(\s*\)|\bperformance\s*\.\s*now\b/;

function readsTheClock(source: string): boolean {
  return CLOCK_READ.test(source);
}

describe('pages read the clock only through the shared display clock', () => {
  it('scans the whole page surface, not an empty or partial set', () => {
    // A scan that quietly covers nothing would pass for ever, so pin what it must reach.
    const files = surfaceFiles();
    expect(files.length).toBeGreaterThan(20);
    expect(files).toEqual(expect.arrayContaining([
      'src/pages/ActivityPage.tsx',
      'src/pages/InboxPage.tsx',
      'src/pages/ProjectsPage.tsx',
      'src/pages/WorkbenchPage.tsx',
      'src/components/AppShell.tsx',
      'src/components/RecentChanges.tsx',
      'src/components/workbench/WorkbenchFrame.tsx',
    ]));
    expect(files.some((file) => file.endsWith('.test.tsx') || file.endsWith('.test.ts'))).toBe(false);
  });

  it('contains no read of the browser clock outside src/lib/display-clock.tsx', () => {
    // AWR-PPO-079: displayed ages and calendar labels come from the read's own clock
    // walked forward (readClockOffsetUnixMs/referenceNowUnixMs) and from the deployment's
    // pinned display zone (useDisplayTimeZone). A page reading the browser clock directly
    // would silently go back to it, so every form of that read is forbidden here.
    const violations = surfaceFiles().filter((file) => readsTheClock(readFileSync(file, 'utf8')));
    expect(violations).toEqual([]);
  });

  it.each([
    'const nowUnixMs = Date.now();',
    'const clock = Date.now;',
    'const nowUnixMs = Date . now ();',
    'const today = new Date();',
    'const today = new Date( );',
    'const ms = new Date().getTime();',
    'const started = performance.now();',
  ])('flags a page that reads the clock itself: %s', (line) => {
    expect(readsTheClock(['export function Page() {', `  ${line}`, '  return null;', '}'].join('\n'))).toBe(true);
  });

  it.each([
    'const parsed = Date.parse("2026-11-19");',
    'const moment = new Date(unixMs);',
    'const epoch = new Date(0);',
    'const utc = Date.UTC(2026, 10, 19);',
    'const nowUnixMs = referenceNowUnixMs(offset);',
    'const formatter = new Intl.DateTimeFormat("en-US");',
  ])('leaves a given moment alone: %s', (line) => {
    expect(readsTheClock(line)).toBe(false);
  });
});
