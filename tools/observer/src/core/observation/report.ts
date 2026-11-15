import { humanText } from '../../i18n/human-language';
import { artifactLabel, checkLabel } from './labels';
import { readableNarrative } from './readable';
import { readsAsName } from './owner';
import { absoluteTimeText } from './time';
import { hasObservationValue, rawObservationText, statementText, statusWord } from './values';
import type { CappedList, ProgressReportArtifact, ProgressReportResult, ProgressReportTest } from './types';

/** How many entries of one list a person is shown. Original reports remain in service history. */
export const REPORT_LIST_LIMIT = 5;

function asRecord(value: unknown): Record<string, unknown> | null {
  return typeof value === 'object' && value !== null && !Array.isArray(value) ? value as Record<string, unknown> : null;
}

function capped<T>(items: T[]): CappedList<T> {
  return { shown: items.slice(0, REPORT_LIST_LIMIT), more: Math.max(0, items.length - REPORT_LIST_LIMIT) };
}

function reportedSentence(value: unknown, language: string): string | null {
  if (!hasObservationValue(value)) return null;
  const text = statementText(value);
  return (text == null ? null : readableNarrative(text, language)) ?? humanText('reportedContentUnavailable', language);
}

function sentences(value: unknown, language: string): string[] {
  const entries = Array.isArray(value) ? value : [value];
  return entries.flatMap((entry) => {
    const text = reportedSentence(entry, language);
    return text == null ? [] : [text];
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
 * moved on after the report was written. Business prose uses the shared narrative projection.
 * Supplied content that cannot be displayed remains distinct from a missing report.
 */
export function describeProgressReport(progress: unknown, options: { language: string; finished?: boolean }): ProgressReportResult {
  const { language } = options;
  const report = asRecord(progress);
  if (report == null) return MISSING;

  const summary = reportedSentence(report.summary, language);
  const completed = capped(sentences(report.completed, language));
  const blockerSentences = sentences(report.blockers, language);
  const blockers = { ...capped(blockerSentences), reported: Array.isArray(report.blockers) || blockerSentences.length > 0 };
  const tests = capped(
    (Array.isArray(report.tests) ? report.tests : []).flatMap((entry, index): ProgressReportTest[] => {
      const test = asRecord(entry);
      if (test == null) return [];
      const word = statusWord(test.outcome, { language, field: 'result' });
      return [{ name: checkLabel(test.name, { language, position: index + 1 }).text, outcome: word.text, tone: word.tone }];
    }),
  );
  const artifacts = capped(
    (Array.isArray(report.artifacts) ? report.artifacts : []).flatMap((entry, index): ProgressReportArtifact[] => {
      const label = artifactLabel(asRecord(entry)?.label, { language, position: index + 1 });
      return label.technical.raw == null ? [] : [{ text: label.text, raw: label.technical.raw }];
    }),
  );
  const hasContent = summary != null || completed.shown.length > 0 || blockers.reported || tests.shown.length > 0 || artifacts.shown.length > 0;
  if (!hasContent) return MISSING;

  const rawName = statementText(asRecord(report.origin)?.actor_name);
  const name = rawName != null && readsAsName(rawName) ? rawName : humanText('reporterUnnamed', language);
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
