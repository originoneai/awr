import { describe, expect, it } from 'vitest';
import { formatObservationValue, rawObservationValue, statusLabel } from '../../../i18n/craft';
import { formatUnixMs } from '../../../lib/format';
import { absoluteTimeText } from '../index';
import { APPROVED_REVIEW, DEVELOPMENT_ACTIVITY, SNAPSHOT_REPLY } from './fixtures';

describe('craft compatibility goldens', () => {
  it('keeps existing page status text unchanged while sharing one vocabulary', () => {
    expect(statusLabel(APPROVED_REVIEW.state, 'zh-CN', 'state')).toBe('已批准');
    expect(statusLabel(APPROVED_REVIEW.state, 'en-US', 'state')).toBe('Approved');
    expect(formatObservationValue(DEVELOPMENT_ACTIVITY.result, 'zh-CN', 'result')).toBe('已提交');
    expect(formatObservationValue('open', 'zh-CN', 'title')).toBe('open');
    expect(formatObservationValue('covered', 'zh-CN', 'status')).toBe('covered');
  });

  it('keeps raw serialization and absolute-time goldens unchanged', () => {
    expect(rawObservationValue('0042')).toBe('0042');
    const at = SNAPSHOT_REPLY.clientReportedAtUnixMs;
    expect(formatUnixMs(at, 'zh-CN')).toBe(absoluteTimeText(at, { language: 'zh-CN' }).text);
    expect(formatUnixMs(at, 'en-US')).toBe(absoluteTimeText(at, { language: 'en-US' }).text);
  });
});
