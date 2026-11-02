import { humanText } from '../../i18n/human-language';
import { rawObservationText, statementText, statusWord } from './values';
import { abbreviateIds } from './readable';
import { absoluteTimeText } from './time';
import type { WorkStateInput, WorkStateResult } from './types';

function finiteTime(value: unknown): number | null {
  return typeof value === 'number' && Number.isFinite(value) ? value : null;
}

/**
 * What a person needs to know about one work's current report: the next step its reporter wrote in
 * the checkpoint (never the service's agent-addressed hint or operation name), and when it last
 * reported, flagged when the service itself says that report is stale and the work is still open (a
 * finished work's reporter is no longer expected to report, so the clock alone says nothing about
 * it). Nothing is inferred from a read that did not happen.
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
    ? { kind: 'loading', text: humanText('loading', language), fullText: null, technical }
    : reportedNext != null
      ? { kind: 'reported', text: abbreviateIds(reportedNext), fullText: reportedNext, technical }
      : { kind: 'missing', text: humanText('nextStepMissing', language), fullText: null, technical };

  const reporting = readState === 'ready' ? snapshot?.observation?.reporting : null;
  const at = finiteTime(reporting?.client_reported_at_unix_ms);
  const stale = typeof reporting?.client_stale === 'boolean' ? reporting.client_stale : null;
  const runtimeState = readState === 'ready' ? rawObservationText(snapshot?.runtime?.state ?? snapshot?.observation?.runtime?.state).text : null;
  const phase = rawObservationText(progress?.phase).text;
  const finished = runtimeState === 'completed' || phase === 'delivered';
  const reportTechnical = { runtimeState, phase };
  const progressWord = deriveProgress({ language, readState, phase, runtimeState, rowStatus: rawObservationText(input.workStatus).text });
  let lastReport: WorkStateResult['lastReport'];
  if (readState === 'loading') {
    lastReport = { kind: 'loading', text: humanText('loading', language), timeText: null, staleText: null, atUnixMs: null, stale: null, finished: false, technical: { atUnixMs: null, ...reportTechnical } };
  } else if (at == null) {
    lastReport = { kind: 'not_measured', text: humanText('notMeasured', language), timeText: null, staleText: null, atUnixMs: null, stale, finished, technical: { atUnixMs: null, ...reportTechnical } };
  } else {
    const timeText = absoluteTimeText(at, { language }).text;
    const staleText = stale === true && !finished ? humanText('stale', language) : null;
    const suffix = staleText == null ? '' : language.toLowerCase().startsWith('zh') ? `（${staleText}）` : ` (${staleText})`;
    lastReport = { kind: 'measured', text: `${timeText}${suffix}`, timeText, staleText, atUnixMs: at, stale, finished, technical: { atUnixMs: at, ...reportTechnical } };
  }
  return { progress: progressWord, next, lastReport };
}

/**
 * One word for how far a work has come: the reporter's own phase, else the runtime state, else the
 * overview row's status; a word the vocabulary does not know is said to be unrecognized, never
 * guessed, and a read that did not happen contributes nothing.
 */
function deriveProgress(input: {
  language: string;
  readState: WorkStateInput['readState'];
  phase: string | null;
  runtimeState: string | null;
  rowStatus: string | null;
}): WorkStateResult['progress'] {
  const { language, readState, phase, runtimeState, rowStatus } = input;
  if (readState === 'loading') {
    return { kind: 'loading', text: humanText('loading', language), tone: 'neutral', technical: { phase: null, runtimeState: null, rowStatus: null } };
  }
  const technical = { phase, runtimeState, rowStatus };
  const candidates = [
    phase == null ? null : { kind: 'phase' as const, word: statusWord(phase, { language, field: 'phase' }) },
    runtimeState == null ? null : { kind: 'runtime' as const, word: statusWord(runtimeState, { language, field: 'runtime_state' }) },
    rowStatus == null ? null : { kind: 'row' as const, word: statusWord(rowStatus, { language, field: 'state' }) },
  ].filter((candidate) => candidate != null);
  const chosen = candidates.find((candidate) => candidate.word.known) ?? candidates[0];
  if (chosen == null) {
    const unknown = statusWord('unknown', { language, field: 'state' });
    return { kind: 'unknown', text: unknown.text, tone: unknown.tone, technical };
  }
  return { kind: chosen.kind, text: chosen.word.text, tone: chosen.word.tone, technical };
}
