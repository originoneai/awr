import * as ts from 'typescript';

export type CraftFileType = 'tsx' | 'css';

export interface CraftViolation {
  rule: string;
  match: string;
}

const ALLOWED_ANTD_COLORS = new Set(['success', 'error', 'warning', 'processing', 'default', 'currentColor', 'inherit']);
const ALLOWED_RADII = new Set([4, 8, 12, 16]);

function copyLiterals(source: string, fileType: CraftFileType, filePath: string): Array<{ text: string; source: string }> {
  if (fileType === 'css') return [...source.matchAll(/(['"`])(?:\\.|(?!\1)[^\\\r\n])*\1/g)]
    .map((match) => ({ text: match[0], source: match[0] }));
  const kind = filePath.endsWith('.ts') ? ts.ScriptKind.TS : ts.ScriptKind.TSX;
  const file = ts.createSourceFile(filePath || 'interface.tsx', source, ts.ScriptTarget.Latest, true, kind);
  const literals: Array<{ text: string; source: string }> = [];
  function visit(node: ts.Node) {
    const text = ts.isStringLiteralLike(node) ? node.text : ts.isTemplateExpression(node)
      ? [node.head.text, ...node.templateSpans.map((span) => span.literal.text)].join('') : null;
    if (text !== null) literals.push({ text, source: node.getText(file) });
    ts.forEachChild(node, visit);
  }
  visit(file);
  return literals;
}

/** Pure source scanner used by both the self-test and the full workspace guard. */
export function scanCraftSource(source: string, fileType: CraftFileType, filePath = ''): CraftViolation[] {
  const violations: CraftViolation[] = [];
  const add = (rule: string, pattern: RegExp) => {
    for (const match of source.matchAll(pattern)) violations.push({ rule, match: match[0] });
  };

  add('hex-color', /#[0-9a-f]{3,8}\b/gi);
  add('color-function', /\b(?:rgba?|hsla?|oklch|oklab|lch|lab|color|color-contrast)\s*\(/gi);
  for (const match of source.matchAll(/\b(?:backgroundColor|background-color|background|border(?:Top|Right|Bottom|Left)?Color|border(?:-top|-right|-bottom|-left)?-color|outlineColor|outline-color|textDecorationColor|text-decoration-color|columnRuleColor|column-rule-color|caretColor|caret-color|fill|stroke)\s*:\s*['"]([A-Za-z]+)['"]/g)) {
    if (!['currentColor', 'transparent', 'none', 'inherit', 'initial', 'unset', 'revert'].includes(match[1])) {
      violations.push({ rule: 'named-color', match: match[0] });
    }
  }
  add('gradient', /\b(?:linear-gradient|radial-gradient|conic-gradient)\s*\(/gi);
  add('colored-shadow', /(?:boxShadow|box-shadow|drop-shadow)[^\n;}]*(?:#|rgba?\(|hsla?\(|blue|red|green|purple|orange|violet|magenta|cyan)/gi);
  add('icon-package', /@ant-design\/icons/g);
  add('transition-all', /\btransition\s*:\s*['"]?all\b/gi);
  add('font-weight', /\bfontWeight\s*:\s*['"]?(?:[6-9]\d{2}|bold|bolder)\b/gi);
  add('font-weight', /\bfont-weight\s*:\s*(?:[6-9]\d{2}|bold|bolder)\b/gi);
  add('uppercase', /\btextTransform\s*:\s*['"]uppercase['"]/gi);
  add('uppercase', /\btext-transform\s*:\s*uppercase\b/gi);
  add('negative-letter-spacing', /\bletterSpacing\s*:\s*-\d/g);
  add('negative-letter-spacing', /\bletter-spacing\s*:\s*-\d/g);
  add('emoji', /[\u{1F000}-\u{1FAFF}\u{2600}-\u{27BF}]/gu);
  for (const literal of copyLiterals(source, fileType, filePath)) {
    if (literal.text.includes('!')) violations.push({ rule: 'copy-exclamation', match: literal.source });
  }
  add('copy-exclamation', />\s*[A-Za-z\u4e00-\u9fff][^<{\n()&|=;]*![^<{\n()&|=;]*</g);

  for (const match of source.matchAll(/\bcolor\s*=\s*\{?['"]([A-Za-z]+)['"]\}?/g)) {
    if (!ALLOWED_ANTD_COLORS.has(match[1])) violations.push({ rule: 'preset-color', match: match[0] });
  }
  for (const match of source.matchAll(/\bcolor\s*:\s*['"]([A-Za-z]+)['"]/g)) {
    if (!ALLOWED_ANTD_COLORS.has(match[1])) violations.push({ rule: 'preset-color', match: match[0] });
  }
  for (const match of source.matchAll(/\bborderRadius\s*:\s*(\d+)/g)) {
    if (!ALLOWED_RADII.has(Number(match[1]))) violations.push({ rule: 'radius', match: match[0] });
  }
  for (const match of source.matchAll(/\bborder-radius\s*:\s*(\d+)px/g)) {
    if (!ALLOWED_RADII.has(Number(match[1]))) violations.push({ rule: 'radius', match: match[0] });
  }
  void filePath; // Kept for scanner API compatibility; freshness is checked at every call site.
  add('freshness-misuse', /freshness\.state\.not_measured/g);

  // The file type is explicit so callers cannot accidentally scan an unsupported extension.
  if (fileType !== 'tsx' && fileType !== 'css') throw new Error(`Unsupported craft source type: ${fileType}`);
  return violations;
}
