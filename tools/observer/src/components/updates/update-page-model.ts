import { compactTimeText, readableNarrative, workLabel } from '../../core/observation';
import { readsAsName } from '../../core/observation/owner';
import { readClockOffsetUnixMs } from '../../lib/display-clock';
import { findForbiddenVisibleText } from '../../lib/human-language-guard';
import type {
  PeriodUpdateFactSnapshot,
  PeriodUpdateRecord,
  PeriodUpdateStatus,
  PeriodUpdateWaitKind,
  PlaneFreshness,
  TeamFreshness,
  TeamPeriodUpdateResponse,
  TeamPeriodUpdatesResponse,
} from '../../lib/types';
import { isoWeekNumber } from '../projects/project-overview-model';

export type UpdateFactKey =
  | 'verified'
  | 'waits_opened'
  | 'waits_cleared'
  | 'review_returned'
  | 'delivered_unverified'
  | 'completed_without_independent_review'
  | 'decisions_made';

export type UpdateFactUnit = 'item' | 'wait';

export interface UpdateFactDetailItem {
  key: string;
  workId: string;
  title: string;
  waitKind?: PeriodUpdateWaitKind;
}

export interface UpdateFactWaitKindGroup {
  kind: PeriodUpdateWaitKind;
  count: number;
  items: UpdateFactDetailItem[];
}

export interface UpdateFactRow {
  key: UpdateFactKey;
  unit: UpdateFactUnit;
  count: number;
  items: UpdateFactDetailItem[];
  waitKinds?: UpdateFactWaitKindGroup[];
}

export type FreshnessSource =
  | { status: 'ok'; freshness: TeamFreshness }
  | { status: 'failed' }
  | { status: 'pending' };

const WAIT_KIND_ORDER: readonly PeriodUpdateWaitKind[] = [
  'review',
  'decision',
  'dependency',
  'external',
  'unknown',
];

const PLANE_KEYS = ['report_received', 'authoritative', 'page_read'] as const;

const ISO_DATE_PATTERN = /^(\d{4})-(\d{2})-(\d{2})$/;
const WEEK_KEY_PATTERN = /^(\d{4})-W(\d{2})$/;
const MONTH_KEY_PATTERN = /^(\d{4})-(\d{2})$/;

export interface IsoDateParts {
  year: number;
  month: number;
  day: number;
}

export function parseIsoDate(value: string | null | undefined): IsoDateParts | null {
  if (typeof value !== 'string') return null;
  const match = ISO_DATE_PATTERN.exec(value);
  if (!match) return null;
  const year = Number(match[1]);
  const month = Number(match[2]);
  const day = Number(match[3]);
  if (month < 1 || month > 12 || day < 1 || day > 31) return null;
  const date = new Date(Date.UTC(year, month - 1, day));
  if (date.getUTCFullYear() !== year || date.getUTCMonth() !== month - 1 || date.getUTCDate() !== day) {
    return null;
  }
  return { year, month, day };
}

export function parsePeriodWeekNumber(
  periodKey: string | null | undefined,
  startsOn: string | null | undefined,
  endsOn: string | null | undefined,
): number | null {
  if (typeof periodKey === 'string') {
    const match = WEEK_KEY_PATTERN.exec(periodKey);
    if (match) {
      const week = Number(match[2]);
      if (week >= 1 && week <= 53) return week;
    }
  }
  return isoWeekNumber(startsOn, endsOn);
}

export function parsePeriodMonth(
  periodKey: string | null | undefined,
  startsOn: string | null | undefined,
): { year: number; month: number } | null {
  if (typeof periodKey === 'string') {
    const match = MONTH_KEY_PATTERN.exec(periodKey);
    if (match) {
      const year = Number(match[1]);
      const month = Number(match[2]);
      if (month >= 1 && month <= 12) return { year, month };
    }
  }
  const start = parseIsoDate(startsOn);
  return start ? { year: start.year, month: start.month } : null;
}

function shortEnglishMonth(year: number, month: number): string {
  return new Intl.DateTimeFormat('en-US', { month: 'short', timeZone: 'UTC' })
    .format(new Date(Date.UTC(year, month - 1, 1)));
}

function longEnglishMonth(year: number, month: number): string {
  return new Intl.DateTimeFormat('en-US', { month: 'long', timeZone: 'UTC' })
    .format(new Date(Date.UTC(year, month - 1, 1)));
}

