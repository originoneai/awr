import { execFileSync } from 'node:child_process';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { describe, expect, it } from 'vitest';
import BASELINE from './test-fixtures/derived-duplication-baseline.json';

const SCRIPT = 'scripts/check-derived-duplication.mjs';
function run(...args: string[]) {
  return execFileSync(process.execPath, [SCRIPT, ...args], { encoding: 'utf8' });
}

describe('derived-duplication source ratchet', () => {
  it('matches the measured public baseline exactly and defines M=0', () => {
    expect(JSON.parse(run('--json'))).toEqual(BASELINE);
    expect(BASELINE.target).toBe(0);
  });

  it('assigns every local derivation pattern to a later adoption slice', () => {
    const manifest = JSON.parse(run('--manifest')) as Array<{ id: string; ranges: Array<{ slice: string }> }>;
    expect(manifest.map((rule) => rule.id)).toEqual(Object.keys(BASELINE.rules));
    for (const rule of manifest) {
      for (const range of rule.ranges) expect(['fold-operations', 'work-state', 'page-adoption']).toContain(range.slice);
    }
  });

  it('counts synthetic semantic ranges without being fooled by nested or indented closures', () => {
    const moduleUrl = pathToFileURL(resolve(SCRIPT)).href;
    const source = [
      'function groupedWorks() {',
      '  const groups = items.map(() => ({',
      '  }));',
      '  return groups;',
      '}',
      'if (ready) {',
      '  return;',
      '}',
    ].join('\n');
    const code = `import { countRange } from ${JSON.stringify(moduleUrl)}; const source = ${JSON.stringify(source)}; console.log(JSON.stringify([countRange(source, { start: 'function groupedWorks', end: '^}' }), countRange(source, { start: 'if (ready)', end: '^}' }), countRange(source, { start: 'missing', end: '^}' })]));`;
    const output = execFileSync(process.execPath, ['--input-type=module', '-e', code], { encoding: 'utf8' });
    expect(JSON.parse(output)).toEqual([5, 3, 0]);
  });
});
