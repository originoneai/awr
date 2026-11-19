// @vitest-environment node
import { describe, expect, it } from 'vitest';
import { sanitizeUntrustedText, truncateCodePoints } from './sanitize';
import { MALICIOUS_TASK_NAMES, textInvariants } from './handback-test-utils';

const BUDGET = 240;

describe('sanitizeUntrustedText on malicious task names', () => {
  for (const { input, keeps } of MALICIOUS_TASK_NAMES) {
    it(`cleans ${JSON.stringify(input.slice(0, 40))} and still reads as the thing`, () => {
      const { text } = sanitizeUntrustedText(input, BUDGET);
      expect(textInvariants(text, BUDGET), text).toEqual([]);
      expect(text).toContain(keeps);
    });
  }

  it('returns empty text for null, undefined and invisible-only input', () => {
    expect(sanitizeUntrustedText(null, BUDGET)).toEqual({ text: '', truncated: false });
    expect(sanitizeUntrustedText(undefined, BUDGET)).toEqual({ text: '', truncated: false });
    expect(sanitizeUntrustedText('\u200B\u200D\u202E\uFEFF', BUDGET)).toEqual({ text: '', truncated: false });
  });

  it('drops credentials and identifier shapes wherever they appear', () => {
    for (const input of [
      'ghp_16C7e42Z9f8aBc0123456789 推进',
      '检查 550e8400-e29b-41d4-a716-446655440000 的记录',
      '7ASZFJQK9T4MNPQRTVWX24680M 延迟',
      'created_at 与 work.read 字段核对',
      'AWR-PPO-012 与 T-112 的联调',
    ]) {
      const { text } = sanitizeUntrustedText(input, BUDGET);
      expect(textInvariants(text, BUDGET), text).toEqual([]);
    }
  });

  it('truncates by code point, never splitting an emoji or a Chinese character', () => {
    const { text, truncated } = sanitizeUntrustedText('订单🧑‍💻查询'.repeat(60), 30);
    expect(truncated).toBe(true);
    expect(Array.from(text).length).toBe(30);
    expect(text.endsWith('…')).toBe(true);
    expect(textInvariants(text, 30)).toEqual([]);
    expect(sanitizeUntrustedText('空状态检查', 4)).toEqual({ text: '空状态…', truncated: true });
  });

  it('marks truncation only when something was actually cut', () => {
    expect(sanitizeUntrustedText('短任务', BUDGET).truncated).toBe(false);
    expect(truncateCodePoints('一二三', 5)).toEqual({ text: '一二三', truncated: false });
  });
});

/**
 * Fault injection: each case takes an output that a broken sanitizer would produce
 * (newline folding dropped, invisible-character removal dropped, code-unit truncation)
 * and proves the predicates above flag it — so the tests fail on that fault — while
 * the real sanitizer's output on the same input passes every predicate.
 */
describe('the malicious-name tests catch broken sanitizers', () => {
  const evil = '第一行\n第二行\u200B<script>说"完成"</script>🧑‍💻';

  it('flags output a fold-less sanitizer would produce (newline survives)', () => {
    const broken = evil.replace(/[\p{Cf}]/gu, '');
    expect(textInvariants(broken, BUDGET)).toContain('contains a line break or tab');
    const { text } = sanitizeUntrustedText(evil, BUDGET);
    expect(textInvariants(text, BUDGET)).toEqual([]);
    expect(text).not.toMatch(/[\r\n]/);
  });

  it('flags output an invisible-keeping sanitizer would produce (zero-width survives)', () => {
    const broken = evil.normalize('NFKC').replace(/\s+/g, ' ');
    expect(textInvariants(broken, BUDGET)).toContain('contains an invisible or bidi character');
    const { text } = sanitizeUntrustedText(evil, BUDGET);
    expect(textInvariants(text, BUDGET)).toEqual([]);
  });

  it('flags output a code-unit (byte-style) truncation would produce (split surrogate pair)', () => {
    const long = '👨‍👩‍👧🧑‍💻订单'.repeat(40);
    const broken = long.slice(0, 31); // 31 UTF-16 units lands inside a surrogate pair
    expect(textInvariants(broken, 31)).toContain('contains a lone surrogate');
    const { text } = sanitizeUntrustedText(long, 31);
    expect(textInvariants(text, 31)).toEqual([]);
    expect(Array.from(text).length).toBe(31);
  });

  it('flags output a quote-and-bracket-keeping sanitizer would produce', () => {
    const broken = evil.normalize('NFKC').replace(/[\p{Cc}\p{Cf}\p{Cs}\p{Co}\p{Cn}]/gu, ' ').replace(/\s+/g, ' ');
    expect(textInvariants(broken, BUDGET)).toContain('contains an angle bracket');
    expect(textInvariants(broken, BUDGET)).toContain('contains a straight quote');
  });
});
