import { humanText } from '../../i18n/human-language';
import { rawObservationText } from './values';
import { absoluteTimeText } from './time';
import type { WorkStateInput, WorkStateResult } from './types';

function finiteTime(value: unknown): number | null {
  return typeof value === 'number' && Number.isFinite(value) ? value : null;
}

/**
 * What a person needs to know about one work's current report: the next step its reporter gave
 * (never the service's agent-addressed operation name), and when it last reported, flagged when the
 * service itself says that report is stale. Nothing is inferred from a read that did not happen.
 */
export function deriveWorkState(input: WorkStateInput): WorkStateResult {
  const { language, readState, snapshot } = input;
  const guidance = snapshot?.guidance?.action;
  const reportedNext = readState === 'ready'
    ? rawObservationText(snapshot?.next_step).text ?? rawObservationText(input.workNextStep).text
    : readState === 'failed' ? rawObservationText(input.workNextStep).text : null;
  const technical = {
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
    lastReport = { kind: 'loading', text: humanText('loading', language), atUnixMs: null, stale: null, technical: { atUnixMs: null } };
  } else if (at == null) {
    lastReport = { kind: 'not_measured', text: humanText('notMeasured', language), atUnixMs: null, stale, technical: { atUnixMs: null } };
  } else {
    const time = absoluteTimeText(at, { language }).text;
    const suffix = stale === true ? (language.toLowerCase().startsWith('zh') ? `（${humanText('stale', language)}）` : ` (${humanText('stale', language)})`) : '';
    lastReport = { kind: 'measured', text: `${time}${suffix}`, atUnixMs: at, stale, technical: { atUnixMs: at } };
  }
  return { next, lastReport };
}
