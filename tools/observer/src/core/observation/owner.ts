import { humanText } from '../../i18n/human-language';
import { statementText } from './values';
import type { WorkStateInput } from './types';

export type OwnerSource = 'row' | 'snapshot' | 'none';
export type OwnerReadState = 'ready' | 'loading' | 'failed';

export interface OwnerResult {
  /** The name to show, or null when nothing honest can be said. */
  name: string | null;
  source: OwnerSource;
  /** Whether the only reason there is no name yet is that the read is still running. */
  loading: boolean;
  technical: { row: string | null; snapshotOwnerName: string | null; snapshotOwner: string | null };
}

/**
 * Who is responsible for one work, in the words the data actually holds: the overview row's own
 * owner field when it has one, else the snapshot's responsibility (the owner's name, else the
 * owner id as written — a short person id reads as a name); a read that has not come back yet
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
    row,
    snapshotOwnerName: statementText(input.snapshot?.observation?.responsibility?.owner_name) ?? null,
    snapshotOwner: statementText(input.snapshot?.observation?.responsibility?.owner) ?? null,
  };
  if (row != null) return { name: row, source: 'row', loading: false, technical };
  if (input.readState === 'loading') {
    return { name: null, source: 'none', loading: true, technical };
  }
  if (input.readState === 'ready') {
    const name = technical.snapshotOwnerName ?? technical.snapshotOwner;
    if (name != null) return { name, source: 'snapshot', loading: false, technical };
  }
  return { name: null, source: 'none', loading: false, technical };
}

/** The owner result as one word for a table cell: the name, the loading word, or not provided. */
export function ownerCellText(result: OwnerResult, options: { language: string }): string {
  if (result.loading) return humanText('loading', options.language);
  return result.name ?? humanText('notProvided', options.language);
}
