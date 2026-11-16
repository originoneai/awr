import { FORBIDDEN_VISIBLE_PATTERNS, humanText } from '../../i18n/human-language';
import { hasObservationValue, rawObservationText, statementText } from './values';
import { readableNarrative } from './readable';
import type { WorkStateInput } from './types';

export type OwnerSource = 'row' | 'snapshot' | 'historical' | 'none';
export type OwnerReadState = 'ready' | 'loading' | 'failed';

export interface OwnerResult {
  /** The name to show, or null when nothing honest can be said. */
  name: string | null;
  source: OwnerSource;
  /** Whether the only reason there is no name yet is that the read is still running. */
  loading: boolean;
  technical: { row: string | null; snapshotOwnerName: string | null; snapshotOwner: string | null };
}

/** The pattern ids that mean "this text is an identifier"; shared with the human-language guard. */
export const IDENTIFIER_PATTERN_IDS = new Set(['snake_case', 'operation_name', 'long_hex', 'ulid', 'uuid', 'sha256', 'error_class']);

/**
 * Does this raw value read as a person's name — short, letters (any script), short model versions, spaces, dots,
 * hyphens and apostrophes, no identifier shape the guard already knows and no long digit runs?
 * A machine identifier in the owner field is not shown as a name; the raw value stays for the
 * backend records.
 */
export function readsAsName(value: string): boolean {
  if (value.length === 0 || value.length > 40) return false;
  if (readableNarrative(value, 'en-US') !== value) return false;
  if (FORBIDDEN_VISIBLE_PATTERNS.some(({ id, pattern }) => IDENTIFIER_PATTERN_IDS.has(id) && pattern.test(value))) {
    return false;
  }
  if (/\d{4,}/.test(value)) return false;
  // Short model versions are part of legitimate Agent names.
  return /^[\p{L}][\p{L}\p{M}\p{N} .'-]*$/u.test(value);
}

/**
 * Who is responsible for one work, in the words the data actually holds: the overview row's own
 * owner field when it has one, else the snapshot's explicit owner name. Identifiers never
 * substitute for a display name. A read that has not come back yet
 * says it is loading rather than "not provided", which is a claim the page cannot make while
 * the answer is still on its way; a failed read or no responsibility at all leaves the honest
 * "not provided".
 */
export function deriveOwner(input: {
  rowOwner: unknown;
  snapshot: WorkStateInput['snapshot'];
  readState: OwnerReadState;
}): OwnerResult {
  const row = statementText(input.rowOwner);
  const technical = {
    row: rawObservationText(input.rowOwner).text,
    snapshotOwnerName: rawObservationText(input.snapshot?.observation?.responsibility?.owner_name).text,
    snapshotOwner: statementText(input.snapshot?.observation?.responsibility?.owner) ?? null,
  };
  const history = (value: unknown): OwnerResult | null => {
    if (!value || typeof value !== 'object' || Array.isArray(value)) return null;
    const entry = value as Record<string, unknown>;
    if (entry.provenance !== 'historical' || typeof entry.name !== 'string') return null;
    return { name: readsAsName(entry.name) ? entry.name : null, source: 'historical', loading: false, technical };
  };
  const rowHistory = history(input.rowOwner);
  const responsibility = input.snapshot?.observation?.responsibility;
  const currentRecorded = input.readState === 'ready' && responsibility?.version != null && responsibility.version !== '0';
  if (rowHistory && !currentRecorded) return rowHistory;
  if (rowHistory && currentRecorded) {
    const currentName = statementText(responsibility?.owner_name);
    return { name: currentName && readsAsName(currentName) ? currentName : null, source: 'snapshot', loading: false, technical };
  }
  if (hasObservationValue(input.rowOwner)) return { name: row != null && readsAsName(row) ? row : null, source: 'row', loading: false, technical };
  if (input.readState === 'loading') {
    return { name: null, source: 'none', loading: true, technical };
  }
  if (input.readState === 'ready') {
    const ownerName = input.snapshot?.observation?.responsibility?.owner_name;
    if (hasObservationValue(ownerName)) {
      const name = statementText(ownerName);
      return { name: name != null && readsAsName(name) ? name : null, source: 'snapshot', loading: false, technical };
    }
    const retained = !currentRecorded && !hasObservationValue(responsibility?.owner)
      ? history(responsibility?.historical) : null;
    if (retained) return retained;
  }
  return { name: null, source: 'none', loading: false, technical };
}

/** The owner result as one word for a table cell: the name, the loading word, or not provided. */
export function ownerCellText(result: OwnerResult, options: { language: string }): string {
  if (result.loading) return humanText('loading', options.language);
  if (result.name == null && result.source !== 'none') return humanText('nameUnavailable', options.language);
  if (result.source === 'historical' && result.name) {
    return `${result.name} ${options.language.toLowerCase().startsWith('zh') ? '（历史记录）' : '(historical record)'}`;
  }
  return result.name ?? humanText('notProvided', options.language);
}