export function formatPeriodRange(
  startsOn: string | null | undefined,
  endsOn: string | null | undefined,
  language: string,
): string | null {
  const start = parseIsoDate(startsOn);
  const end = parseIsoDate(endsOn);
  if (!start || !end) return null;
  const zh = language.toLowerCase().startsWith('zh');
  if (zh) {
    if (start.year === end.year && start.month === end.month) {
      return `${start.month} 月 ${start.day} 日到 ${end.day} 日`;
    }
    if (start.year === end.year) {
      return `${start.month} 月 ${start.day} 日到 ${end.month} 月 ${end.day} 日`;
    }
    return `${start.year} 年 ${start.month} 月 ${start.day} 日到 ${end.year} 年 ${end.month} 月 ${end.day} 日`;
  }
  const startMonth = shortEnglishMonth(start.year, start.month);
  const endMonth = shortEnglishMonth(end.year, end.month);
  if (start.year === end.year && start.month === end.month) {
    return `${startMonth} ${start.day} – ${end.day}`;
  }
  if (start.year === end.year) {
    return `${startMonth} ${start.day} – ${endMonth} ${end.day}`;
  }
  return `${startMonth} ${start.day}, ${start.year} – ${endMonth} ${end.day}, ${end.year}`;
}

export function formatPeriodLabel(
  update: Pick<PeriodUpdateRecord, 'period_kind' | 'period_key' | 'starts_on' | 'ends_on'>,
  language: string,
  referenceYear?: number | null,
): string | null {
  const zh = language.toLowerCase().startsWith('zh');
  if (update.period_kind === 'week') {
    const week = parsePeriodWeekNumber(update.period_key, update.starts_on, update.ends_on);
    if (week !== null) return zh ? `第 ${week} 周` : `Week ${week}`;
  }
  if (update.period_kind === 'month') {
    const parsed = parsePeriodMonth(update.period_key, update.starts_on);
    if (parsed) {
      const includeYear = referenceYear != null && parsed.year !== referenceYear;
      if (zh) return includeYear ? `${parsed.year} 年 ${parsed.month} 月` : `${parsed.month} 月`;
      const monthName = longEnglishMonth(parsed.year, parsed.month);
      return includeYear ? `${monthName} ${parsed.year}` : monthName;
    }
  }
  return formatPeriodRange(update.starts_on, update.ends_on, language);
}

/**
 * Clean every free-text update sentence through product-language projection so internal
 * identifiers, hashes, diagnostic assignments, and protocol strings never reach the page.
 */
export function sanitizeUpdateSummary(
  summary: string | null | undefined,
  language: string,
  fallback: string,
): string {
  const trimmed = typeof summary === 'string' ? summary.trim() : '';
  if (!trimmed) return fallback;
  const cleaned = readableNarrative(trimmed, language);
  if (!cleaned) return fallback;
  if (findForbiddenVisibleText(cleaned).length > 0) return fallback;
  return cleaned;
}

export function sanitizeAuthorName(displayName: string | null | undefined): string | null {
  const trimmed = typeof displayName === 'string' ? displayName.trim() : '';
  if (!trimmed) return null;
  if (!readsAsName(trimmed)) return null;
  if (findForbiddenVisibleText(trimmed).length > 0) return null;
  return trimmed;
}

export function resolveTaskTitle(
  workId: string,
  workTitleById: ReadonlyMap<string, string>,
  language: string,
  fallback: string,
): string {
  const recorded = workTitleById.get(workId);
  if (!recorded) return fallback;
  const trimmed = recorded.trim();
  if (!trimmed || trimmed === workId) return fallback;
  const label = workLabel(trimmed, { language, recordedTitle: true, fallback });
  if (label.kind !== 'named') return fallback;
  if (findForbiddenVisibleText(label.text).length > 0) return fallback;
  return label.text;
}

function buildWorkIdItems(
  factKey: UpdateFactKey,
  workIds: readonly string[],
  workTitleById: ReadonlyMap<string, string>,
  language: string,
  fallbackTaskTitle: string,
): UpdateFactDetailItem[] {
  return workIds.map((workId, index) => ({
    key: `${factKey}:${workId}:${index}`,
    workId,
    title: resolveTaskTitle(workId, workTitleById, language, fallbackTaskTitle),
  }));
}

