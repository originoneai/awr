import { describe, expect, it } from 'vitest';
import { contractGroups } from './lib/contract-groups';

/** The merger both pages share: agreements merge only when they are the whole same agreement. */
describe('contractGroups', () => {
  it('merges works whose goals and acceptance are both the same', () => {
    const groups = contractGroups([
      { title: 'A', goals: ['one'], acceptance: ['two'] },
      { title: 'B', goals: ['one'], acceptance: ['two'] },
    ]);
    expect(groups).toEqual([{ titles: ['A', 'B'], goals: ['one'], acceptance: ['two'] }]);
  });

  it('keeps apart works that agree on goals but not on acceptance (and the other way round)', () => {
    const groups = contractGroups([
      { title: 'A', goals: ['one'], acceptance: ['two'] },
      { title: 'B', goals: ['one'], acceptance: ['other'] },
      { title: 'C', goals: ['different'], acceptance: ['two'] },
    ]);
    expect(groups).toHaveLength(3);
  });

  it('gives no group to a work with no goals and no acceptance', () => {
    expect(contractGroups([{ title: 'A', goals: [], acceptance: [] }, { title: 'B' }])).toEqual([]);
  });

  it('groups a work that has only goals, and one that has only acceptance', () => {
    const groups = contractGroups([
      { title: 'A', goals: ['only goal'] },
      { title: 'B', acceptance: ['only acceptance'] },
    ]);
    expect(groups).toEqual([
      { titles: ['A'], goals: ['only goal'], acceptance: undefined },
      { titles: ['B'], goals: undefined, acceptance: ['only acceptance'] },
    ]);
  });
});
