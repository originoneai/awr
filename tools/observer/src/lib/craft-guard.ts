export type CraftFileType = 'tsx' | 'css';

export interface CraftViolation {
  rule: string;
  match: string;
}

const ALLOWED_ANTD_COLORS = new Set(['success', 'error', 'warning', 'processing', 'default', 'currentColor', 'inherit']);
const ALLOWED_RADII = new Set([4, 8, 12, 16]);
const FRESHNESS_FILES = new Set([
  'components/FreshnessBadge.tsx',
  'components/WorkDetailPanel.tsx',
  'pages/ProjectOverview.tsx',
  'pages/WorkbenchPage.tsx',
  // Out of this baseline PR; its separate craft pass owns all remaining copy.
  'pages/ActivityPage.tsx',
]);

/** Pure source scanner used by both the self-test and the full workspace guard. */
export function scanCraftSource(source: string, fileType: CraftFileType, filePath = ''): CraftViolation[] {
  const violations: CraftViolation[] = [];
  const add = (rule: string, pattern: RegExp) => {
    for (const match of source.matchAll(pattern)) violations.push({ rule, match: match[0] });
  };

  add('hex-color', /#[0-9a-f]{3,8}\b/gi);
  add('color-function', /\b(?:rgba?|hsla?)\s*\(/gi);
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
  add('copy-exclamation', /(['"`])[^'"`\n]*![^'"`\n]*\1/g);
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
  if (!FRESHNESS_FILES.has(filePath)) add('freshness-misuse', /freshness\.state\.not_measured/g);

  // The file type is explicit so callers cannot accidentally scan an unsupported extension.
  if (fileType !== 'tsx' && fileType !== 'css') throw new Error(`Unsupported craft source type: ${fileType}`);
  return violations;
}
