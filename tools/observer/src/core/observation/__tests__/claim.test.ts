import { describe, expect, it } from 'vitest';
import { claimState } from '../index';

describe('claimState', () => {
  it('names a claimed work, a work nobody holds, and says "not provided" when the field is missing', () => {
    expect(claimState('claimed', { language: 'zh-CN' })).toEqual({ kind: 'claimed', text: '已认领', technical: { raw: 'claimed' } });
    expect(claimState(null, { language: 'zh-CN' })).toEqual({ kind: 'unclaimed', text: '未认领', technical: { raw: null } });
    expect(claimState(undefined, { language: 'zh-CN' })).toEqual({ kind: 'unknown', text: '未提供', technical: { raw: null } });
  });

  it('never words a value that is not a claim state as one, and keeps it verbatim for the technical layer', () => {
    for (const value of ['released', '', 0, false, { state: 'claimed' }]) {
      const result = claimState(value, { language: 'zh-CN' });
      expect(result.kind, JSON.stringify(value)).toBe('unknown');
      expect(result.text).toBe('未提供');
    }
    expect(claimState('released', { language: 'zh-CN' }).technical.raw).toBe('released');
  });

  it('words English the same way', () => {
    expect(claimState('claimed', { language: 'en-US' }).text).toBe('Claimed');
    expect(claimState(null, { language: 'en-US' }).text).toBe('Unclaimed');
    expect(claimState(undefined, { language: 'en-US' }).text).toBe('Not provided');
  });
});
