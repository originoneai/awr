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

/**
 * The text of a page as a person reads it: the entire technical-details region, collapsed or expanded, is
 * left out, and the text of separate elements is kept apart. `textContent` glues neighbours together, so a
 * file name followed by a capital letter or a digit lost its word boundary and escaped the patterns.
 */
export function defaultVisibleText(root: ParentNode): string {
  const clone = root.cloneNode(true) as ParentNode;
  clone.querySelectorAll('[data-testid="technical-details"]').forEach((element) => element.remove());
  const owner = (clone as Node).ownerDocument ?? document;
  const walker = owner.createTreeWalker(clone as Node, NodeFilter.SHOW_TEXT);
  const parts: string[] = [];
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    const text = (node.textContent ?? '').trim();
    if (text) parts.push(text);
  }
  return parts.join('\n');
}

export function withinHumanLanguageBaseline(current: number, baseline: number): boolean {
  return current === baseline;
}
