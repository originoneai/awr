export const SEARCH_RESULT_LIMIT = 10;
export const SEARCH_RECENT_LIMIT = 6;

export const SEARCH_KINDS = ['project', 'task', 'person', 'goal', 'document', 'lesson'] as const;
export type SearchKind = (typeof SEARCH_KINDS)[number];
export type SearchCoverage = 'complete' | 'partial' | 'unread';

/** A navigation-only entry. The key and path are never presented as search text. */
export interface SearchItem {
  key: string;
  kind: SearchKind;
  title: string;
  meta: string;
  path: string;
}

/** Each kind has one list-backed source; unread sources never contribute entries. */
export interface SearchSource {
  kind: SearchKind;
  coverage: SearchCoverage;
  items: readonly SearchItem[];
  failed?: boolean;
  /** Set only when all-authorized scope silently falls back to this project for tasks and people. */
  implicitProjectName?: string;
}

export interface IndexedSearchItem extends SearchItem {
  normalizedText: string;
}

export interface SearchIndex {
  items: readonly IndexedSearchItem[];
  byKey: ReadonlyMap<string, IndexedSearchItem>;
}

/** Fold letter case and compatibility forms; invisible direction controls cannot hide a match. */
export function normalizeSearchText(value: string): string {
  return value.normalize('NFKC')
    .replace(/[\u200b-\u200f\u202a-\u202e\u2060-\u2064\u2066-\u206f\ufeff]/g, '')
    .toLocaleLowerCase();
}

/** Build only when a source list changes, not for each keystroke. */
export function buildSearchIndex(sources: readonly SearchSource[]): SearchIndex {
  const items: IndexedSearchItem[] = [];
  const byKey = new Map<string, IndexedSearchItem>();
  for (const source of sources) {
    if (source.coverage === 'unread') continue;
    for (const item of source.items) {
      if (byKey.has(item.key)) continue;
      const indexed = {
        ...item,
        normalizedText: normalizeSearchText(`${item.title} ${item.meta}`),
      };
      items.push(indexed);
      byKey.set(item.key, indexed);
    }
  }
  return { items, byKey };
}

/** Immediate, case-insensitive substring matching with a hard ten-row ceiling. */
export function searchIndex(index: SearchIndex, query: string): readonly IndexedSearchItem[] {
  const normalizedQuery = normalizeSearchText(query.trim());
  if (!normalizedQuery) return [];
  const hits: IndexedSearchItem[] = [];
  for (const item of index.items) {
    if (!item.normalizedText.includes(normalizedQuery)) continue;
    hits.push(item);
    if (hits.length === SEARCH_RESULT_LIMIT) break;
  }
  return hits;
}

/** Recent references resolve against the current authorized index on every open. */
export function recentSearchItems(index: SearchIndex, keys: readonly string[]): readonly IndexedSearchItem[] {
  const items: IndexedSearchItem[] = [];
  const seen = new Set<string>();
  for (const key of keys) {
    if (seen.has(key)) continue;
    seen.add(key);
    const item = index.byKey.get(key);
    if (!item) continue;
    items.push(item);
    if (items.length === SEARCH_RECENT_LIMIT) break;
  }
  return items;
}
