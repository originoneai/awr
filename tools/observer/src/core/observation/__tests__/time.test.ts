import { describe, expect, it } from 'vitest';
import { absoluteTimeText, ageText, compactTimeText } from '../index';
import { OVERVIEW_REPLY, SNAPSHOT_REPLY } from './fixtures';

const OPTIONS = { year: 'numeric', month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit', second: '2-digit', hour12: false } as const;

describe('compactTimeText', () => {
  it('keeps relative and today wording without seconds in both languages', () => {
    const at = Date.UTC(2026, 10, 18, 9, 40, 59);
    expect(compactTimeText(at, { language: 'zh-CN', nowUnixMs: at + 12 * 60_000, timeZone: 'Asia/Shanghai' })).toBe('12 分钟前');
    expect(compactTimeText(at, { language: 'en-US', nowUnixMs: at + 12 * 60_000, timeZone: 'America/New_York' })).toBe('12 minutes ago');
    expect(compactTimeText(at, { language: 'zh-CN', nowUnixMs: at + 2 * 60 * 60_000, timeZone: 'Asia/Shanghai' })).toBe('今天 17:40');
    expect(compactTimeText(at, { language: 'en-US', nowUnixMs: at + 2 * 60 * 60_000, timeZone: 'America/New_York' })).toBe('Today at 04:40');
  });

  it('uses the viewer zone across midnight instead of the UTC date', () => {
    const at = Date.UTC(2026, 0, 2, 16, 30, 45);
    const now = Date.UTC(2026, 0, 3, 4, 0);
    expect(compactTimeText(at, { language: 'zh-CN', nowUnixMs: now, timeZone: 'Asia/Shanghai' })).toBe('今天 00:30');
    expect(compactTimeText(at, { language: 'zh-CN', nowUnixMs: now, timeZone: 'UTC' })).toBe('昨天 16:30');
    // Beyond 24 hours the same zone split reads as the two zones' own calendar dates.
    const lateNow = Date.UTC(2026, 0, 3, 18, 0);
    expect(compactTimeText(at, { language: 'zh-CN', nowUnixMs: lateNow, timeZone: 'Asia/Shanghai' })).toBe('1 月 3 日 00:30');
    expect(compactTimeText(at, { language: 'en-US', nowUnixMs: lateNow, timeZone: 'America/New_York' })).toBe('Jan 2, 11:30');
  });

  it('keeps yesterday wording only inside the last 24 hours of the reference clock', () => {
    // The prototype's ledger: 15:20 twenty hours old still says 昨天, the same yesterday
    // five hours older already says its calendar date (11 月 18 日 10:02).
    const now = Date.UTC(2026, 10, 19, 12, 0);
    expect(compactTimeText(Date.UTC(2026, 10, 18, 15, 20), { language: 'zh-CN', nowUnixMs: now, timeZone: 'UTC' })).toBe('昨天 15:20');
    expect(compactTimeText(Date.UTC(2026, 10, 18, 15, 20), { language: 'en-US', nowUnixMs: now, timeZone: 'UTC' })).toBe('Yesterday at 15:20');
    expect(compactTimeText(Date.UTC(2026, 10, 18, 10, 2), { language: 'zh-CN', nowUnixMs: now, timeZone: 'UTC' })).toBe('11 月 18 日 10:02');
    expect(compactTimeText(Date.UTC(2026, 10, 18, 10, 2), { language: 'en-US', nowUnixMs: now, timeZone: 'UTC' })).toBe('Nov 18, 10:02');
  });

  it('crosses midnight with relative wording inside the hour and calendar wording after it', () => {
    const now = Date.UTC(2026, 10, 19, 0, 30);
    expect(compactTimeText(Date.UTC(2026, 10, 18, 23, 45), { language: 'zh-CN', nowUnixMs: now, timeZone: 'UTC' })).toBe('45 分钟前');
    expect(compactTimeText(Date.UTC(2026, 10, 18, 22, 0), { language: 'zh-CN', nowUnixMs: now, timeZone: 'UTC' })).toBe('昨天 22:00');
    // The same pair in a zone west of UTC keeps its own day split.
    expect(compactTimeText(Date.UTC(2026, 10, 18, 22, 0), { language: 'zh-CN', nowUnixMs: now, timeZone: 'America/New_York' })).toBe('今天 17:00');
  });

  it('crosses a month boundary with yesterday inside 24 hours and a calendar date beyond', () => {
    const now = Date.UTC(2026, 11, 1, 10, 0);
    expect(compactTimeText(Date.UTC(2026, 10, 30, 20, 0), { language: 'zh-CN', nowUnixMs: now, timeZone: 'UTC' })).toBe('昨天 20:00');
    expect(compactTimeText(Date.UTC(2026, 10, 29, 8, 0), { language: 'zh-CN', nowUnixMs: now, timeZone: 'UTC' })).toBe('11 月 29 日 08:00');
  });

  it('crosses a year boundary the same way, adding the year only for earlier years', () => {
    const now = Date.UTC(2027, 0, 1, 10, 0);
    expect(compactTimeText(Date.UTC(2026, 11, 31, 20, 0), { language: 'zh-CN', nowUnixMs: now, timeZone: 'UTC' })).toBe('昨天 20:00');
    expect(compactTimeText(Date.UTC(2026, 11, 30, 23, 0), { language: 'zh-CN', nowUnixMs: now, timeZone: 'UTC' })).toBe('2026 年 12 月 30 日 23:00');
    expect(compactTimeText(Date.UTC(2026, 11, 30, 23, 0), { language: 'en-US', nowUnixMs: now, timeZone: 'UTC' })).toBe('Dec 30, 2026, 23:00');
  });

  it('keeps the zone calendar on daylight-saving switch days', () => {
    // America/New_York leaves DST on 2026-11-01: the 22-hour-old fact below spans the
    // repeated hour and still reads as yesterday's wall clock.
    const fallNow = Date.UTC(2026, 10, 1, 17, 0);
    expect(compactTimeText(Date.UTC(2026, 9, 31, 19, 0), { language: 'zh-CN', nowUnixMs: fallNow, timeZone: 'America/New_York' })).toBe('昨天 15:00');
    // Entering DST on 2026-03-08: 23 hours old, still yesterday.
    const springNow = Date.UTC(2026, 2, 8, 16, 0);
    expect(compactTimeText(Date.UTC(2026, 2, 7, 17, 0), { language: 'zh-CN', nowUnixMs: springNow, timeZone: 'America/New_York' })).toBe('昨天 12:00');
  });

  it('falls back to the calendar fact when no reference clock is supplied', () => {
    const at = Date.UTC(2026, 10, 18, 10, 2);
    expect(compactTimeText(at, { language: 'zh-CN', nowUnixMs: Number.NaN, timeZone: 'UTC' })).toBe('2026 年 11 月 18 日 10:02');
    expect(compactTimeText(at, { language: 'en-US', nowUnixMs: Number.NaN, timeZone: 'UTC' })).toBe('Nov 18, 2026, 10:02');
  });

  it('handles new year boundaries and includes the year only for earlier years', () => {
    const now = Date.UTC(2027, 0, 1, 12, 0);
    const at = Date.UTC(2026, 11, 31, 16, 5, 30);
    expect(compactTimeText(at, { language: 'zh-CN', nowUnixMs: now, timeZone: 'Asia/Shanghai' })).toBe('今天 00:05');
    expect(compactTimeText(at, { language: 'en-US', nowUnixMs: now, timeZone: 'America/New_York' })).toBe('Yesterday at 11:05');

    const olderNow = Date.UTC(2027, 0, 3, 12, 0);
    const older = Date.UTC(2025, 11, 31, 23, 5, 59);
    expect(compactTimeText(older, { language: 'zh-CN', nowUnixMs: olderNow, timeZone: 'Asia/Shanghai' })).toBe('2026 年 1 月 1 日 07:05');
    expect(compactTimeText(older, { language: 'en-US', nowUnixMs: olderNow, timeZone: 'America/New_York' })).toBe('Dec 31, 2025, 18:05');
  });

  it('uses compact numeric dates within the current year and never emits seconds', () => {
    const now = Date.UTC(2026, 11, 31, 12, 0);
    const at = Date.UTC(2026, 10, 18, 10, 2, 59);
    expect(compactTimeText(at, { language: 'zh-CN', nowUnixMs: now, timeZone: 'Asia/Shanghai' })).toBe('11 月 18 日 18:02');
    expect(compactTimeText(at, { language: 'en-US', nowUnixMs: now, timeZone: 'America/New_York' })).toBe('Nov 18, 05:02');
    expect(compactTimeText(at, { language: 'en-US', nowUnixMs: now, timeZone: 'America/New_York' })).not.toContain(':59');
    expect(compactTimeText(Number.NaN, { language: 'zh-CN', nowUnixMs: now, timeZone: 'Asia/Shanghai' })).toBe('未提供');
  });
});

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

  it('reads the clock in 24 hours for an afternoon time, in both languages', () => {
    const afternoon = Date.UTC(2026, 10, 2, 17, 5, 9);
    expect(absoluteTimeText(afternoon, { language: 'en-US', timeZone: 'UTC' }).text).toBe('11/02/2026, 17:05:09');
    expect(absoluteTimeText(afternoon, { language: 'zh-CN', timeZone: 'UTC' }).text).toBe('2026/11/02 17:05:09');
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

  it('says today with the 24-hour time of an afternoon event, in both languages', () => {
    const afternoon = Date.UTC(2026, 10, 2, 17, 5, 9);
    const now = Date.UTC(2026, 10, 2, 20, 0, 0);
    expect(ageText(afternoon, { language: 'zh-CN', nowUnixMs: now, timeZone: 'UTC' }).text).toBe('今天 17:05');
    expect(ageText(afternoon, { language: 'en-US', nowUnixMs: now, timeZone: 'UTC' }).text).toBe('Today at 17:05');
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
