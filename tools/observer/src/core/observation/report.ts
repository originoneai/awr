import { humanText } from '../../i18n/human-language';
import { checkLabel } from './labels';
import { readableSentence } from './readable';
import { absoluteTimeText } from './time';
import { rawObservationText, statementText, statusWord } from './values';
import type { CappedList, ProgressReportResult, ProgressReportTest } from './types';

/** How many entries of one list a person is shown; every field stays in the technical details. */
export const REPORT_LIST_LIMIT = 5;

function asRecord(value: unknown): Record<string, unknown> | null {
  return typeof value === 'object' && value !== null && !Array.isArray(value) ? value as Record<string, unknown> : null;
}

function capped<T>(items: T[]): CappedList<T> {
  return { shown: items.slice(0, REPORT_LIST_LIMIT), more: Math.max(0, items.length - REPORT_LIST_LIMIT) };
}

function sentences(value: unknown, language: string): string[] {
  if (!Array.isArray(value)) return [];
  return value.flatMap((entry) => {
    const text = statementText(entry);
    return text == null ? [] : [readableSentence(text, language)];
  });
}

function fill(template: string, values: Record<string, string>): string {
  return Object.entries(values).reduce((text, [key, value]) => text.replace(`{{${key}}}`, value), template);
}

const MISSING: ProgressReportResult = {
  kind: 'missing',
  summary: null,
  completed: { shown: [], more: 0 },
  blockers: { shown: [], more: 0, reported: false },
  tests: { shown: [], more: 0 },
  artifacts: { shown: [], more: 0 },
  declaration: [],
  contractNote: null,
  technical: { items: [] },
};

/**
 * The reporter's checkpoint in plain words: its own summary, what is done, what blocks it, the tests
 * with their outcome words, what was produced, who said it and when, and a warning when the task
 * moved on after the report was written. The reporter's sentences are shown verbatim; ids, hashes,
 * references and every raw field go to the technical items.
 */
export function describeProgressReport(progress: unknown, options: { language: string; finished?: boolean }): ProgressReportResult {
  const { language } = options;
  const report = asRecord(progress);
  if (report == null) return MISSING;

  const summaryText = statementText(report.summary);
  const summary = summaryText == null ? null : readableSentence(summaryText, language);
  const completed = capped(sentences(report.completed, language));
  const blockers = { ...capped(sentences(report.blockers, language)), reported: Array.isArray(report.blockers) };
  const tests = capped(
    (Array.isArray(report.tests) ? report.tests : []).flatMap((entry, index): ProgressReportTest[] => {
      const test = asRecord(entry);
      if (test == null) return [];
      const word = statusWord(test.outcome, { language, field: 'result' });
      return [{ name: checkLabel(test.name, { language, position: index + 1 }).text, outcome: word.text, tone: word.tone }];
    }),
  );
  const artifacts = capped(
    (Array.isArray(report.artifacts) ? report.artifacts : []).flatMap((entry): string[] => {
      const label = statementText(asRecord(entry)?.label);
      return label == null ? [] : [label];
    }),
  );
  const hasContent = summary != null || completed.shown.length > 0 || blockers.reported || tests.shown.length > 0 || artifacts.shown.length > 0;
  if (!hasContent) return MISSING;

  const name = statementText(asRecord(report.origin)?.actor_name) ?? humanText('reporterUnnamed', language);
  const at = typeof report.reported_at_unix_ms === 'number' && Number.isFinite(report.reported_at_unix_ms)
    ? absoluteTimeText(report.reported_at_unix_ms, { language }).text
    : null;
  const declaration = [at == null
    ? fill(humanText('reportedBy', language), { name })
    : fill(humanText('reportedByAt', language), { name, time: at })];
  if (report.provenance === 'caller_declared') declaration.push(humanText('declaredNotVerified', language));
  if (report.stale === true && options.finished !== true) declaration.push(humanText('stale', language));

  return {
    kind: 'reported',
    summary,
    completed,
    blockers,
    tests,
    artifacts,
    declaration,
    contractNote: report.contract_matches_current === false ? humanText('contractChanged', language) : null,
    technical: { items: Object.entries(report).map(([key, value]) => ({ label: `progress.${key}`, value: rawObservationText(value).text })) },
  };
}

export function moreLinesText(count: number, language: string): string {
  return fill(humanText('moreLines', language), { count: String(count) });
}
