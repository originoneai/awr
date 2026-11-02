import type { ReadState } from '../core/observation';

/** The read state the core derivations take, from the state of a react-query result. */
export function readStateOf(query: { isError: boolean; isPending: boolean } | undefined): ReadState {
  if (query?.isError) return 'failed';
  return query && !query.isPending ? 'ready' : 'loading';
}
