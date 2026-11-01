import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';
import { scanCraftSource } from './lib/craft-guard';

function filesUnder(path: string): string[] {
  return readdirSync(path).flatMap((name) => {
    const file = join(path, name);
    return statSync(file).isDirectory() ? filesUnder(file) : [file];
  });
}

describe('craft source scanner self-test', () => {
  it.each([
    ['#123ABC', 'hex-color'],
    ['rgb(1, 2, 3)', 'color-function'],
    ['hsla(120, 50%, 50%, 0.5)', 'color-function'],
    ['linear-gradient(to right, red, blue)', 'gradient'],
    ['box-shadow: 0 0 20px rgba(139, 92, 246, 0.5)', 'colored-shadow'],
    ['drop-shadow-[0_0_20px_rgba(139,92,246,0.5)]', 'colored-shadow'],
    ['<Tag color="blue">', 'preset-color'],
    ["style={{ color: 'purple' }}", 'preset-color'],
    ['import { Bell } from "@ant-design/icons"', 'icon-package'],
    ['transition: all 160ms ease', 'transition-all'],
    ["transition: 'all 160ms ease'", 'transition-all'],
    ['font-weight: 700', 'font-weight'],
    ["fontWeight: 'bold'", 'font-weight'],
    ['textTransform: "uppercase"', 'uppercase'],
    ["textTransform: 'uppercase'", 'uppercase'],
    ['text-transform: uppercase', 'uppercase'],
    ['letter-spacing: -0.02em', 'negative-letter-spacing'],
    ['letterSpacing: -0.01', 'negative-letter-spacing'],
    ['🚀', 'emoji'],
    ['"Ready!"', 'copy-exclamation'],
    ['borderRadius: 10', 'radius'],
    ['border-radius: 20px', 'radius'],
    ['t("freshness.state.not_measured")', 'freshness-misuse'],
  ])('detects %s as %s', (source, rule) => {
    expect(scanCraftSource(source, 'tsx').map((violation) => violation.rule)).toContain(rule);
  });

  it('accepts token-based, semantic and non-copy patterns without false positives', () => {
    const legal = [
      '<Tag color="success">', '<Tag color="error">', '<Tag color="warning">', '<Tag color="processing">', '<Tag color="default">',
      'box-shadow: inset 0 0 0 1px var(--ant-color-border-secondary)',
      'transition: color 160ms cubic-bezier(0.22, 1, 0.36, 1)',
      'fontWeight: 500', 'font-weight: 400', 'letterSpacing: 0', 'borderRadius: 8', 'border-radius: 12px',
      'const next = !current;',
      't("common.notProvided")',
    ].join('\n');
    expect(scanCraftSource(legal, 'tsx')).toEqual([]);
    expect(scanCraftSource('t("freshness.state.not_measured")', 'tsx', 'components/FreshnessBadge.tsx')).toEqual([]);
  });
});

describe('craft workspace scan', () => {
  it('finds no forbidden pattern outside the explicit token/scanner/test exemptions', () => {
    const exempt = new Set(['theme.ts', 'i18n/craft.ts', 'lib/craft-guard.ts', 'test-utils.tsx']);
    const files = filesUnder('src').filter((file) => /\.(tsx?|css)$/.test(file)).filter((file) => {
      const relative = file.slice(4);
      return !exempt.has(relative) && !/\.test\.[cm]?[jt]sx?$/.test(relative);
    });
    const violations = files.flatMap((file) => {
      const relative = file.slice(4);
      const type = file.endsWith('.css') ? 'css' : 'tsx';
      return scanCraftSource(readFileSync(file, 'utf8'), type, relative).map((violation) => ({ file: relative, ...violation }));
    });
    expect(violations).toEqual([]);
  });
});
