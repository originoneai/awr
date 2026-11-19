/**
 * Cleaning for untrusted free text (task names, project names, question text) before it
 * is quoted inside a handback line. The line is pasted into an Agent chat, so anything
 * that could change meaning there — newlines, markdown, command-looking prefixes,
 * invisible characters, quoting characters — is neutralized, and identifier-shaped or
 * credential-shaped tokens are dropped entirely. Pure; no I/O.
 */

export interface SanitizedText {
  text: string;
  truncated: boolean;
}

/** Control characters (newlines, tabs) become separators; deleting them would merge words. */
const CONTROL = /[\p{Cc}]/gu;
/** Invisible and invalid characters (zero-width, bidi overrides, lone surrogates, private use) are deleted. */
const INVISIBLE = /[\p{Cf}\p{Cs}\p{Co}\p{Cn}]/gu;

/** Credential-shaped tokens never belong in a spoken line. */
const CREDENTIAL_SHAPED = [
  /\bbearer\s+[A-Za-z0-9._~+/=-]{8,}/gi,
  /\b(?:sk|pk|ghp|gho|ghu|xox[bap]|AKIA|AIza)[A-Za-z0-9_-]{8,}\b/g,
];

/** Identifier-shaped tokens: hashes, ids, work-item keys, protocol fields. Dropped, not displayed. */
const IDENTIFIER_SHAPED = [
  /\bsha(?:256|512):[0-9A-Za-z]+/gi,
  /\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b/gi,
  /\b[0-9a-f]{12,}\b/gi,
  /\b[0-9A-HJKMNP-TV-Z]{26}\b/g,
  /\b[A-Z][A-Z0-9]{0,7}-(?:[A-Z0-9]{1,8}-)*\d[A-Z0-9]*\b/g,
  /\b[a-z][a-z0-9]*_[a-z0-9_]+\b/g,
  /\b[a-z][a-z0-9_]*\.[a-z][a-z0-9_]*\b/gi,
];

/** Latin abbreviations a dotted-name scrub would otherwise eat ("e.g.", "i.e."). */
const DOTTED_ALLOWLIST = new Set(['e.g', 'i.e', 'etc']);

/** Markdown link syntax becomes fullwidth brackets, so it cannot render as a link anywhere. */
const MARKDOWN_LINK = /\[([^\[\]]*)\]\(([^()]*)\)/g;

/** Quotes and backticks become fullwidth look-alikes that cannot open code spans or quotes. */
const QUOTES: ReadonlyArray<readonly [RegExp, string]> = [
  [/["“”]/g, '＂'],
  [/['‘’`]/g, '＇'],
];

/** Angle brackets become fullwidth, so HTML or XML-like tags cannot parse. */
const ANGLE: ReadonlyArray<readonly [RegExp, string]> = [
  [/</g, '＜'],
  [/>/g, '＞'],
];

/** Leading markdown and command markers are stripped: # > - * + ` / ! (before quote replacement, so they are still recognizable). */
const LEADING_MARKERS = /^[#>*\-+/!`]+(?:\s*[#>*\-+/!`]+)*/;

function foldWhitespace(text: string): string {
  return text.replace(/\s+/g, ' ').trim();
}

function dropMatches(text: string, patterns: readonly RegExp[]): string {
  let out = text;
  for (const pattern of patterns) {
    out = out.replace(pattern, (match) => (DOTTED_ALLOWLIST.has(match.toLowerCase()) ? match : ' '));
  }
  return out;
}

function countCodePoints(text: string): number {
  return Array.from(text).length;
}

/** Truncate by code point (never splitting a surrogate pair), marking the cut with an ellipsis. */
export function truncateCodePoints(text: string, budget: number): SanitizedText {
  const chars = Array.from(text);
  if (chars.length <= budget) return { text, truncated: false };
  if (budget <= 0) return { text: '', truncated: true };
  // A budget of one keeps a single readable character; larger budgets leave room for the ellipsis.
  const keep = budget === 1 ? 1 : budget - 1;
  const mark = budget === 1 ? '' : '…';
  return { text: chars.slice(0, keep).join('') + mark, truncated: true };
}

/**
 * Clean one untrusted value. Returns empty text for null, undefined or text that is only
 * invisible characters; the caller substitutes its own fallback wording. The result never
 * contains newlines, tabs, straight quotes, backticks, angle brackets, invisible or control
 * characters, lone surrogates, or identifier/credential-shaped tokens.
 */
export function sanitizeUntrustedText(input: string | null | undefined, budget: number): SanitizedText {
  if (typeof input !== 'string' || input.length === 0) return { text: '', truncated: false };
  // Fullwidth ASCII and exotic spaces fold to their plain forms first.
  const normalized = input.normalize('NFKC');
  // Control characters become separators (words must not merge); invisible ones are deleted.
  const separated = normalized.replace(CONTROL, ' ').replace(INVISIBLE, '');
  // Newlines and tabs die here; only spaces survive.
  const folded = foldWhitespace(separated);
  if (folded === '') return { text: '', truncated: false };
  // Identifiers and credentials are dropped, not displayed.
  const scrubbed = foldWhitespace(dropMatches(dropMatches(folded, CREDENTIAL_SHAPED), IDENTIFIER_SHAPED));
  // Markdown structure is neutralized: leading markers first (still recognizable), then links, angle brackets, quotes.
  let text = scrubbed.replace(LEADING_MARKERS, '').trim();
  text = text.replace(MARKDOWN_LINK, '［$1］（$2）');
  for (const [pattern, replacement] of ANGLE) text = text.replace(pattern, replacement);
  for (const [pattern, replacement] of QUOTES) text = text.replace(pattern, replacement);
  text = text.trim();
  if (text === '') return { text: '', truncated: false };
  const truncated = truncateCodePoints(text, budget);
  return truncated.text === '' ? { text: '', truncated: false } : truncated;
}

export { countCodePoints };
