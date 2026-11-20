import { readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';
import { findForbiddenIconImports } from './forbidden-icon-imports';

/*
 * The guard of AWR-PPO-069: icons come from the registry
 * (src/components/icons/prototype-icons.tsx) and nowhere else. Any import of
 * lucide-react or @ant-design/icons — value, type-only, dynamic or require — in a
 * non-test source file outside the registry fails here.
 */

const REGISTRY = join('src', 'components', 'icons', 'prototype-icons.tsx');

function* sourceFiles(dir: string): Generator<string> {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) yield* sourceFiles(path);
    else if (/\.(ts|tsx)$/.test(entry.name) && !/\.(test|spec)\./.test(entry.name)) yield path;
  }
}

describe('icon import guard', () => {
  it('flags a direct icon import (counterexample)', () => {
    const source = `import { Search } from 'lucide-react';\nexport const x = 1;\n`;
    const hits = findForbiddenIconImports(source, 'demo.tsx');
    expect(hits).toHaveLength(1);
    expect(hits[0].line).toBe(1);
    expect(hits[0].excerpt).toContain('lucide-react');
  });

  it('flags type-only, namespace, side-effect, dynamic, require, sub-path and template-literal forms (counterexamples)', () => {
    expect(findForbiddenIconImports(`import type { LucideIcon } from "lucide-react";`, 'a.ts')).toHaveLength(1);
    expect(findForbiddenIconImports(`import * as icons from 'lucide-react'`, 'b.ts')).toHaveLength(1);
    expect(findForbiddenIconImports(`import 'lucide-react';`, 'c.ts')).toHaveLength(1);
    expect(findForbiddenIconImports(`const m = await import('lucide-react');`, 'd.ts')).toHaveLength(1);
    expect(findForbiddenIconImports(`const m = require('lucide-react');`, 'e.ts')).toHaveLength(1);
    expect(findForbiddenIconImports(`export { X } from 'lucide-react/dist/esm/icons/house.mjs';`, 'f.ts')).toHaveLength(1);
    expect(findForbiddenIconImports(`import { HomeOutlined } from '@ant-design/icons';`, 'g.tsx')).toHaveLength(1);
    expect(findForbiddenIconImports('const m = await import(`lucide-react`);', 'h.ts')).toHaveLength(1);
    expect(findForbiddenIconImports('const m = import(`lucide-react/${name}`);', 'i.ts')).toHaveLength(1);
    expect(findForbiddenIconImports('require(`@ant-design/icons`);', 'j.ts')).toHaveLength(1);
  });

  it('passes registry imports and plain mentions (counterexamples)', () => {
    expect(findForbiddenIconImports(`import { SearchIcon } from './icons/prototype-icons';`, 'k.tsx')).toHaveLength(0);
    expect(findForbiddenIconImports(`const note = "lucide-react shapes stay in the registry";`, 'l.ts')).toHaveLength(0);
    expect(findForbiddenIconImports(`const pattern = /from\\s*['"]x['"]/;`, 'm.ts')).toHaveLength(0);
    expect(findForbiddenIconImports('const label = `icons come from lucide-react via the registry`;', 'n.ts')).toHaveLength(0);
  });

  it('reports nothing outside the registry and nothing missing inside it', () => {
    const violations = [];
    for (const file of sourceFiles('src')) {
      if (file === REGISTRY) continue;
      violations.push(...findForbiddenIconImports(readFileSync(file, 'utf8'), file));
    }
    expect(violations).toEqual([]);
    // The registry is the single entry: it keeps exactly one icon-package import.
    expect(findForbiddenIconImports(readFileSync(REGISTRY, 'utf8'), REGISTRY)).toHaveLength(1);
  });
});
