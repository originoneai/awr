import { describe, expect, it } from 'vitest';
import { absoluteTimeText, ageText } from '../index';
import { OVERVIEW_REPLY, SNAPSHOT_REPLY } from './fixtures';

const OPTIONS = { year: 'numeric', month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit', second: '2-digit', hour12: false } as const;

describe('absoluteTimeText', () => {
  it('matches the established absolute format for a real snapshot timestamp', () => {
    const at = SNAPSHOT_REPLY.clientReportedAtUnixMs;
    expect(absoluteTimeText(at, { language: 'zh-CN', timeZone: 'UTC' })).toEqual({
      text: new Intl.DateTimeFormat('zh-CN', { ...OPTIONS, timeZone: 'UTC' }).format(new Date(at)),
      technical: { atUnixMs: at },
    });
    expect(absoluteTimeText(at, { language: 'en-US', timeZone: 'UTC' }).text).toBe(
      new Intl.DateTimeFormat('en-US', { ...OPTIONS, timeZone: 'UTC' }).format(new Date(at)),
    );
  });

  it('honors the caller time zone', () => {
    const utc = absoluteTimeText(0, { language: 'en-US', timeZone: 'UTC' }).text;
    const pacific = absoluteTimeText(0, { language: 'en-US', timeZone: 'America/Los_Angeles' }).text;
    expect(utc).not.toBe(pacific);
  });

  it('returns missing for invalid input without inventing a clock', () => {
    for (const value of [null, undefined, Number.NaN, Number.POSITIVE_INFINITY]) {
      expect(absoluteTimeText(value, { language: 'zh-CN' })).toEqual({ text: '未提供', technical: { atUnixMs: null } });
    }
  });
});

describe('ageText', () => {
  const at = SNAPSHOT_REPLY.clientReportedAtUnixMs;

  it('uses relative text only for the latest hour with an explicit now', () => {
    const nowUnixMs = at + 3 * 60 * 1000;
    expect(ageText(at, { language: 'zh-CN', nowUnixMs, timeZone: 'UTC' })).toMatchObject({
      text: '3 分钟前', kind: 'relative', atUnixMs: at, referenceUnixMs: nowUnixMs,
      technical: { atUnixMs: at, referenceUnixMs: nowUnixMs, timeZone: 'UTC' },
    });
    expect(ageText(at, { language: 'en-US', nowUnixMs, timeZone: 'UTC' }).text).toBe('3 minutes ago');
    expect(ageText(at, { language: 'en-US', nowUnixMs: at + 60_000, timeZone: 'UTC' }).text).toBe('1 minute ago');
    expect(ageText(at, { language: 'zh-CN', nowUnixMs: at + 30_000, timeZone: 'UTC' }).text).toBe('刚刚');
  });

  it('uses today for older timestamps on the same local day', () => {
    const result = ageText(at, { language: 'zh-CN', nowUnixMs: at + 60 * 60 * 1000, timeZone: 'UTC' });
    expect(result.kind).toBe('today');
    expect(result.text).toMatch(/^今天 /);
  });

  it('uses the caller time zone for day boundaries', () => {
    const nowUnixMs = Date.UTC(2026, 0, 2, 0, 30);
    const earlier = nowUnixMs - 90 * 60 * 1000;
    expect(ageText(earlier, { language: 'en-US', nowUnixMs, timeZone: 'America/Los_Angeles' }).kind).toBe('today');
    expect(ageText(earlier, { language: 'en-US', nowUnixMs, timeZone: 'UTC' }).kind).toBe('absolute');
  });

  it('shows the factual absolute time for a future timestamp, never a negative countdown', () => {
    const nowUnixMs = at - 1;
    const result = ageText(at, { language: 'zh-CN', nowUnixMs, timeZone: 'UTC' });
    expect(result.kind).toBe('future');
    expect(result.text).toBe(absoluteTimeText(at, { language: 'zh-CN', timeZone: 'UTC' }).text);
    expect(result.text).not.toContain('-1');
  });

  it('uses absolute facts rather than future semantics for an invalid reference clock', () => {
    const result = ageText(at, { language: 'zh-CN', nowUnixMs: Number.NaN, timeZone: 'UTC' });
    expect(result.kind).toBe('absolute');
    expect(result.text).toBe(absoluteTimeText(at, { language: 'zh-CN', timeZone: 'UTC' }).text);
  });

  it('uses absolute dates for older days and missing copy for absent facts', () => {
    const older = ageText(OVERVIEW_REPLY.pageReadAtUnixMs, { language: 'zh-CN', nowUnixMs: OVERVIEW_REPLY.pageReadAtUnixMs + 86400000, timeZone: 'UTC' });
    expect(older.kind).toBe('absolute');
    expect(ageText(null, { language: 'zh-CN', nowUnixMs: at, timeZone: 'UTC' })).toMatchObject({ text: '未提供', kind: 'missing', atUnixMs: null });
  });
});