function buildWaitRow(
  factKey: 'waits_opened' | 'waits_cleared',
  metric: PeriodUpdateFactSnapshot['waits_opened'],
  workTitleById: ReadonlyMap<string, string>,
  language: string,
  fallbackTaskTitle: string,
): UpdateFactRow {
  const byKind = new Map(metric.kinds.map((entry) => [entry.kind, entry]));
  const waitKinds: UpdateFactWaitKindGroup[] = [];
  const items: UpdateFactDetailItem[] = [];

  for (const kind of WAIT_KIND_ORDER) {
    const entry = byKind.get(kind);
    if (!entry || entry.count === 0) continue;
    const kindItems = entry.work_ids.map((workId, index) => ({
      key: `${factKey}:${kind}:${workId}:${index}`,
      workId,
      title: resolveTaskTitle(workId, workTitleById, language, fallbackTaskTitle),
      waitKind: kind,
    }));
    waitKinds.push({ kind, count: entry.count, items: kindItems });
    items.push(...kindItems);
  }

  return {
    key: factKey,
    unit: 'wait',
    count: metric.count,
    items,
    waitKinds,
  };
}

/**
 * Project a PPD-011 immutable fact snapshot into table rows whose detail arrays strictly
 * preserve the reported count for every row and every wait-kind breakdown.
 * The five prototype core rows always appear in prototype order; non-zero
 * `decisions_made` and `completed_without_independent_review` rows are appended so no
 * recorded fact is ever hidden.
 */
export function buildFactRows(
  snapshot: PeriodUpdateFactSnapshot,
  workTitleById: ReadonlyMap<string, string>,
  language: string,
  fallbackTaskTitle: string,
): UpdateFactRow[] {
  const rows: UpdateFactRow[] = [
    {
      key: 'verified',
      unit: 'item',
      count: snapshot.verified.count,
      items: buildWorkIdItems('verified', snapshot.verified.work_ids, workTitleById, language, fallbackTaskTitle),
    },
    buildWaitRow('waits_opened', snapshot.waits_opened, workTitleById, language, fallbackTaskTitle),
    buildWaitRow('waits_cleared', snapshot.waits_cleared, workTitleById, language, fallbackTaskTitle),
    {
      key: 'review_returned',
      unit: 'item',
      count: snapshot.review_returned.count,
      items: buildWorkIdItems('review_returned', snapshot.review_returned.work_ids, workTitleById, language, fallbackTaskTitle),
    },
    {
      key: 'delivered_unverified',
      unit: 'item',
      count: snapshot.delivered_unverified.count,
      items: buildWorkIdItems('delivered_unverified', snapshot.delivered_unverified.work_ids, workTitleById, language, fallbackTaskTitle),
    },
  ];

  if (snapshot.completed_without_independent_review.count > 0) {
    rows.push({
      key: 'completed_without_independent_review',
      unit: 'item',
      count: snapshot.completed_without_independent_review.count,
      items: buildWorkIdItems(
        'completed_without_independent_review',
        snapshot.completed_without_independent_review.work_ids,
        workTitleById,
        language,
        fallbackTaskTitle,
      ),
    });
  }

  if (snapshot.decisions_made.count > 0) {
    rows.push({
      key: 'decisions_made',
      unit: 'wait',
      count: snapshot.decisions_made.count,
      items: buildWorkIdItems('decisions_made', snapshot.decisions_made.work_ids, workTitleById, language, fallbackTaskTitle),
    });
  }

  return rows;
}

/**
 * Find the previous period's update relative to `currentUpdate` in a descending-ordered list.
 * Skips other versions of the same period and prefers the current version of the earlier period.
 */
export function findPreviousPeriodUpdate(
  items: readonly PeriodUpdateRecord[],
  currentUpdate: Pick<PeriodUpdateRecord, 'update_id' | 'period_key' | 'starts_on' | 'ends_on'>,
): PeriodUpdateRecord | null {
  const earlier = items.filter((item) => {
    if (item.update_id === currentUpdate.update_id) return false;
    if (item.period_key === currentUpdate.period_key) return false;
    if (item.starts_on !== currentUpdate.starts_on) return item.starts_on < currentUpdate.starts_on;
    if (item.ends_on !== currentUpdate.ends_on) return item.ends_on < currentUpdate.ends_on;
    return item.period_key < currentUpdate.period_key;
  });
  if (earlier.length === 0) return null;
  const firstPeriodKey = earlier[0].period_key;
  const samePeriod = earlier.filter((item) => item.period_key === firstPeriodKey);
  return samePeriod.find((item) => item.current) ?? samePeriod[0];
}

