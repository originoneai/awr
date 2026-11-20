import { humanText } from '../../i18n/human-language';
import type { AbsoluteTimeResult, AgeTextResult } from './types';

function validTime(value: number | null | undefined): value is number {
  return typeof value === 'number' && Number.isFinite(value);
}

function absolute(value: number, language: string, timeZone?: string): string {
  return new Intl.DateTimeFormat(language, {
    year: 'numeric', month: '2-digit', day: '2-digit',
    hour: '2-digit', minute: '2-digit', second: '2-digit', hour12: false,
    timeZone,
  }).format(new Date(value));
}

/** Format a factual timestamp; invalid or missing input never falls back to the current clock. */
export function absoluteTimeText(
  unixMs: number | null | undefined,
  options: { language: string; timeZone?: string },
): AbsoluteTimeResult {
  if (!validTime(unixMs)) {
    return { text: humanText('notProvided', options.language), technical: { atUnixMs: null } };
  }
  return { text: absolute(unixMs, options.language, options.timeZone), technical: { atUnixMs: unixMs } };
}

function dayKey(unixMs: number, timeZone: string): string {
  const parts = new Intl.DateTimeFormat('en-US', {
    year: 'numeric', month: '2-digit', day: '2-digit', timeZone,
  }).formatToParts(new Date(unixMs));
  const values = Object.fromEntries(parts.map((part) => [part.type, part.value]));
  return `${values.year}-${values.month}-${values.day}`;
}

function todayTime(unixMs: number, language: string, timeZone: string): string {
  return new Intl.DateTimeFormat(language, {
    hour: '2-digit', minute: '2-digit', hour12: false, timeZone,
  }).format(new Date(unixMs));
}

/** Relative age is limited to the last hour and always uses an explicit reference clock. */
export function ageText(
  unixMs: number | null | undefined,
  options: { language: string; nowUnixMs: number; timeZone: string },
): AgeTextResult {
  const technical = { atUnixMs: validTime(unixMs) ? unixMs : null, referenceUnixMs: options.nowUnixMs, timeZone: options.timeZone };
  if (!validTime(unixMs)) {
    return { text: humanText('notProvided', options.language), atUnixMs: null, referenceUnixMs: options.nowUnixMs, kind: 'missing', technical };
  }
  if (!validTime(options.nowUnixMs)) {
    return { text: absolute(unixMs, options.language, options.timeZone), atUnixMs: unixMs, referenceUnixMs: options.nowUnixMs, kind: 'absolute', technical };
  }
  if (unixMs > options.nowUnixMs) {
    return { text: absolute(unixMs, options.language, options.timeZone), atUnixMs: unixMs, referenceUnixMs: options.nowUnixMs, kind: 'future', technical };
  }
  const elapsed = options.nowUnixMs - unixMs;
  if (elapsed < 60 * 60 * 1000) {
    const minutes = Math.floor(elapsed / 60000);
    const text = elapsed < 60 * 1000
      ? humanText('justNow', options.language)
      : humanText(minutes === 1 ? 'minuteAgo' : 'minutesAgo', options.language).replace('{{count}}', String(minutes));
    return { text, atUnixMs: unixMs, referenceUnixMs: options.nowUnixMs, kind: 'relative', technical };
  }
  if (dayKey(unixMs, options.timeZone) === dayKey(options.nowUnixMs, options.timeZone)) {
    const time = todayTime(unixMs, options.language, options.timeZone);
    return { text: humanText('todayAt', options.language).replace('{{time}}', time), atUnixMs: unixMs, referenceUnixMs: options.nowUnixMs, kind: 'today', technical };
  }
  return { text: absolute(unixMs, options.language, options.timeZone), atUnixMs: unixMs, referenceUnixMs: options.nowUnixMs, kind: 'absolute', technical };
}

interface CompactCalendarParts {
  year: number;
  month: number;
  day: number;
  hour: number;
  minute: number;
}

function compactCalendarParts(unixMs: number, timeZone: string): CompactCalendarParts {
  const parts = new Intl.DateTimeFormat('en-US', {
    year: 'numeric', month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit',
    hourCycle: 'h23', timeZone,
  }).formatToParts(new Date(unixMs));
  const values = Object.fromEntries(parts.map((part) => [part.type, part.value]));
  return {
    year: Number(values.year), month: Number(values.month), day: Number(values.day),
    hour: Number(values.hour), minute: Number(values.minute),
  };
}

function compactDateText(parts: CompactCalendarParts, language: string, includeYear: boolean): string {
  const time = `${String(parts.hour).padStart(2, '0')}:${String(parts.minute).padStart(2, '0')}`;
  if (language.toLowerCase().startsWith('zh')) {
    const date = includeYear
      ? `${parts.year} 年 ${parts.month} 月 ${parts.day} 日`
      : `${parts.month} 月 ${parts.day} 日`;
    return `${date} ${time}`;
  }
  const month = new Intl.DateTimeFormat('en-US', { month: 'short', timeZone: 'UTC' })
    .format(new Date(Date.UTC(parts.year, parts.month - 1, parts.day)));
  return includeYear
    ? `${month} ${parts.day}, ${parts.year}, ${time}`
    : `${month} ${parts.day}, ${time}`;
}

/** Compact ledger time: relative within an hour, then viewer-zone calendar labels without seconds. */
export function compactTimeText(
  unixMs: number | null | undefined,
  options: { language: string; nowUnixMs: number; timeZone: string },
): string {
  const age = ageText(unixMs, options);
  if (age.kind === 'missing' || age.kind === 'relative' || age.kind === 'today') return age.text;
  if (!validTime(unixMs)) return age.text;
  const event = compactCalendarParts(unixMs, options.timeZone);
  if (!validTime(options.nowUnixMs)) return compactDateText(event, options.language, true);
  const current = compactCalendarParts(options.nowUnixMs, options.timeZone);
  const currentDay = Date.UTC(current.year, current.month - 1, current.day);
  const yesterdayKey = new Date(currentDay - 86_400_000).toISOString().slice(0, 10);
  const eventKey = `${event.year}-${String(event.month).padStart(2, '0')}-${String(event.day).padStart(2, '0')}`;
  const time = `${String(event.hour).padStart(2, '0')}:${String(event.minute).padStart(2, '0')}`;
  if (eventKey === yesterdayKey) {
    return options.language.toLowerCase().startsWith('zh') ? `昨天 ${time}` : `Yesterday at ${time}`;
  }
  return compactDateText(event, options.language, event.year !== current.year);
}
