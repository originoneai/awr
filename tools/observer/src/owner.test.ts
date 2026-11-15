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

  it('never substitutes a short person identifier for a missing display name', () => {
    const result = deriveOwner({ rowOwner: null, snapshot: snapshot(null, 'mei'), readState: 'ready' });
    expect(result.name).toBeNull();
    expect(result.source).toBe('none');
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

  it('rejects machine-shaped names while preserving short Agent model versions', () => {
    // q avoids the hex shapes: 40 a's would be a long hex string by the identifier rules.
    expect(readsAsName('q'.repeat(41))).toBe(false); // S2: past the 40 bound
    expect(readsAsName('q'.repeat(40))).toBe(true);
    for (const value of ['docs/support', 'x<y', 'hey!', '👍thumbsup', 'name	tab']) { // S5
      expect(readsAsName(value), value).toBe(false);
      const result = deriveOwner({ rowOwner: null, snapshot: snapshot(null, value), readState: 'ready' });
      expect(result.name, value).toBeNull();
    }
    for (const value of ['Grok4', 'GLM4.6', 'Agent 2', 'lin2']) {
      expect(readsAsName(value), value).toBe(true);
    }
    expect(readsAsName('2026')).toBe(false);
    expect(readsAsName('Agent 1234')).toBe(false);
  });

  it.each(['client[secret]=synthetic-name', '{"token":"synthetic-name"}', 'agent_private_123', 'actor-123', 'AWR-OBS-123', 'a'.repeat(64)])(
    'keeps a supplied unreadable name distinct from an absent name for %s', (value) => {
      for (const input of [
        { rowOwner: value, snapshot: null, readState: 'ready' as const },
        { rowOwner: null, snapshot: snapshot(value, 'person'), readState: 'ready' as const },
      ]) {
        const result = deriveOwner(input);
        expect(result.name).toBeNull();
        expect(result.source).not.toBe('none');
        expect(ownerCellText(result, { language: 'zh-CN' })).toBe('名称暂时无法显示');
        expect(ownerCellText(result, { language: 'en-US' })).toBe('Name cannot be displayed');
      }
    },
  );

  it('recognizes names in any script while requiring an explicit display-name field', () => {
    for (const owner of ['mei', 'Lin', '周宁', 'Zhou Ning', "O'Brien", 'Jean-Luc']) {
      expect(readsAsName(owner), owner).toBe(true);
      const result = deriveOwner({ rowOwner: null, snapshot: snapshot(owner, 'internal-person'), readState: 'ready' });
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


describe('supplied non-name values', () => {
  it.each([{ token: 'synthetic-name-object' }, [], 7, false, 'agent7', 'dev42', 'client9'])(
    'keeps a supplied unsafe name distinct from no name for %j', (value) => {
      for (const input of [
        { rowOwner: value, snapshot: null, readState: 'ready' as const },
        { rowOwner: null, snapshot: { observation: { responsibility: { owner_name: value } } }, readState: 'ready' as const },
      ]) {
        const result = deriveOwner(input);
        expect(result.name).toBeNull();
        expect(ownerCellText(result, { language: 'en-US' })).toBe('Name cannot be displayed');
      }
    },
  );
});
