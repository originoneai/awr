import { describe, expect, it } from 'vitest';
import {
  buildFactRows,
  deriveUpdateClockOffset,
  findPreviousPeriodUpdate,
  formatPeriodLabel,
  formatPeriodRange,
  hasUnknownWaitKind,
  mergeConservativeFreshness,
  mergeLoadedUpdates,
  parseIsoDate,
  parsePeriodMonth,
  parsePeriodWeekNumber,
  resolveTaskTitle,
  sanitizeAuthorName,
  sanitizeUpdateSummary,
} from './update-page-model';
import type {
  PeriodUpdateFactSnapshot,
  PeriodUpdateRecord,
  TeamFreshness,
  TeamPeriodUpdateResponse,
  TeamPeriodUpdatesResponse,
} from '../../lib/types';

function makeRecord(overrides: Partial<PeriodUpdateRecord> = {}): PeriodUpdateRecord {
  return {
    update_id: 'U-001',
    period_kind: 'week',
    period_key: '2026-W47',
    starts_on: '2026-11-16',
    ends_on: '2026-11-22',
    status: 'risk',
    summary: '接口第 3 版的复核拖了两天，页面联调在等。',
    author: { person_id: 'M-001', display_name: '周宁' },
    recorded_at_unix_ms: 1795080600000,
    version: '1',
    supersedes_update_id: null,
    fact_snapshot_ref: 'F-001',
    current: true,
    ...overrides,
  };
}

function makeFreshness(
  at: number,
  overrides: Partial<TeamFreshness> = {},
): TeamFreshness {
  return {
    report_received: { status: 'not_measured' },
    authoritative: { status: 'measured', at_unix_ms: at },
    page_read: { status: 'measured', at_unix_ms: at },
    ...overrides,
  };
}

describe('parseIsoDate, formatPeriodRange and formatPeriodLabel', () => {
  it('validates calendar dates strictly and rejects impossible dates', () => {
    expect(parseIsoDate('2026-11-16')).toEqual({ year: 2026, month: 11, day: 16 });
    expect(parseIsoDate('2026-02-30')).toBeNull();
    expect(parseIsoDate('2026-13-01')).toBeNull();
    expect(parseIsoDate('not-a-date')).toBeNull();
  });

  it('formats same-month, cross-month and cross-year ranges in zh-CN and en-US', () => {
    expect(formatPeriodRange('2026-11-16', '2026-11-22', 'zh-CN')).toBe('11 月 16 日到 22 日');
    expect(formatPeriodRange('2026-10-26', '2026-11-01', 'zh-CN')).toBe('10 月 26 日到 11 月 1 日');
    expect(formatPeriodRange('2025-12-29', '2026-01-04', 'zh-CN')).toBe('2025 年 12 月 29 日到 2026 年 1 月 4 日');
    expect(formatPeriodRange('2026-11-16', '2026-11-22', 'en-US')).toBe('Nov 16 – 22');
    expect(formatPeriodRange('2026-10-26', '2026-11-01', 'en-US')).toBe('Oct 26 – Nov 1');
    expect(formatPeriodRange('2025-12-29', '2026-01-04', 'en-US')).toBe('Dec 29, 2025 – Jan 4, 2026');
  });

  it('parses week numbers and month labels cleanly', () => {
    expect(parsePeriodWeekNumber('2026-W47', '2026-11-16', '2026-11-22')).toBe(47);
    expect(parsePeriodWeekNumber('invalid', '2026-11-16', '2026-11-22')).toBe(47);
    expect(parsePeriodMonth('2026-11', '2026-11-01')).toEqual({ year: 2026, month: 11 });
    expect(formatPeriodLabel(makeRecord(), 'zh-CN', 2026)).toBe('第 47 周');
    expect(formatPeriodLabel(makeRecord(), 'en-US', 2026)).toBe('Week 47');
    expect(formatPeriodLabel(makeRecord({ period_kind: 'month', period_key: '2026-11', starts_on: '2026-11-01', ends_on: '2026-11-30' }), 'zh-CN', 2026)).toBe('11 月');
    expect(formatPeriodLabel(makeRecord({ period_kind: 'month', period_key: '2025-11', starts_on: '2025-11-01', ends_on: '2025-11-30' }), 'zh-CN', 2026)).toBe('2025 年 11 月');
    expect(formatPeriodLabel(makeRecord({ period_kind: 'month', period_key: '2025-11', starts_on: '2025-11-01', ends_on: '2025-11-30' }), 'en-US', 2026)).toBe('November 2025');
  });
});

