import { describe, expect, it } from 'vitest';
import OVERVIEW from '../../../test-fixtures/overview/overview-works-run09.json';
import { groupWorks } from '../index';

type Work = { key: string; title: string; workstream_id: string };
const REAL_WORKS = OVERVIEW.works as Work[];
const REAL_STREAMS = OVERVIEW.workstreams as Array<{ id: string; external_key?: string; title?: string }>;

const work = (key: string, workstream_id: string): Work => ({ key, title: key, workstream_id });

describe('groupWorks', () => {
  it('keeps the real run-09 works in service order under their one workstream, named by its title', () => {
    const result = groupWorks(REAL_WORKS, REAL_STREAMS);
    expect(result.works.map((item) => item.key)).toEqual(['issue-filter-api', 'issue-filter-page']);
    expect(result.groups).toHaveLength(1);
    expect(result.groups[0].title).toBe('Customer support queue');
    expect(result.titles).toEqual({ [REAL_STREAMS[0].id]: 'Customer support queue' });
  });

  it('names a workstream by its title, else its key, else its id', () => {
    const result = groupWorks([], [
      { id: 'a', title: 'Alpha', external_key: 'alpha-key' },
      { id: 'b', external_key: 'beta-key' },
      { id: 'c' },
      { id: 'd', title: '', external_key: '' },
    ]);
    expect(result.titles).toEqual({ a: 'Alpha', b: 'beta-key', c: 'c', d: 'd' });
  });

  it('puts every work in exactly one group, in service order, and appends works of unknown workstreams without copying or dropping any', () => {
    const works = [work('w1', 'x'), work('w2', 'unknown-1'), work('w3', 'y'), work('w4', 'x'), work('w5', 'unknown-2')];
    const result = groupWorks(works, [{ id: 'y', title: 'Y' }, { id: 'x', title: 'X' }]);
    expect(result.groups.map((group) => [group.id, group.works.map((item) => item.key)])).toEqual([['y', ['w3']], ['x', ['w1', 'w4']]]);
    expect(result.works.map((item) => item.key)).toEqual(['w3', 'w1', 'w4', 'w2', 'w5']);
    expect([...result.works.map((item) => item.key)].sort()).toEqual(['w1', 'w2', 'w3', 'w4', 'w5']);
    expect(result.titles.unknown).toBeUndefined();
  });

  it('does not merge works that share a title and does not copy works into a repeated workstream entry', () => {
    const works = [{ key: 'one', title: 'Same', workstream_id: 'x' }, { key: 'two', title: 'Same', workstream_id: 'y' }];
    const result = groupWorks(works, [{ id: 'x', title: 'First' }, { id: 'x', title: 'Repeated' }, { id: 'y', title: 'Second' }]);
    expect(result.groups.map((group) => group.title)).toEqual(['First', 'Second']);
    expect(result.works.map((item) => item.key)).toEqual(['one', 'two']);
    expect(result.titles.x).toBe('First');
  });

  it('leaves every work in place when there are no workstreams at all', () => {
    const works = [work('w1', 'x'), work('w2', 'y')];
    const result = groupWorks(works, []);
    expect(result.works).toEqual(works);
    expect(result.groups).toEqual([]);
  });
});
