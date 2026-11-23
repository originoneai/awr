import { ALLOWED_PRODUCT_WORDS, FORBIDDEN_VISIBLE_PATTERNS, type AllowedProductWord } from '../i18n/human-language';
import { IDENTIFIER_PATTERN_IDS } from '../core/observation/owner';
import { containsDiagnosticContent } from '../core/observation/readable';

export { ALLOWED_PRODUCT_WORDS, IDENTIFIER_PATTERN_IDS };
export type { AllowedProductWord };
export interface VisibleTextViolation {
  id: string;
  match: string;
}

export function findForbiddenVisibleText(text: string): VisibleTextViolation[] {
  return FORBIDDEN_VISIBLE_PATTERNS.flatMap(({ id, pattern }) => {
    const match = text.match(pattern);
    return match ? [{ id, match: match[0] }] : [];
  });
}

/** Reporter prose is checked for identifier shapes and structured diagnostics. */
// The one set lives in the core; the guard and the owner gate share it.

export function findIdentifierText(text: string): VisibleTextViolation[] {
  const prose = text.replace(/https?:\/\/[^\s()]+/gi, (url) => containsDiagnosticContent(url) ? url : '');
  return findForbiddenVisibleText(prose).filter((violation) => IDENTIFIER_PATTERN_IDS.has(violation.id));
}

/** The mark a page puts on a sentence its work's reporter wrote and the page shows as written. */
const REPORTER_SELECTOR = '[data-reporter-text]';

function textOf(root: ParentNode): string {
  const owner = (root as Node).ownerDocument ?? document;
  const walker = owner.createTreeWalker(root as Node, NodeFilter.SHOW_TEXT);
  const parts: string[] = [];
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    const text = (node.textContent ?? '').trim();
    if (text) parts.push(text);
  }
  return parts.join('\n');
}

/**
 * Check all rendered text, including disclosures and interactive overlays.
 * Keep separate elements apart. `textContent` glues neighbours together, so a
 * file name followed by a capital letter or a digit lost its word boundary and escaped the patterns.
 */
export function defaultVisibleText(root: ParentNode): string {
  return textOf(root);
}

/**
 * The default visible text in two parts: the words of the page itself, and the sentences a work's reporter
 * wrote (elements marked `data-reporter-text`), after the shared product projection.
 */
export function visibleTextParts(root: ParentNode): { own: string; reporter: string } {
  const clone = root.cloneNode(true) as ParentNode;
  const marked = [...clone.querySelectorAll(REPORTER_SELECTOR)].filter((element) => element.parentElement?.closest(REPORTER_SELECTOR) == null);
  const reporter = marked.map((element) => textOf(element)).filter(Boolean).join('\n');
  marked.forEach((element) => element.remove());
  return { own: textOf(clone), reporter };
}

/** Product vocabulary, reporter references and diagnostic payloads are checked after rendering. */
export function findHumanLanguageViolations(root: ParentNode): VisibleTextViolation[] {
  const product = root.cloneNode(true) as ParentNode;
  // Only the supported connection address may expose the service endpoint.
  for (const element of product.querySelectorAll('[data-agent-connection-address]')) {
    try {
      const url = new URL(element.textContent ?? '');
      if (['https:', 'http:'].includes(url.protocol) && !url.username && !url.password && !url.search && !url.hash
        && /^\/v\d+\/projects\/[^/]+\/mcp\/?$/.test(url.pathname)) element.remove();
    } catch { /* Unrecognized content receives the ordinary product checks. */ }
  }
  const { own, reporter } = visibleTextParts(product);
  const descriptions = [...root.querySelectorAll('[title],[aria-label],[alt]')]
    .flatMap(element => ['title', 'aria-label', 'alt'].map(attribute => element.getAttribute(attribute) ?? ''));
  const diagnostics = [own, reporter, ...descriptions].flatMap((text) => text.split('\n')
    .filter(containsDiagnosticContent).map((match) => ({ id: 'diagnostic_content', match })));
  return [...findForbiddenVisibleText(own), ...findIdentifierText(reporter), ...descriptions.flatMap(findForbiddenVisibleText), ...diagnostics];
}

export function withinHumanLanguageBaseline(current: number, baseline: number): boolean {
  return current === baseline;
}
