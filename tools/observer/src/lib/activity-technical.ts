import type { TechnicalDetailItem } from '../components/TechnicalDetails';
import type { OperationEvent } from '../core/observation';

/** Raw values of one audit record, for the collapsed technical details only. */
export function eventTechnicalItems(event: OperationEvent): TechnicalDetailItem[] {
  return [
    { label: 'id', value: event.id, testId: 'activity-event-id' },
    { label: 'actor_id', value: event.actor_id, testId: 'activity-event-actor-id' },
    { label: 'client_id', value: event.client_id },
    { label: 'credential_id', value: event.credential_id },
    { label: 'action', value: event.action },
    { label: 'work_id', value: event.work_id },
    { label: 'result', value: event.result },
    { label: 'created_at_unix_ms', value: event.created_at_unix_ms },
  ];
}

/** The service's own statements about what these records are, kept verbatim for the technical details. */
export function pageTechnicalItems(page: {
  scope?: unknown;
  request_retention?: unknown;
  request_retention_capacity?: unknown;
  development_source?: unknown;
  chat_text_collected?: unknown;
  tool_io_collected?: unknown;
  unknown_means?: unknown;
} | undefined): TechnicalDetailItem[] {
  return [
    { label: 'scope', value: page?.scope },
    { label: 'request_retention', value: page?.request_retention },
    { label: 'request_retention_capacity', value: page?.request_retention_capacity },
    { label: 'development_source', value: page?.development_source },
    { label: 'chat_text_collected', value: page?.chat_text_collected },
    { label: 'tool_io_collected', value: page?.tool_io_collected },
    { label: 'unknown_means', value: page?.unknown_means },
  ];
}
