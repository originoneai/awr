/**
 * Cleaning for untrusted free text (task names, project names, question text) before it
 * is quoted inside a handback line. The line is pasted into an Agent chat, so anything
 * that could change meaning there — newlines, markdown, command-looking prefixes,
 * invisible characters, quoting characters, placeholder syntax — is neutralized, and
 * identifier-shaped or credential-shaped tokens are dropped entirely. Pure; no I/O.
 *
 * Cost is bounded before any pattern runs: the raw input is cut to a few thousand code
 * points (the output can never exceed the sentence cap, so nothing past a small
 * multiple of the budget matters) and every repetition in every pattern is bounded, so
 * hostile long inputs finish in linear time.
 */

export interface SanitizedText {
  text: string;
  truncated: boolean;
}

/** Control characters (newlines, tabs) become separators; deleting them would merge words. */
const CONTROL = /[\p{Cc}]/gu;
/** Invisible and invalid characters (zero-width, bidi overrides, lone surrogates, private use) are deleted. */
const INVISIBLE = /[\p{Cf}\p{Cs}\p{Co}\p{Cn}]/gu;
/** Hidden marks: variation selectors other than the presentation selectors, plus the combining grapheme joiner. U+FE0E and U+FE0F stay (emoji need them). */
const HIDDEN_MARKS = /[\u{FE00}-\u{FE0D}\u{E0100}-\u{E01EF}\u034F]/gu;

/** Credential-shaped tokens never belong in a spoken line. */
const CREDENTIAL_SHAPED = [
  /\bbearer\s+[A-Za-z0-9._~+/=-]{8,}/gi,
  /\b(?:sk|pk|ghp|gho|ghu|xox[bap]|AKIA|AIza)[A-Za-z0-9_-]{8,}\b/g,
];

/**
 * Identifier-shaped tokens: hashes, ids, work-item keys, protocol fields. Dropped, not
 * displayed. Every repetition is bounded ({0,3}) so a long hostile name cannot make
 * the pattern backtrack.
 */
const IDENTIFIER_SHAPED = [
  /\bsha(?:256|512):[0-9A-Za-z]+/gi,
  /\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b/gi,
  /\b[0-9a-f]{12,}\b/gi,
  /\b[0-9A-HJKMNP-TV-Z]{26}\b/g,
  /\b[A-Z][A-Z0-9]{0,7}-(?:[A-Z0-9]{1,8}-){0,3}\d[A-Z0-9]*\b/g,
  /\b[a-z][a-z0-9]*_[a-z0-9_]+\b/g,
];

/**
 * A dotted name with three or more segments reads as a machine path whatever its
 * first segment is; two-segment dotted names are dropped only when the first segment
 * is a service namespace (see SERVICE_NAMESPACES), so "server.js" and "example.com"
 * stay while "work.list" goes.
 */
const THREE_PLUS_DOTTED = /\b[a-z][a-z0-9_]*(?:\.[a-z][a-z0-9_]*){2,}\b/gi;
const TWO_SEGMENT_DOTTED = /\b[a-z][a-z0-9_]*\.[a-z][a-z0-9_]*\b/gi;
const SERVICE_NAMESPACES = new Set([
  'work', 'task', 'session', 'claim', 'execution', 'review', 'delivery', 'handoff', 'evidence',
  'planning', 'update', 'updates', 'audit', 'decision', 'decisions', 'dependency', 'dependencies',
  'source', 'sources', 'document', 'documents', 'workstream', 'workstreams', 'completion',
  'command', 'artifact', 'events',
]);

/** Quotes, backticks, brackets and braces become fullwidth look-alikes: no code spans, links, tags or placeholder syntax can survive. */
const REPLACEMENTS: ReadonlyArray<readonly [RegExp, string]> = [
  [/["“”]/g, '＂'],
  [/['‘’`]/g, '＇'],
  [/</g, '＜'],
  [/>/g, '＞'],
  [/\[/g, '［'],
  [/\]/g, '］'],
  [/\{/g, '｛'],
  [/\}/g, '｝'],
];

/** Leading markdown and command markers are stripped: # > - * + ` / ! (before replacements, so they are still recognizable). */
const LEADING_MARKERS = /^[#>*\-+/!`]+(?:\s*[#>*\-+/!`]+)*/;

function foldWhitespace(text: string): string {
  return text.replace(/\s+/g, ' ').trim();
}

function dropMatches(text: string, patterns: readonly RegExp[]): string {
  let out = text;
  for (const pattern of patterns) out = out.replace(pattern, ' ');
  return out;
}

/** Two-segment dotted names go only when their first segment is a service namespace. */
function dropServiceOperations(text: string): string {
  return text.replace(TWO_SEGMENT_DOTTED, (match) => (
    SERVICE_NAMESPACES.has(match.split('.', 1)[0].toLowerCase()) ? ' ' : match
  ));
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
 * contains newlines, tabs, straight quotes, backticks, angle brackets, square brackets,
 * ASCII braces, invisible or hidden-mark characters, lone surrogates, live placeholders,
 * or identifier/credential-shaped tokens.
 */
export function sanitizeUntrustedText(input: string | null | undefined, budget: number): SanitizedText {
  if (typeof input !== 'string' || input.length === 0) return { text: '', truncated: false };
  // Cut the raw input before any pattern runs: nothing past a small multiple of the
  // budget can matter, and the cut is what keeps long hostile inputs linear.
  const rawLimit = Math.max(budget * 4, 1024);
  const codePoints = Array.from(input);
  const raw = codePoints.length > rawLimit ? codePoints.slice(0, rawLimit).join('') : input;
  // Fullwidth ASCII and exotic spaces fold to their plain forms first.
  const normalized = raw.normalize('NFKC');
  // Control characters become separators (words must not merge); invisible ones and
  // hidden marks are deleted.
  const separated = normalized.replace(CONTROL, ' ').replace(INVISIBLE, '').replace(HIDDEN_MARKS, '');
  // Newlines and tabs die here; only spaces survive.
  const folded = foldWhitespace(separated);
  if (folded === '') return { text: '', truncated: false };
  // Identifiers, credentials, machine paths and service operations are dropped, not displayed.
  const scrubbed = foldWhitespace(
    dropServiceOperations(dropMatches(dropMatches(folded, CREDENTIAL_SHAPED), [...IDENTIFIER_SHAPED, THREE_PLUS_DOTTED])),
  );
  // Markdown and command structure is neutralized: leading markers first (still
  // recognizable), then every quoting, bracketing and brace character.
  let text = scrubbed.replace(LEADING_MARKERS, '').trim();
  for (const [pattern, replacement] of REPLACEMENTS) text = text.replace(pattern, replacement);
  text = text.trim();
  if (text === '') return { text: '', truncated: false };
  const truncated = truncateCodePoints(text, budget);
  return truncated.text === '' ? { text: '', truncated: false } : truncated;
}

export { countCodePoints };
