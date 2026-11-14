import { describe, expect, it } from 'vitest';
import { OPERATION_VOCABULARY } from '../../../i18n/human-language';
import REPORTS from '../../../test-fixtures/work-state/progress-reports-run09.json';
import { abbreviateIds, readableSentence } from '../index';

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

describe('readableSentence', () => {
  const DELIVERED = REPORTS['api-delivered'];
  const IN_REVIEW = REPORTS['api-in-review-owned-by-other'];

  it('says the operation and check names of the real checkpoints in words and cuts the ids, in both languages', () => {
    expect(readableSentence(DELIVERED.completed[0], 'zh-CN')).toBe('准备并入主线 相关记录 confirmed applied');
    expect(readableSentence(DELIVERED.completed[0], 'en-US')).toBe('Prepare mainline integration the related record confirmed applied');
    expect(readableSentence(DELIVERED.completed[1], 'zh-CN')).toBe('完成交付 receipt 相关记录');
    expect(readableSentence(DELIVERED.completed[1], 'en-US')).toBe('Finalize delivery receipt the related record');
    expect(readableSentence(IN_REVIEW.summary, 'zh-CN')).toBe('Status and keyword filters are on branch issue-filter-api at 相关记录. npm test passed 5 of 5. 本地内容与测试 passed. Review is open.');
    expect(readableSentence(IN_REVIEW.summary, 'en-US')).toContain('npm test passed 5 of 5. Local content and tests passed. Review is open.');
  });

  it('says every operation of the shared vocabulary that has a dotted or snake_case name as the activity page words it', () => {
    const named = Object.entries(OPERATION_VOCABULARY).filter(([name]) => /[._]/.test(name));
    expect(named.length).toBeGreaterThan(30);
    for (const [name, words] of named) {
      expect(readableSentence(name, 'zh-CN'), name).toBe(words.zh);
      expect(readableSentence(`ran ${name}.`, 'en-US'), name).toBe(`ran ${words.en}.`);
    }
  });

  it('leaves a machine name nobody has words for exactly as written', () => {
    expect(readableSentence('ran foo.bar_baz', 'zh-CN')).toBe('ran 相关操作');
    expect(readableSentence('the read_only flag', 'zh-CN')).toBe('the 相关操作 flag');
    expect(readableSentence('filterIssues combines status', 'zh-CN')).toBe('filterIssues combines status');
  });

  it('replaces whole names only', () => {
    for (const text of ['xdelivery.finalize', 'Xdelivery.finalize', '2delivery.finalize', 'delivery.finalize2x', 'my.delivery.finalize', 'delivery.finalizes']) {
      expect(readableSentence(text, 'en-US'), text).not.toContain('Finalize delivery');
    }
    expect(readableSentence('(delivery.finalize), then delivery.finalize.', 'en-US')).toBe('(Finalize delivery), then Finalize delivery.');
  });

  it('does nothing the second time and nothing to an empty text', () => {
    const once = readableSentence(DELIVERED.completed[0], 'en-US');
    expect(readableSentence(once, 'en-US')).toBe(once);
    expect(readableSentence('', 'zh-CN')).toBe('');
  });
});
