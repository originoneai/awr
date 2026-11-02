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
