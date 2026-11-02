import { describe, expect, it } from 'vitest';
import { findForbiddenVisibleText } from '../../../lib/human-language-guard';
import { describeUnknown, rawObservationText, statusWord } from '../index';
import { APPROVED_REVIEW, DEVELOPMENT_ACTIVITY, OVERVIEW_REPLY, UNKNOWN_REQUEST } from './fixtures';

describe('rawObservationText', () => {
  it('keeps real missing and scalar values exact', () => {
    expect(rawObservationText(OVERVIEW_REPLY.owner_person)).toEqual({ text: null, technical: { raw: null } });
    expect(rawObservationText(OVERVIEW_REPLY.status).technical.raw).toBe('unknown');
    expect(rawObservationText(false).technical.raw).toBe('false');
    expect(rawObservationText(42).technical.raw).toBe('42');
  });

  it('does not coerce numeric-looking strings', () => {
    for (const value of ['0042', '1.10', '9007199254740993']) {
      expect(rawObservationText(value).text).toBe(value);
      expect(rawObservationText(value).technical.raw).toBe(value);
    }
  });

  it('serializes objects instead of leaking object coercion', () => {
    const result = rawObservationText({ state: 'approved', count: 2 });
    expect(result.technical.raw).toBe('{"state":"approved","count":2}');
    expect(result.text).not.toBe('[object Object]');
  });
});

describe('statusWord', () => {
  it('uses the shared human vocabulary for real review and activity states', () => {
    expect(statusWord(APPROVED_REVIEW.state, { language: 'zh-CN', field: 'state' })).toMatchObject({
      text: '已批准', known: true, tone: 'positive', technical: { raw: 'approved' },
    });
    expect(statusWord(APPROVED_REVIEW.state, { language: 'en-US', field: 'state' }).text).toBe('Approved');
    expect(statusWord(DEVELOPMENT_ACTIVITY.result, { language: 'zh-CN', field: 'result' }).text).toBe('已提交');
  });

  it('colours each class of status word by its meaning', () => {
    const tone = (value: string) => statusWord(value, { language: 'zh-CN', field: 'state' }).tone;
    for (const value of ['approved', 'confirmed', 'succeeded']) expect(tone(value), value).toBe('positive');
    for (const value of ['failed', 'rejected', 'invalidated', 'blocking']) expect(tone(value), value).toBe('negative');
    for (const value of ['pending', 'open', 'claimed', 'dispatched']) expect(tone(value), value).toBe('warning');
    for (const value of ['current', 'disabled', 'unknown']) expect(tone(value), value).toBe('neutral');
  });

  it('never translates a title that happens to look like a status', () => {
    expect(statusWord('open', { language: 'zh-CN', field: 'title' })).toMatchObject({ text: 'open', known: false, technical: { raw: 'open' } });
  });

  it('keeps an unrecognized enum technical and gives a human default', () => {
    const result = statusWord('covered', { language: 'zh-CN', field: 'status' });
    expect(result).toMatchObject({ text: '未识别状态', known: false, tone: 'neutral', technical: { raw: 'covered' } });
  });

  it('uses ordinary missing copy, never the freshness state', () => {
    const result = statusWord(null, { language: 'zh-CN', field: 'state' });
    expect(result.text).toBe('未提供');
    expect(result.text).not.toBe('尚未读取');
    expect(result.missing).toBe(true);
    expect(result.technical.raw).toBeNull();
  });
});

describe('describeUnknown', () => {
  it('maps the real unknown explanation to human language and preserves it verbatim', () => {
    const result = describeUnknown({ ...UNKNOWN_REQUEST, language: 'zh-CN' });
    expect(result.isUnknown).toBe(true);
    expect(result.text).toBe('未知');
    expect(result.disclosure).toBe('还没有完成记录；请先核对原业务请求，再决定是否重试');
    expect(result.disclosure).not.toContain('completion_not_recorded');
    expect(result.technical).toEqual({ result: 'unknown', means: UNKNOWN_REQUEST.unknown_means });
    expect(findForbiddenVisibleText(`${result.text}${result.disclosure}`)).toEqual([]);
  });

  it('does not warn for a non-unknown real result', () => {
    const result = describeUnknown({ ...DEVELOPMENT_ACTIVITY, language: 'zh-CN' });
    expect(result).toMatchObject({ isUnknown: false, text: null, disclosure: null });
    expect(result.technical.result).toBe('committed');
  });

  it('uses generic human copy for an unmapped reason and retains the original', () => {
    const result = describeUnknown({ result: 'unknown', unknown_means: 'backend_timeout_stage_unknown', language: 'en-US' });
    expect(result.disclosure).toBe('The outcome is uncertain. Check it before retrying');
    expect(result.disclosure).not.toContain('backend_timeout');
    expect(result.technical.means).toBe('backend_timeout_stage_unknown');
  });

  it('declares a missing reason without borrowing freshness vocabulary', () => {
    const result = describeUnknown({ result: 'unknown', unknown_means: null, language: 'zh-CN' });
    expect(result.disclosure).toBe('结果暂时不确定，具体原因未提供');
    expect(result.disclosure).not.toContain('尚未读取');
    expect(result.technical.means).toBeNull();
  });
});
