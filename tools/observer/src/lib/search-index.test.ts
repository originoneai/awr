import { describe, expect, it } from 'vitest';
import {
  buildSearchIndex,
  recentSearchItems,
  searchIndex,
  type SearchItem,
  type SearchSource,
} from './search-index';

function item(index: number, title = `合成任务 ${index}`): SearchItem {
  return {
    key: `task-${index}`,
    kind: 'task',
    title,
    meta: `合成项目 ${index % 3} · 合成成员`,
    path: `/projects/demo?tab=plan&work=${index}`,
  };
}

describe('search index', () => {
  it('matches Chinese, English and digits without case sensitivity', () => {
    const index = buildSearchIndex([{ kind: 'task', coverage: 'complete', items: [item(1, '交付 Alpha 2048')] }]);
    for (const query of ['交付', 'alpha', 'ALPHA', '2048', '２０４８']) {
      expect(searchIndex(index, query).map((entry) => entry.key)).toEqual(['task-1']);
    }
  });

  it('matches through invisible controls without interpreting title text as markup', () => {
    const title = '\u200b交付\u202eAlpha\u2066 2048';
    const index = buildSearchIndex([{ kind: 'task', coverage: 'complete', items: [item(1, title)] }]);
    expect(searchIndex(index, 'alpha')[0]?.title).toBe(title);
    expect(searchIndex(index, '交付')).toHaveLength(1);
  });

  it('returns at most the first ten matches in source order', () => {
    const index = buildSearchIndex([{ kind: 'task', coverage: 'complete', items: Array.from({ length: 14 }, (_, n) => item(n)) }]);
    const hits = searchIndex(index, '合成任务');
    expect(hits).toHaveLength(10);
    expect(hits.map((entry) => entry.key)).toEqual(Array.from({ length: 10 }, (_, n) => `task-${n}`));
  });

  it('never indexes items attached to an unread source', () => {
    const source: SearchSource = { kind: 'task', coverage: 'unread', items: [item(1, '不应出现的记录')] };
    const index = buildSearchIndex([source]);
    expect(index.items).toEqual([]);
    expect(searchIndex(index, '不应出现')).toEqual([]);
  });

  it('resolves only six current recent references and drops stale or duplicate references', () => {
    const index = buildSearchIndex([{ kind: 'task', coverage: 'complete', items: Array.from({ length: 8 }, (_, n) => item(n)) }]);
    const recent = recentSearchItems(index, ['missing', 'task-7', 'task-7', 'task-6', 'task-5', 'task-4', 'task-3', 'task-2', 'task-1']);
    expect(recent.map((entry) => entry.key)).toEqual(['task-7', 'task-6', 'task-5', 'task-4', 'task-3', 'task-2']);
  });

  it('searches ten thousand records within the generous CI budget and reports the measured time', () => {
    const records = Array.from({ length: 10_000 }, (_, n) => item(n, n === 9_999 ? '末尾目标 2048' : `普通工作 ${n}`));
    const index = buildSearchIndex([{ kind: 'task', coverage: 'complete', items: records }]);
    const startedAt = performance.now();
    const found = searchIndex(index, '末尾目标');
    const missing = searchIndex(index, '完全不存在的词');
    const elapsedMs = performance.now() - startedAt;
    console.info(`[search-index] 10,000 records, two full scans: ${elapsedMs.toFixed(3)} ms; assertion budget: 250 ms`);
    expect(found.map((entry) => entry.key)).toEqual(['task-9999']);
    expect(missing).toEqual([]);
    expect(elapsedMs).toBeLessThan(250);
  });
});
