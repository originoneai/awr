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
    ['borderRadius: 11', 'radius'],
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
      'fontWeight: 500', 'fontWeight: 600', 'font-weight: 400', 'letterSpacing: 0',
      'borderRadius: 8', 'borderRadius: 10', 'border-radius: 10px', 'border-radius: 12px',
      'const next = !current;',
      "typeof binding === 'string' && binding.trim() !== ''",
      't("common.notProvided")',
    ].join('\n');
    expect(scanCraftSource(legal, 'tsx')).toEqual([]);
  });

  it('reads quoted literals before checking their copy punctuation', () => {
    expect(scanCraftSource("const empty = ''; const valid = empty !== 'missing';", 'tsx')).toEqual([]);
    expect(scanCraftSource('const label = "It\'s ready!";', 'tsx').map(violation => violation.rule)).toContain('copy-exclamation');
  });

  it('distinguishes regular-expression punctuation from actual interface copy', () => {
    const source = "const scalar = value.replace(/^[\"']|[\"']$/g, '').replace(/[.!?]+$/, '');";
    expect(scanCraftSource(source, 'tsx')).toEqual([]);
    expect(scanCraftSource(source + ' const label = "Ready!";', 'tsx').map((violation) => violation.rule)).toContain('copy-exclamation');
    expect(scanCraftSource('const label = `Ready ${name}!`;', 'tsx').map((violation) => violation.rule)).toContain('copy-exclamation');
  });

  it('rejects raw freshness keys even in formerly exempt files', () => {
    for (const file of [
      'components/FreshnessBadge.tsx',
      'components/WorkDetailPanel.tsx',
      'pages/ProjectOverview.tsx',
      'pages/WorkbenchPage.tsx',
      'pages/ActivityPage.tsx',
    ]) {
      expect(scanCraftSource('t("freshness.state.not_measured")', 'tsx', file)).toEqual([
        { rule: 'freshness-misuse', match: 'freshness.state.not_measured' },
      ]);
    }
  });

  it.each(['light', 'dark'])('rejects an injected non-token color in a %s themed component', (mode) => {
    const source = `<div data-theme="${mode}" style={{ backgroundColor: 'rebeccapurple', color: 'var(--craft-color-ink)' }} />`;
    expect(scanCraftSource(source, 'tsx', `components/ThemeProbe.tsx`).map((violation) => violation.rule))
      .toContain('named-color');
  });

  it.each(['light', 'dark'])('accepts token references in a %s themed component', (mode) => {
    const source = `<div data-theme="${mode}" style={{ backgroundColor: 'var(--craft-color-surface)', color: 'var(--craft-color-ink)' }} />`;
    expect(scanCraftSource(source, 'tsx', 'components/ThemeProbe.tsx')).toEqual([]);
  });

  it.each(['light', 'dark'])('rejects CSS declarations and color words in %s shorthands', (mode) => {
    const probes: Array<['css' | 'tsx', string]> = [
      ['css', `.x[data-theme="${mode}"] { color: red; }`],
      ['css', `.x[data-theme="${mode}"] { background: red; }`],
      ['css', `.x[data-theme="${mode}"] { border-color: rebeccapurple; }`],
      ['css', `.x[data-theme="${mode}"] { border: 1px solid red; }`],
      ['tsx', `<div data-theme="${mode}" style={{ border: '1px solid red' }} />`],
      ['tsx', `<div data-theme="${mode}" style={{ outline: '2px solid tomato' }} />`],
      ['tsx', `<div data-theme="${mode}" style={{ boxShadow: '0 0 4px rebeccapurple' }} />`],
      ['tsx', `<div data-theme="${mode}" style={{ background: 'center / cover no-repeat red' }} />`],
    ];
    for (const [fileType, source] of probes) {
      expect(scanCraftSource(source, fileType, `components/ThemeProbe.${fileType}`).map((violation) => violation.rule), source)
        .toContain('named-color');
    }
  });

  it('allows token functions and non-color keywords in declarations and shorthands', () => {
    const css = '.x { color: currentColor; background: transparent; border: 1px solid currentColor; outline: 2px solid transparent; box-shadow: none; }'
      + '.y { background: url("red.png") no-repeat; border-color: inherit; }';
    expect(scanCraftSource(css, 'css', 'components/ThemeProbe.css')).toEqual([]);
    const tsx = `<div style={{
      border: '1px solid currentColor', outline: '2px solid transparent',
      background: 'var(--craft-color-surface)', boxShadow: 'inset 0 0 0 1px var(--ant-color-border-secondary)'
    }} />`;
    expect(scanCraftSource(tsx, 'tsx', 'components/ThemeProbe.tsx')).toEqual([]);
  });
});

describe('craft workspace scan', () => {
  it('finds no forbidden pattern outside the explicit token/scanner/test exemptions', () => {
    // Only token definitions/mirrors are exempt; first-paint.test.ts pins the CSS mirror to theme.ts.
    const exempt = new Set(['theme.ts', 'theme-fallback.css', 'i18n/craft.ts', 'lib/craft-guard.ts', 'test-utils.tsx']);
    const files = filesUnder('src').filter((file) => /\.(tsx?|css)$/.test(file)).filter((file) => {
      const relative = file.slice(4);
      // Test files need literal token values; every production page and component remains scanned.
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
