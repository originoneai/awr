import { humanText } from '../../i18n/human-language';
import type { WorkStateInput } from './types';

/** One thing the service measured that a person must know about, whatever the reporter says. */
export interface TroubleFact {
  kind: 'integration-rejected' | 'execution-failed';
  /** One plain sentence, the words the page already uses. */
  text: string;
  technical: Record<string, unknown>;
}

function record(value: unknown): Record<string, unknown> | null {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
    ? value as Record<string, unknown>
    : null;
}

function text(value: unknown): string | null {
  return typeof value === 'string' && value.length > 0 ? value : null;
}

/**
 * The trouble the service itself recorded about a work, outranking whatever its reporter claims:
 * an integration rejected on the current binding (the run's real rejected case also says the
 * target changed), and a failed last execution (the vocabulary knows only the word `failed`;
 * other terminal words are not guessed). Each fact carries one plain sentence — never the words
 * integration, binding or claim — and its raw values for the technical details. A read that did
 * happen and holds none of these facts says nothing: silence here is the service's own answer.
 */
export function deriveTrouble(input: {
  snapshot: WorkStateInput['snapshot'];
  readState: 'ready' | 'loading' | 'failed';
  options: { language: string };
}): TroubleFact[] {
  if (input.readState !== 'ready') return [];
  const observation = input.snapshot?.observation;
  const facts: TroubleFact[] = [];

  const integration = record(observation?.collaboration?.integration);
  const integrationState = text(integration?.state);
  if (integration != null && integrationState === 'rejected' && integration.current_binding === true) {
    const targetChanged = integration.target_changed === true;
    facts.push({
      kind: 'integration-rejected',
      text: targetChanged
        ? humanText('troubleIntegrationRejectedTargetMoved', input.options.language)
        : humanText('troubleIntegrationRejected', input.options.language),
      technical: {
        'collaboration.integration.state': integrationState,
        'collaboration.integration.current_binding': integration.current_binding,
        'collaboration.integration.target_changed': integration.target_changed ?? null,
        'collaboration.integration.id': integration.id ?? null,
      },
    });
  }

  const execution = record(observation?.execution);
  const executionState = text(execution?.state);
  if (execution != null && executionState === 'failed') {
    facts.push({
      kind: 'execution-failed',
      text: humanText('troubleExecutionFailed', input.options.language),
      technical: {
        'execution.state': executionState,
        'execution.execution_id': execution.execution_id ?? null,
      },
    });
  }

  return facts;
}
