import type { TechnicalDetailItem } from '../components/TechnicalDetails';
import type { WorkStateResult } from '../core/observation';

/** The reporter's raw next action, the service's agent-addressed hint and guidance, for the collapsed technical details only. */
export function workStateTechnicalItems(state: WorkStateResult): TechnicalDetailItem[] {
  return [
    { label: 'progress.next_action', value: state.next.technical.nextAction, testId: 'technical-next-action' },
    { label: 'next_step', value: state.next.technical.nextStep, testId: 'technical-next-step' },
    { label: 'guidance.op', value: state.next.technical.guidanceOp, testId: 'technical-guidance-op' },
    { label: 'guidance.note', value: state.next.technical.guidanceNote, testId: 'technical-guidance-note' },
    { label: 'client_reported_at_unix_ms', value: state.lastReport.technical.atUnixMs },
    { label: 'client_stale', value: state.lastReport.stale, testId: 'technical-client-stale' },
    { label: 'runtime.state', value: state.lastReport.technical.runtimeState, testId: 'technical-runtime-state' },
    { label: 'progress.phase', value: state.lastReport.technical.phase, testId: 'technical-phase' },
    { label: 'runtime.selected_completion_id', value: state.progress.technical.completionId, testId: 'technical-completion-id' },
  ];
}