describe('product-language sanitization for summaries, authors and task titles', () => {
  it('keeps human prose intact while scrubbing identifiers and rejecting diagnostic payloads', () => {
    expect(sanitizeUpdateSummary('接口第 3 版的复核拖了两天，页面联调在等。', 'zh-CN', '不可显示'))
      .toBe('接口第 3 版的复核拖了两天，页面联调在等。');
    expect(sanitizeUpdateSummary('  The lead wrote this exact sentence.  ', 'en-US', 'Unavailable'))
      .toBe('The lead wrote this exact sentence.');
    expect(sanitizeUpdateSummary('Waiting on 01M7TSKHVDC25RY7GZBC6K0ABK to finish', 'en-US', 'Unavailable'))
      .toBe('Waiting on the related record to finish');
    expect(sanitizeUpdateSummary('{"token":"secret-value"}', 'zh-CN', '不可显示')).toBe('不可显示');
    expect(sanitizeUpdateSummary('actor_id=client-01 status=failed', 'zh-CN', '不可显示')).toBe('不可显示');
    expect(sanitizeUpdateSummary('   ', 'zh-CN', '不可显示')).toBe('不可显示');
  });

  it('rejects opaque identifiers as author names or task titles', () => {
    expect(sanitizeAuthorName('周宁')).toBe('周宁');
    expect(sanitizeAuthorName('Jordan Lee')).toBe('Jordan Lee');
    expect(sanitizeAuthorName('actor-01')).toBeNull();
    expect(sanitizeAuthorName('01M7TSKHVDC25RY7GZBC6K0ABK')).toBeNull();
    expect(sanitizeAuthorName('')).toBeNull();

    const titles = new Map([
      ['T-107', '订单列表分页'],
      ['T-RAW', 'T-RAW'],
      ['T-DIAG', 'payload={"secret":"x"}'],
    ]);
    expect(resolveTaskTitle('T-107', titles, 'zh-CN', '某件任务')).toBe('订单列表分页');
    expect(resolveTaskTitle('T-MISSING', titles, 'zh-CN', '某件任务')).toBe('某件任务');
    expect(resolveTaskTitle('T-RAW', titles, 'zh-CN', '某件任务')).toBe('某件任务');
    expect(resolveTaskTitle('T-DIAG', titles, 'zh-CN', '某件任务')).toBe('某件任务');
  });
});

describe('buildFactRows invariants', () => {
  it('keeps the 5 prototype rows when extra PPD-011 counts are zero and preserves count === items.length', () => {
    const snapshot: PeriodUpdateFactSnapshot = {
      verified: { count: 2, work_ids: ['T-107', 'T-108'] },
      completed_without_independent_review: { count: 0, work_ids: [] },
      waits_opened: {
        count: 6,
        kinds: [
          { kind: 'decision', count: 2, work_ids: ['T-104', 'T-105'] },
          { kind: 'dependency', count: 1, work_ids: ['T-103'] },
          { kind: 'review', count: 3, work_ids: ['T-102', 'T-109', 'T-110'] },
        ],
      },
      waits_cleared: { count: 0, kinds: [] },
      review_returned: { count: 1, work_ids: ['T-105'] },
      delivered_unverified: { count: 1, work_ids: ['T-114'] },
      decisions_made: { count: 0, work_ids: [] },
    };
    const titles = new Map([
      ['T-107', '订单列表分页'],
      ['T-108', '提交按钮防重复'],
      ['T-105', '登录异常处理'],
      ['T-114', '错误提示文案'],
    ]);
    const rows = buildFactRows(snapshot, titles, 'zh-CN', '某件任务');
    expect(rows.map((row) => row.key)).toEqual([
      'verified',
      'waits_opened',
      'waits_cleared',
      'review_returned',
      'delivered_unverified',
    ]);
    for (const row of rows) {
      expect(row.items).toHaveLength(row.count);
      if (row.waitKinds) {
        expect(row.waitKinds.reduce((sum, kind) => sum + kind.count, 0)).toBe(row.count);
        for (const kind of row.waitKinds) {
          expect(kind.items).toHaveLength(kind.count);
        }
      }
    }
    // Wait kinds follow prototype order: review -> decision -> dependency.
    expect(rows[1].waitKinds?.map((group) => group.kind)).toEqual(['review', 'decision', 'dependency']);
  });

  it('appends non-zero decisions_made and completed_without_independent_review without deduplicating duplicate decision work_ids', () => {
    const snapshot: PeriodUpdateFactSnapshot = {
      verified: { count: 0, work_ids: [] },
      completed_without_independent_review: { count: 1, work_ids: ['T-201'] },
      waits_opened: { count: 1, kinds: [{ kind: 'unknown', count: 1, work_ids: ['T-202'] }] },
      waits_cleared: { count: 0, kinds: [] },
      review_returned: { count: 0, work_ids: [] },
      delivered_unverified: { count: 0, work_ids: [] },
      decisions_made: { count: 2, work_ids: ['T-101', 'T-101'] },
    };
    const rows = buildFactRows(snapshot, new Map([['T-101', '订单查询接口']]), 'zh-CN', '某件任务');
    expect(rows.map((row) => row.key)).toEqual([
      'verified',
      'waits_opened',
      'waits_cleared',
      'review_returned',
      'delivered_unverified',
      'completed_without_independent_review',
      'decisions_made',
    ]);
    const decisionsRow = rows.find((row) => row.key === 'decisions_made');
    expect(decisionsRow?.count).toBe(2);
    expect(decisionsRow?.items).toHaveLength(2);
    expect(hasUnknownWaitKind(snapshot)).toBe(true);
  });
});

