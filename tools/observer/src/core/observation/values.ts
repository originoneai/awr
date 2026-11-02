import { STATUS_LABELS, humanText } from '../../i18n/human-language';
import type { DescribeUnknownResult, RawObservationTextResult, SemanticTone, StatusWordResult } from './types';

export const STATUS_FIELDS = new Set([
  'state', 'status', 'result', 'runtime_state', 'decision', 'outcome', 'dispatch_state',
  'definition_state', 'selection_basis', 'source_observation', 'trust_basis', 'fact_source',
  'provider', 'independence_kind', 'approval_basis', 'severity', 'role', 'business_roles',
  'relation', 'adoption_version_policies', 'phase',
]);

const POSITIVE = new Set(['approved', 'confirmed', 'committed', 'succeeded', 'completed', 'passed', 'applied', 'consistent', 'exact', 'observed', 'enabled', 'delivered']);
const NEGATIVE = new Set(['rejected', 'invalidated', 'failed', 'blocking', 'blocked']);
const WARNING = new Set(['open', 'pending', 'dispatched', 'claimed', 'active', 'return', 'waiting_user', 'waiting_dependency', 'ready_for_review', 'reviewing', 'reworking']);

function isZh(language: string): boolean {
  return language.toLowerCase().startsWith('zh');
}

function rawText(value: unknown): string | null {
  if (value == null || value === '') return null;
  if (typeof value === 'string') return value;
  if (typeof value === 'number' || typeof value === 'boolean') return String(value);
  return JSON.stringify(value) ?? null;
}

/** Serialize a business scalar or an exact technical value without numeric coercion. */
export function rawObservationText(value: unknown): RawObservationTextResult {
  const raw = rawText(value);
  return { text: raw, technical: { raw } };
}

/** Translate only known values in known status fields; titles are never translated. */
export function statusWord(value: unknown, options: { language: string; field?: string }): StatusWordResult {
  const raw = rawText(value);
  if (raw == null) {
    return { text: humanText('notProvided', options.language), known: false, missing: true, tone: 'neutral', technical: { raw } };
  }
  const field = options.field ?? '';
  const label = STATUS_FIELDS.has(field) ? STATUS_LABELS[raw] : undefined;
  if (label) {
    const text = label[isZh(options.language) ? 0 : 1];
    const tone: SemanticTone = POSITIVE.has(raw) ? 'positive' : NEGATIVE.has(raw) ? 'negative' : WARNING.has(raw) ? 'warning' : 'neutral';
    return { text, known: true, missing: false, tone, technical: { raw } };
  }
  if (STATUS_FIELDS.has(field)) {
    return { text: humanText('unrecognizedStatus', options.language), known: false, missing: false, tone: 'neutral', technical: { raw } };
  }
  return { text: raw, known: false, missing: false, tone: 'neutral', technical: { raw } };
}

/** Keep unknown outcomes explicit without exposing engineering prose by default. */
export function describeUnknown(input: {
  result: unknown;
  unknown_means: unknown;
  language: string;
}): DescribeUnknownResult {
  const result = rawText(input.result);
  const means = rawText(input.unknown_means);
  const isUnknown = result === 'unknown';
  if (!isUnknown) return { isUnknown, text: null, disclosure: null, technical: { result, means } };
  const text = STATUS_LABELS.unknown[isZh(input.language) ? 0 : 1];
  const disclosure = means == null
    ? humanText('unknownWithoutMeans', input.language)
    : means === 'completion_not_recorded; inspect business request before retrying'
      ? humanText('unknownCompletionNotRecorded', input.language)
      : humanText('unknownGeneric', input.language);
  return { isUnknown, text, disclosure, technical: { result, means } };
}
