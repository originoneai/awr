import type { TechnicalDetailItem } from '../components/TechnicalDetails';
import type { WorkStateResult } from '../core/observation';

/** The service's agent-addressed guidance and the raw next step, for the collapsed technical details only. */
export function workStateTechnicalItems(state: WorkStateResult): TechnicalDetailItem[] {
  return [
    { label: 'next_step', value: state.next.technical.nextStep, testId: 'technical-next-step' },
    { label: 'guidance.op', value: state.next.technical.guidanceOp, testId: 'technical-guidance-op' },
    { label: 'guidance.note', value: state.next.technical.guidanceNote, testId: 'technical-guidance-note' },
    { label: 'client_reported_at_unix_ms', value: state.lastReport.technical.atUnixMs },
  ];
}
