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
    // The whole core except the guard itself, whose own patterns would trip it.
    const violations = filesUnder('src/core')
      .filter((file) => /\.(?:ts|tsx)$/.test(file) && !file.endsWith('dependency-guard.ts'))
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

  it('detects every module family the core must not reach, in each import spelling', () => {
    const rule = (source: string) => scanCoreSource(source).map((item) => item.rule);
    expect(rule("import { createRoot } from 'react-dom/client';")).toEqual(['ui-import']);
    expect(rule("import { useTranslation } from 'react-i18next';")).toEqual(['ui-import']);
    expect(rule("import { useQuery } from '@tanstack/react-query';")).toEqual(['ui-import']);
    expect(rule("import { q } from '../../lib/queries';")).toEqual(['page-or-read-import']);
    expect(rule("import 'react';")).toEqual(['ui-import']);
    expect(rule("export * from 'antd';")).toEqual(['ui-import']);
    expect(rule("import {\n  a,\n  b,\n} from 'antd';")).toEqual(['ui-import']);
    expect(rule("const lazy = await import('antd');")).toEqual(['ui-import']);
    expect(rule("const lazy = require('react');")).toEqual(['ui-import']);
    expect(rule("const page = await import('../../pages/ActivityPage');")).toEqual(['page-or-read-import']);
    expect(rule("const same = await import('./values');\nexport { x } from './values';")).toEqual([]);
  });

  it('detects the browser, network, clock and timer reaches a pure core must not make', () => {
    const rule = (source: string) => scanCoreSource(source).map((item) => item.rule);
    expect(rule('window.location.href;')).toEqual(['dom']);
    expect(rule('navigator.userAgent;')).toEqual(['dom']);
    expect(rule("localStorage.getItem('a');")).toEqual(['browser-storage']);
    expect(rule("sessionStorage.setItem('a', 'b');")).toEqual(['browser-storage']);
    expect(rule('const request = new XMLHttpRequest();')).toEqual(['fetch']);
    expect(rule('const now = new Date();')).toEqual(['implicit-clock']);
    expect(rule('const now = performance.now();')).toEqual(['implicit-clock']);
    expect(rule('setTimeout(() => 1, 5);')).toEqual(['timer']);
    expect(rule('setInterval(() => 1, 5);')).toEqual(['timer']);
    expect(rule('requestAnimationFrame(() => 1);')).toEqual(['timer']);
    // A date built from an explicit value is not a clock read.
    expect(rule('const day = new Date(1700000000000);')).toEqual([]);
  });

  it('reuses the existing observation-contract states and thresholds verbatim', () => {
    expect(coreContract.PLANE_STATES).toBe(existingContract.PLANE_STATES);
    expect(coreContract.OBSERVATION_STALE_AFTER_MS).toBe(existingContract.OBSERVATION_STALE_AFTER_MS);
    expect(coreContract.READ_TIMEOUT_MS).toBe(existingContract.READ_TIMEOUT_MS);
    expect(coreContract.OVERVIEW_DEADLINE_MS).toBe(existingContract.OVERVIEW_DEADLINE_MS);
  });
});
