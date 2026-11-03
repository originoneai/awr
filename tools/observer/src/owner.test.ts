import { describe, expect, it } from 'vitest';
import { deriveOwner, ownerCellText, readsAsName } from './core/observation';

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

  it('falls back further to the owner id as written when it reads as a name', () => {
    const result = deriveOwner({ rowOwner: null, snapshot: snapshot(null, 'mei'), readState: 'ready' });
    expect(result.name).toBe('mei');
    expect(result.source).toBe('snapshot');
  });

  it('never shows a machine identifier from the owner field, whatever shape it takes', () => {
    for (const owner of [
      'person_01HXYZABCDEF0123456789', // underscore + ULID-like tail
      '01M6ARNTMQCWJMWM5QQVEK51MC', // ULID
      'b8a7f2c1-9d3e-4a5b-8c6d-0e1f2a3b4c5d', // UUID
      '0123456789abcdef0123456789abcdef01234567', // 40 hex digits
    ]) {
      const result = deriveOwner({ rowOwner: null, snapshot: snapshot(null, owner), readState: 'ready' });
      expect(result.name, owner).toBeNull();
      expect(result.technical.snapshotOwner, owner).toBe(owner);
      expect(ownerCellText(result, { language: 'zh-CN' }), owner).toBe('未提供');
    }
  });

  it('refuses the shape gate\'s own edges: too long, wrong characters, digits (S2/S5/S7)', () => {
    // q avoids the hex shapes: 40 a's would be a long hex string by the identifier rules.
    expect(readsAsName('q'.repeat(41))).toBe(false); // S2: past the 40 bound
    expect(readsAsName('q'.repeat(40))).toBe(true);
    for (const value of ['docs/support', 'x<y', 'hey!', '👍thumbsup', 'name	tab']) { // S5
      expect(readsAsName(value), value).toBe(false);
      const result = deriveOwner({ rowOwner: null, snapshot: snapshot(null, value), readState: 'ready' });
      expect(result.name, value).toBeNull();
    }
    for (const value of ['agent7', 'lin2', 'dev42']) { // S7: a digit anywhere refuses
      expect(readsAsName(value), value).toBe(false);
    }
    // Long digit runs were already covered by the identifier shapes; a single digit is the rule.
    expect(readsAsName('2026')).toBe(false);
  });

  it('shows owner values that read as names in any script', () => {
    for (const owner of ['mei', 'Lin', '周宁', 'Zhou Ning', "O'Brien", 'Jean-Luc']) {
      expect(readsAsName(owner), owner).toBe(true);
      const result = deriveOwner({ rowOwner: null, snapshot: snapshot(null, owner), readState: 'ready' });
      expect(result.name, owner).toBe(owner);
    }
  });

  it('says not provided when the read failed, even if a snapshot from before is still held (F6)', () => {
    // In the app a failed refetch keeps the last data; a failed read must not show it.
    const result = deriveOwner({ rowOwner: null, snapshot: snapshot('Mei', 'mei'), readState: 'failed' });
    expect(result.name).toBeNull();
    expect(ownerCellText(result, { language: 'zh-CN' })).toBe('未提供');
    // And the row's own owner is unaffected by the read state.
    const fromRow = deriveOwner({ rowOwner: '周宁', snapshot: null, readState: 'failed' });
    expect(fromRow.name).toBe('周宁');
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