/** Deduplicate paginated update pages by update_id while preserving server descending order. */
export function mergeLoadedUpdates(pages: readonly (readonly PeriodUpdateRecord[])[]): PeriodUpdateRecord[] {
  const seen = new Set<string>();
  const merged: PeriodUpdateRecord[] = [];
  for (const page of pages) {
    for (const item of page) {
      if (seen.has(item.update_id)) continue;
      seen.add(item.update_id);
      merged.push(item);
    }
  }
  return merged;
}

/**
 * Derive the display-clock offset from the period update read's own `as_of` and `page_read`
 * timestamps so relative times stay aligned with the read's own clock without reading the browser clock.
 */
export function deriveUpdateClockOffset(
  inspectResponse: TeamPeriodUpdateResponse | undefined,
  listResponse: TeamPeriodUpdatesResponse | undefined,
): number | null {
  for (const response of [inspectResponse, listResponse]) {
    const asOfUnixMs = response?.result.data.as_of.at_unix_ms;
    const pageReadUnixMs = response?.freshness.page_read.at_unix_ms;
    if (typeof asOfUnixMs === 'number' && typeof pageReadUnixMs === 'number') {
      const offset = readClockOffsetUnixMs({
        read_at_unix_ms: pageReadUnixMs,
        input: { options: { nowUnixMs: asOfUnixMs } },
      });
      if (offset !== null) return offset;
    }
  }
  return null;
}

/**
 * Conservative freshness merger across all active read sources on the page:
 * - If any active source failed (or reported a failed plane), the plane reads `failed`.
 * - Otherwise if any source reported `stale`, the plane reads `stale`.
 * - Otherwise `measured` only when every source is ready and reports `measured` for that plane
 *   (or for `report_received`, when at least one source measured it and none failed/stale/pending).
 * - Timestamps always pick the OLDEST confirmed time (`Math.min`), never hiding an older read behind a newer one.
 */
export function mergeConservativeFreshness(sources: readonly FreshnessSource[]): TeamFreshness | undefined {
  if (sources.length === 0) return undefined;

  const mergePlane = (key: (typeof PLANE_KEYS)[number]): PlaneFreshness => {
    if (sources.some((source) => source.status === 'failed')) {
      return { status: 'failed' };
    }
    const okSources = sources.filter((source): source is { status: 'ok'; freshness: TeamFreshness } => source.status === 'ok');
    const planes = okSources.map((source) => source.freshness[key]);

    if (planes.some((plane) => plane?.status === 'failed')) {
      return { status: 'failed' };
    }

    const times = planes
      .filter((plane) => plane?.status === 'measured' || plane?.status === 'stale')
      .map((plane) => plane?.at_unix_ms)
      .filter((value): value is number => typeof value === 'number' && Number.isFinite(value));
    const oldestTime = times.length > 0 ? Math.min(...times) : undefined;

    if (planes.some((plane) => plane?.status === 'stale')) {
      return { status: 'stale', ...(oldestTime !== undefined ? { at_unix_ms: oldestTime } : {}) };
    }

    if (sources.some((source) => source.status === 'pending')) {
      return { status: 'not_measured' };
    }

    if (key === 'report_received') {
      if (planes.some((plane) => plane?.status === 'measured')) {
        return { status: 'measured', ...(oldestTime !== undefined ? { at_unix_ms: oldestTime } : {}) };
      }
      return { status: 'not_measured' };
    }

    if (planes.length > 0 && planes.every((plane) => plane?.status === 'measured')) {
      return { status: 'measured', ...(oldestTime !== undefined ? { at_unix_ms: oldestTime } : {}) };
    }

    return { status: 'not_measured' };
  };

  return {
    report_received: mergePlane('report_received'),
    authoritative: mergePlane('authoritative'),
    page_read: mergePlane('page_read'),
  };
}

export function formatWrittenTime(
  recordedAtUnixMs: number,
  options: { language: string; nowUnixMs: number; timeZone: string },
): string {
  return compactTimeText(recordedAtUnixMs, options);
}

export function hasUnknownWaitKind(snapshot: PeriodUpdateFactSnapshot | null | undefined): boolean {
  if (!snapshot) return false;
  return snapshot.waits_opened.kinds.some((entry) => entry.kind === 'unknown')
    || snapshot.waits_cleared.kinds.some((entry) => entry.kind === 'unknown');
}

export function isValidPeriodStatus(status: string): status is PeriodUpdateStatus {
  return status === 'ok' || status === 'risk' || status === 'off';
}
