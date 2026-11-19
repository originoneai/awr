import { readdirSync, readFileSync, statSync } from 'node:fs';
import path from 'node:path';
import ts from 'typescript';
import { describe, expect, it } from 'vitest';
import enUS from '../../i18n/en-US';
import zhCN from '../../i18n/zh-CN';
import { CATALOG_COMPONENTS } from './catalog/examples';

const DISPLAY_ROOT = path.resolve(process.cwd(), 'src/components/display');
const SCREENSHOT_ROOT = path.join(DISPLAY_ROOT, '__screenshots__');

function productionFiles(directory: string): string[] {
  return readdirSync(directory).flatMap((entry) => {
    const file = path.join(directory, entry);
    if (statSync(file).isDirectory()) {
      return entry === '__screenshots__' ? [] : productionFiles(file);
    }
    if (!/\.(?:tsx?|css)$/.test(entry)) return [];
    if (entry.includes('.test.') || entry.includes('.spec.') || entry === 'playwright.config.ts') return [];
    return [file];
  });
}

function withoutComments(source: string) {
  return source.replace(/\/\*[\s\S]*?\*\//g, '').replace(/^\s*\/\/.*$/gm, '');
}

/**
 * Allowed: zero, a 1px hairline, percentages, unitless ratios and calculations made from tokens.
 * Forbidden: hex/rgb/hsl colors and literal absolute or font-relative dimensions in any other unit.
 */
function forbiddenLiterals(source: string): string[] {
  const text = withoutComments(source);
  const violations: string[] = [];
  for (const pattern of [/#[\da-f]{3,8}\b/gi, /\b(?:rgba?|hsla?)\s*\(/gi]) {
    for (const match of text.matchAll(pattern)) violations.push(match[0]);
  }
  const dimensions = /(?<![\w.-])-?(?:\d+\.\d+|\d+|\.\d+)(?:px|rem|em|vh|vw|vmin|vmax|cm|mm|in|pt|pc|ex|ch)\b/gi;
  for (const match of text.matchAll(dimensions)) {
    if (match[0] !== '0px' && match[0] !== '1px') violations.push(match[0]);
  }
  return violations;
}

function visibleLiteralViolations(file: string): string[] {
  const source = ts.createSourceFile(file, readFileSync(file, 'utf8'), ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  const violations: string[] = [];
  const visit = (node: ts.Node) => {
    if (ts.isJsxText(node) && node.text.trim()) violations.push(`JSX text: ${node.text.trim()}`);
    if (ts.isJsxAttribute(node) && ts.isIdentifier(node.name)) {
      const name = node.name.text;
      if (['title', 'aria-label', 'placeholder', 'alt'].includes(name)
        && node.initializer && ts.isStringLiteral(node.initializer)) {
        violations.push(`${name}: ${node.initializer.text}`);
      }
    }
    ts.forEachChild(node, visit);
  };
  visit(source);
  return violations;
}

function flattenMessages(value: unknown, prefix = ''): Record<string, string> {
  if (typeof value === 'string') return { [prefix]: value };
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error(`Invalid message at ${prefix}`);
  return Object.entries(value).flatMap(([key, child]) => (
    Object.entries(flattenMessages(child, prefix ? `${prefix}.${key}` : key))
  )).reduce<Record<string, string>>((messages, [key, message]) => {
    messages[key] = message;
    return messages;
  }, {});
}

function forbiddenVocabularyText(text: string) {
  return [
    /\bAWR-[A-Z0-9-]+/i,
    /\bsha(?:256|512):/i,
    /\b[0-9a-f]{12,}\b/i,
    /\b[a-z][a-z0-9]*_[a-z0-9_]+\b/,
    /\b[a-z][a-z0-9]*\.[a-z][a-z0-9]*\.[a-z][a-z0-9]*\b/,
    /^(?:claimed|ready|verified|stalled|not_measured|running)$/i,
  ].some((pattern) => pattern.test(text));
}

describe('design-token and product-language source guards', () => {
  it('rejects raw colors and non-token dimensions in every production component and example', () => {
    const files = productionFiles(DISPLAY_ROOT);
    expect(files.length).toBeGreaterThan(10);
    const violations = files.flatMap((file) => (
      forbiddenLiterals(readFileSync(file, 'utf8')).map((literal) => `${path.relative(DISPLAY_ROOT, file)}: ${literal}`)
    ));
    expect(violations).toEqual([]);
  });

  it('proves the literal guard catches injected color and dimension values but permits hairlines and zero', () => {
    expect(forbiddenLiterals(`
      color: #12abef;
      background: rgb(1 2 3);
      border-color: hsl(20 50% 50%);
      padding: 13px;
      margin: 1rem;
      font-size: 0.8em;
    `)).toEqual(expect.arrayContaining(['#12abef', 'rgb(', 'hsl(', '13px', '1rem', '0.8em']));
    expect(forbiddenLiterals(`
      padding: 0;
      border: 1px solid var(--craft-color-hairline);
      width: 52%;
      stroke-width: calc(var(--ant-line-width-bold) * 0.75);
    `)).toEqual([]);
  });

  it('requires all JSX copy, hover text and accessibility labels to come from the vocabulary', () => {
    const violations = productionFiles(DISPLAY_ROOT)
      .filter((file) => file.endsWith('.tsx'))
      .flatMap((file) => visibleLiteralViolations(file).map((text) => `${path.basename(file)}: ${text}`));
    expect(violations).toEqual([]);
  });

  it('pins a visible keyboard focus ring for openable table rows', () => {
    const styles = readFileSync(path.join(DISPLAY_ROOT, 'display.css'), 'utf8');
    const focusRule = styles.match(/\.obs-data-table tr\[data-openable="true"\]:focus-visible\s*\{[^}]+\}/);
    expect(focusRule?.[0]).toContain('outline: var(--ant-control-outline-width) solid var(--craft-color-accent)');
  });

  it('keeps the components read-only and hides no engineering details in a disclosure', () => {
    const source = productionFiles(DISPLAY_ROOT)
      .filter((file) => file.endsWith('.tsx'))
      .map((file) => readFileSync(file, 'utf8'))
      .join('\n');
    expect(source).not.toMatch(/\bon(?:Dispatch|Claim|Approve|Review|Answer|Merge|Submit|Assign)\s*[=:]/);
    expect(source).not.toMatch(/<(?:details|summary)\b/);
    expect(source).not.toMatch(/\b(?:fetch|axios|mutate|mutateAsync)\s*\(/);
  });
});

describe('bilingual display vocabulary', () => {
  it('has exactly the same keys in Chinese and English', () => {
    expect(Object.keys(flattenMessages(enUS.display)).sort()).toEqual(Object.keys(flattenMessages(zhCN.display)).sort());
  });

  it('contains no internal identifiers, hashes, protocol fields or raw status codes', () => {
    for (const catalog of [zhCN.display, enUS.display]) {
      for (const [key, text] of Object.entries(flattenMessages(catalog))) {
        expect(forbiddenVocabularyText(text), `${key}: ${text}`).toBe(false);
      }
    }
  });

  it('proves missing translations and internal identifiers are detected', () => {
    const brokenEnglish = structuredClone(enUS.display);
    Reflect.deleteProperty(brokenEnglish.empty, 'title');
    expect(Object.keys(flattenMessages(brokenEnglish)).sort()).not.toEqual(Object.keys(flattenMessages(zhCN.display)).sort());
    expect(forbiddenVocabularyText('AWR-PPO-004')).toBe(true);
    expect(forbiddenVocabularyText('created_at_unix_ms')).toBe(true);
  });
});

describe('development catalog coverage', () => {
  it('has a desktop light, desktop dark and phone light baseline for every component', () => {
    for (const component of CATALOG_COMPONENTS) {
      for (const project of ['desktop-light', 'desktop-dark', 'phone-light']) {
        const screenshot = path.join(SCREENSHOT_ROOT, project, `${component}.png`);
        const bytes = readFileSync(screenshot);
        expect(bytes.length, `${project}/${component}`).toBeGreaterThan(1000);
        expect(bytes.subarray(1, 4).toString('ascii')).toBe('PNG');
      }
    }
  });
});
