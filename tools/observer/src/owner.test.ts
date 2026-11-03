import { describe, expect, it } from 'vitest';
import { deriveOwner, ownerCellText } from './core/observation';

/**
 * The shape of the responsibility the service records in a work snapshot, written from the
 * recorded answers of a rehearsal run (synthetic names and ids of our own). In the real data
 * the overview row's owner fields are null and the name lives only here.
 */
describe('deriveOwner', () => {
  const snapshot = (ownerName: unknown, owner: unknown) => ({
    observation: { responsibility: { owner_name: ownerName, owner } },
  });

  it('uses the row\'s own owner field when it has one', () => {
    const result = deriveOwner({ rowOwner: '周宁', snapshot: snapshot('Mei', 'mei'), readState: 'ready' });
    expect(result.name).toBe('周宁');
    expect(result.source).toBe('row');
  });

  it('falls back to the snapshot\'s owner name when the row has none (the real data\'s常态)', () => {
    const result = deriveOwner({ rowOwner: null, snapshot: snapshot('Mei', 'mei'), readState: 'ready' });
    expect(result.name).toBe('Mei');
    expect(result.source).toBe('snapshot');
  });

  it('falls back further to the owner id as written, a short person id reads as a name', () => {
    const result = deriveOwner({ rowOwner: null, snapshot: snapshot(null, 'mei'), readState: 'ready' });
    expect(result.name).toBe('mei');
    expect(result.source).toBe('snapshot');
  });

  it('says loading, not "not provided", while the read is still on its way', () => {
    const result = deriveOwner({ rowOwner: null, snapshot: null, readState: 'loading' });
    expect(result.loading).toBe(true);
    expect(ownerCellText(result, { language: 'zh-CN' })).toBe('加载中…');
    expect(ownerCellText(result, { language: 'en-US' })).toBe('Loading…');
    // And the loading answer never comes from a snapshot that happens to be present.
    const stale = deriveOwner({ rowOwner: null, snapshot: snapshot('Mei', 'mei'), readState: 'loading' });
    expect(ownerCellText(stale, { language: 'zh-CN' })).toBe('加载中…');
  });

  it('says not provided when the read failed or the snapshot holds no responsibility', () => {
    expect(ownerCellText(deriveOwner({ rowOwner: null, snapshot: null, readState: 'failed' }), { language: 'zh-CN' })).toBe('未提供');
    expect(ownerCellText(deriveOwner({ rowOwner: null, snapshot: snapshot(null, null), readState: 'ready' }), { language: 'zh-CN' })).toBe('未提供');
    expect(ownerCellText(deriveOwner({ rowOwner: null, snapshot: {}, readState: 'ready' }), { language: 'en-US' })).toBe('Not provided');
  });
});
