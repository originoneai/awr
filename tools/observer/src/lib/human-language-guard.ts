import { FORBIDDEN_VISIBLE_PATTERNS } from '../i18n/human-language';

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

/** The patterns that are shapes of identifiers; a reporter's own words are scanned for these and nothing else. */
export const IDENTIFIER_PATTERN_IDS: ReadonlySet<string> = new Set(['snake_case', 'operation_name', 'long_hex', 'ulid', 'uuid', 'sha256', 'error_class']);

export function findIdentifierText(text: string): VisibleTextViolation[] {
  return findForbiddenVisibleText(text).filter((violation) => IDENTIFIER_PATTERN_IDS.has(violation.id));
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

function withoutTechnicalDetails(root: ParentNode): ParentNode {
  const clone = root.cloneNode(true) as ParentNode;
  clone.querySelectorAll('[data-testid="technical-details"]').forEach((element) => element.remove());
  return clone;
}

/**
 * The text of a page as a person reads it: the entire technical-details region, collapsed or expanded, is
 * left out, and the text of separate elements is kept apart. `textContent` glues neighbours together, so a
 * file name followed by a capital letter or a digit lost its word boundary and escaped the patterns.
 */
export function defaultVisibleText(root: ParentNode): string {
  return textOf(withoutTechnicalDetails(root));
}

/**
 * The default visible text in two parts: the words of the page itself, and the sentences a work's reporter
 * wrote (elements marked `data-reporter-text`), which the page shows as written and cannot rewrite.
 */
export function visibleTextParts(root: ParentNode): { own: string; reporter: string } {
  const clone = withoutTechnicalDetails(root);
  const marked = [...clone.querySelectorAll(REPORTER_SELECTOR)].filter((element) => element.parentElement?.closest(REPORTER_SELECTOR) == null);
  const reporter = marked.map((element) => textOf(element)).filter(Boolean).join('\n');
  marked.forEach((element) => element.remove());
  return { own: textOf(clone), reporter };
}

/** What the ratchet counts: the page's own words against the whole list, a reporter's words against identifiers only. */
export function findHumanLanguageViolations(root: ParentNode): VisibleTextViolation[] {
  const { own, reporter } = visibleTextParts(root);
  return [...findForbiddenVisibleText(own), ...findIdentifierText(reporter)];
}

export function withinHumanLanguageBaseline(current: number, baseline: number): boolean {
  return current === baseline;
}
