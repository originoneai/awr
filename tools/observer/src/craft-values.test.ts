import { describe, expect, it } from 'vitest';
import { formatMetric, formatObservationValue, statusLabel } from './i18n/craft';

describe('observed value fidelity (review r1.1)', () => {
  it.each([
    '1.10',
    '2.0',
    '0042',
    '9007199254740993',
    '12345678901234567890',
  ])('preserves the exact numeric-looking string %s', (value) => {
    expect(formatObservationValue(value, 'en-US')).toBe(value);
  });

  it('does not translate arbitrary titles that happen to equal status codes', () => {
    expect(formatObservationValue('current', 'zh-CN')).toBe('current');
    expect(formatObservationValue('open', 'zh-CN')).toBe('open');
    expect(formatObservationValue('pool', 'zh-CN')).toBe('pool');
    expect(statusLabel('current', 'zh-CN', 'title')).toBe('current');
  });

  it('formats real numbers and only explicit count/version string fields', () => {
    expect(formatMetric(1000, 'en-US')).toBe('1,000');
    expect(formatObservationValue(1000, 'en-US')).toBe('1,000');
    expect(formatObservationValue('1000', 'en-US', 'items_count')).toBe('1,000');
    expect(formatObservationValue('1000', 'en-US', 'work_id')).toBe('1000');
    expect(formatObservationValue('approved', 'zh-CN', 'state')).toBe('已批准');
  });
});