describe('findPreviousPeriodUpdate and mergeLoadedUpdates', () => {
  it('skips superseded versions of the same period and prefers the current version of the previous period', () => {
    const w47v2 = makeRecord({ update_id: 'U-47-2', period_key: '2026-W47', starts_on: '2026-11-16', ends_on: '2026-11-22', version: '2', current: true });
    const w47v1 = makeRecord({ update_id: 'U-47-1', period_key: '2026-W47', starts_on: '2026-11-15', ends_on: '2026-11-22', version: '1', current: false });
    const w46v1 = makeRecord({ update_id: 'U-46-1', period_key: '2026-W46', starts_on: '2026-11-09', ends_on: '2026-11-15', version: '1', current: false });
    const w46v2 = makeRecord({ update_id: 'U-46-2', period_key: '2026-W46', starts_on: '2026-11-09', ends_on: '2026-11-15', version: '2', current: true });
    const w45 = makeRecord({ update_id: 'U-45-1', period_key: '2026-W45', starts_on: '2026-11-02', ends_on: '2026-11-08', version: '1', current: true });

    expect(findPreviousPeriodUpdate([w47v2, w47v1, w46v1, w46v2, w45], w47v2)?.update_id).toBe('U-46-2');
    expect(findPreviousPeriodUpdate([w47v2, w47v1, w46v1, w46v2, w45], w45)).toBeNull();
  });

  it('deduplicates merged paginated update pages while keeping order', () => {
    const a = makeRecord({ update_id: 'U-1' });
    const b = makeRecord({ update_id: 'U-2', period_key: '2026-W46', starts_on: '2026-11-09', ends_on: '2026-11-15' });
    const c = makeRecord({ update_id: 'U-3', period_key: '2026-W45', starts_on: '2026-11-02', ends_on: '2026-11-08' });
    expect(mergeLoadedUpdates([[a, b], [b, c]]).map((item) => item.update_id)).toEqual(['U-1', 'U-2', 'U-3']);
  });
});

describe('mergeConservativeFreshness and deriveUpdateClockOffset', () => {
  it('marks every plane failed when any active source failed, and uses the oldest timestamp otherwise', () => {
    const failed = mergeConservativeFreshness([
      { status: 'ok', freshness: makeFreshness(2000) },
      { status: 'failed' },
    ]);
    expect(failed?.authoritative.status).toBe('failed');
    expect(failed?.page_read.status).toBe('failed');

    const olderNewer = mergeConservativeFreshness([
      { status: 'ok', freshness: makeFreshness(5000) },
      { status: 'ok', freshness: makeFreshness(1200) },
    ]);
    expect(olderNewer?.authoritative).toEqual({ status: 'measured', at_unix_ms: 1200 });
    expect(olderNewer?.page_read).toEqual({ status: 'measured', at_unix_ms: 1200 });

    const withStale = mergeConservativeFreshness([
      { status: 'ok', freshness: makeFreshness(5000) },
      { status: 'ok', freshness: makeFreshness(1200, { authoritative: { status: 'stale', at_unix_ms: 1200 } }) },
    ]);
    expect(withStale?.authoritative).toEqual({ status: 'stale', at_unix_ms: 1200 });

    const withPending = mergeConservativeFreshness([
      { status: 'ok', freshness: makeFreshness(5000) },
      { status: 'pending' },
    ]);
    expect(withPending?.authoritative.status).toBe('not_measured');
  });

  it('derives the display-clock offset from as_of minus page_read', () => {
    const inspect = {
      ok: true,
      result: { data: { as_of: { at_unix_ms: 1795694400000, source_snapshot_id: 's', project_revision: '1' } } },
      freshness: makeFreshness(1795423914000),
    } as unknown as TeamPeriodUpdateResponse;
    const list = undefined as unknown as TeamPeriodUpdatesResponse | undefined;
    expect(deriveUpdateClockOffset(inspect, list)).toBe(1795694400000 - 1795423914000);
  });
});
