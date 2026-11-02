import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';
import { scanCoreSource } from './core/dependency-guard';
import * as coreContract from './core/observation/contract';
import * as existingContract from './lib/contract';

function filesUnder(path: string): string[] {
  return readdirSync(path).flatMap((name) => {
    const file = join(path, name);
    return statSync(file).isDirectory() ? filesUnder(file) : [file];
  });
}

describe('observation core independence', () => {
  it('contains no UI, page, query, API, fetch, DOM or implicit-clock dependency', () => {
    const violations = filesUnder('src/core/observation')
      .filter((file) => /\.(?:ts|tsx)$/.test(file))
      .flatMap((file) => scanCoreSource(readFileSync(file, 'utf8')).map((violation) => ({ file, ...violation })));
    expect(violations).toEqual([]);
  });

  it('detects each forbidden dependency in synthetic source', () => {
    expect(scanCoreSource("import React from 'react';")[0]?.rule).toBe('ui-import');
    expect(scanCoreSource("export { x } from 'antd';")[0]?.rule).toBe('ui-import');
    expect(scanCoreSource("import { x } from '../../pages/ActivityPage';")[0]?.rule).toBe('page-or-read-import');
    expect(scanCoreSource("import { x } from '../../lib/api';")[0]?.rule).toBe('page-or-read-import');
    expect(scanCoreSource('fetch("/api");')[0]?.rule).toBe('fetch');
    expect(scanCoreSource('document.body.textContent;')[0]?.rule).toBe('dom');
    expect(scanCoreSource('Date.now();')[0]?.rule).toBe('implicit-clock');
    expect(scanCoreSource("import { humanText } from '../../i18n/human-language';\nimport { PLANE_STATES } from '../../lib/contract';")).toEqual([]);
  });

  it('reuses the existing observation-contract states and thresholds verbatim', () => {
    expect(coreContract.PLANE_STATES).toBe(existingContract.PLANE_STATES);
    expect(coreContract.OBSERVATION_STALE_AFTER_MS).toBe(existingContract.OBSERVATION_STALE_AFTER_MS);
    expect(coreContract.READ_TIMEOUT_MS).toBe(existingContract.READ_TIMEOUT_MS);
    expect(coreContract.OVERVIEW_DEADLINE_MS).toBe(existingContract.OVERVIEW_DEADLINE_MS);
  });
});
