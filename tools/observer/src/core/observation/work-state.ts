import { humanText } from '../../i18n/human-language';
import { rawObservationText } from './values';
import { absoluteTimeText } from './time';
import type { WorkStateInput, WorkStateResult } from './types';

function finiteTime(value: unknown): number | null {
  return typeof value === 'number' && Number.isFinite(value) ? value : null;
}

/** A reporter's statement is a non-empty sentence; anything else is not shown as one. */
function statementText(value: unknown): string | null {
  if (typeof value !== 'string') return null;
  const text = value.trim();
  return text.length > 0 ? text : null;
}

/**
 * What a person needs to know about one work's current report: the next step its reporter wrote in
 * the checkpoint (never the service's agent-addressed hint or operation name), and when it last
 * reported, flagged when the service itself says that report is stale. Nothing is inferred from a
 * read that did not happen.
 */
export function deriveWorkState(input: WorkStateInput): WorkStateResult {
  const { language, readState, snapshot } = input;
  const guidance = snapshot?.guidance?.action;
  const progress = readState === 'ready' ? snapshot?.observation?.progress : null;
  const reportedNext = readState === 'ready'
    ? statementText(progress?.next_action) ?? statementText(input.workNextStep)
    : readState === 'failed' ? statementText(input.workNextStep) : null;
  const technical = {
    nextAction: rawObservationText(progress?.next_action).text,
    nextStep: readState === 'ready' ? rawObservationText(snapshot?.next_step).text : null,
    guidanceOp: readState === 'ready' ? rawObservationText(guidance?.op).text : null,
    guidanceNote: readState === 'ready' ? rawObservationText(guidance?.note).text : null,
  };
  const next: WorkStateResult['next'] = readState === 'loading'
    ? { kind: 'loading', text: humanText('loading', language), technical }
    : reportedNext != null
      ? { kind: 'reported', text: reportedNext, technical }
      : { kind: 'missing', text: humanText('nextStepMissing', language), technical };

  const reporting = readState === 'ready' ? snapshot?.observation?.reporting : null;
  const at = finiteTime(reporting?.client_reported_at_unix_ms);
  const stale = typeof reporting?.client_stale === 'boolean' ? reporting.client_stale : null;
  let lastReport: WorkStateResult['lastReport'];
  if (readState === 'loading') {
    lastReport = { kind: 'loading', text: humanText('loading', language), timeText: null, staleText: null, atUnixMs: null, stale: null, technical: { atUnixMs: null } };
  } else if (at == null) {
    lastReport = { kind: 'not_measured', text: humanText('notMeasured', language), timeText: null, staleText: null, atUnixMs: null, stale, technical: { atUnixMs: null } };
  } else {
    const timeText = absoluteTimeText(at, { language }).text;
    const staleText = stale === true ? humanText('stale', language) : null;
    const suffix = staleText == null ? '' : language.toLowerCase().startsWith('zh') ? `（${staleText}）` : ` (${staleText})`;
    lastReport = { kind: 'measured', text: `${timeText}${suffix}`, timeText, staleText, atUnixMs: at, stale, technical: { atUnixMs: at } };
  }
  return { next, lastReport };
}
