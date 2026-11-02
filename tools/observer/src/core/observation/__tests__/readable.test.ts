import { describe, expect, it } from 'vitest';
import { abbreviateIds } from '../index';

describe('abbreviateIds', () => {
  it('cuts ULIDs, git object ids, digests and UUIDs to their first eight characters', () => {
    expect(abbreviateIds('round 01M6AVQ870N7MBRJ5MPCN79T55 is open')).toBe('round 01M6AVQ8… is open');
    expect(abbreviateIds('from a516342436e434d0072ef9c138f419f2f128caac to 5f7eb2fe9c563e4d355aa64a527e0aef754116c1')).toBe('from a5163424… to 5f7eb2fe…');
    expect(abbreviateIds('digest 295b63612f58425c516a25c6e4d5d777d1da6661cc11cce9fb93ffc3c9597c4d.')).toBe('digest 295b6361….');
    expect(abbreviateIds('session 3f2504e0-4f89-41d3-9a0c-0305e82c3301 ended')).toBe('session 3f2504e0… ended');
  });

  it('cuts every occurrence, and each kind in the same sentence', () => {
    const text = 'a 01M6AVQ870N7MBRJ5MPCN79T55 b 01M6B189DXC1A21ZD5YQ6V7EG5 c 5f7eb2fe9c563e4d355aa64a527e0aef754116c1 d 3f2504e0-4f89-41d3-9a0c-0305e82c3301';
    expect(abbreviateIds(text)).toBe('a 01M6AVQ8… b 01M6B189… c 5f7eb2fe… d 3f2504e0…');
  });

  it('leaves everything else alone: ordinary words and numbers, short commit ids, names, paths and Chinese text', () => {
    for (const text of [
      '',
      'Wait for independent review. Do not self-approve or merge to main.',
      'commit 5f7eb2f and abcdef12 were merged',
      'refs/heads/issue-filter-api and delivery.finalize',
      'build 20261101T045929Z passed 5 of 5',
      'ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789',
      'abcdefabcdefabcdefabcdefabcdefabcdefabc',
      '等待独立评审，不要自行批准。',
    ]) {
      expect(abbreviateIds(text), text).toBe(text);
    }
  });

  it('does nothing the second time', () => {
    const once = abbreviateIds('round 01M6AVQ870N7MBRJ5MPCN79T55 at 5f7eb2fe9c563e4d355aa64a527e0aef754116c1');
    expect(abbreviateIds(once)).toBe(once);
  });
});
