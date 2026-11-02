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

/** Exclude the entire technical-details region, whether collapsed or expanded. */
export function defaultVisibleText(root: ParentNode): string {
  const clone = root.cloneNode(true) as ParentNode;
  clone.querySelectorAll('[data-testid="technical-details"]').forEach((element) => element.remove());
  return clone.textContent ?? '';
}

export function withinHumanLanguageBaseline(current: number, baseline: number): boolean {
  return current === baseline;
}
