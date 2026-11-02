import {
  OPERATION_COUNT_TEXT,
  OPERATION_KIND_LABELS,
  OPERATION_VOCABULARY,
  humanText,
  type OperationKind,
} from '../../i18n/human-language';
import type {
  FoldOperationsResult,
  OperationDisclosure,
  OperationEvent,
  OperationStep,
  OperationWordResult,
} from './types';

function zh(language: string): boolean {
  return language.toLowerCase().startsWith('zh');
}

function validTime(value: number | undefined): value is number {
  return typeof value === 'number' && Number.isFinite(value);
}

/** Map one raw operation to human wording without hiding its original name. */
export function operationWord(action: unknown, options: { language: string }): OperationWordResult {
  const rawAction = typeof action === 'string' ? action : null;
  const entry = rawAction == null ? undefined : OPERATION_VOCABULARY[rawAction];
  const kind = entry?.kind ?? 'other';
  const labels = OPERATION_KIND_LABELS[kind];
  return {
    kind,
    text: entry ? entry[zh(options.language) ? 'zh' : 'en'] : labels[zh(options.language) ? 'zh' : 'en'],
    technical: { rawAction },
  };
}

function stepText(kind: OperationKind, count: number, language: string): string {
  const labels = OPERATION_KIND_LABELS[kind];
  const kindText = zh(language) ? labels.zh : count === 1 ? labels.en : labels.enPlural;
  const template = zh(language) ? OPERATION_COUNT_TEXT.zh : OPERATION_COUNT_TEXT.en;
  return template.replace('{{count}}', String(count)).replace('{{kind}}', kindText);
}

/** Sort, disclose and fold audit rows without deleting evidence or inventing object counts. */
export function foldOperations(
  items: readonly OperationEvent[],
  options: {
    language: string;
    gapThresholdMs: number;
    order?: 'oldest-first' | 'newest-first';
  },
): FoldOperationsResult {
  if (!Number.isFinite(options.gapThresholdMs) || options.gapThresholdMs < 0) {
    throw new RangeError('gapThresholdMs must be a non-negative finite number');
  }
  const order = options.order ?? 'newest-first';
  const events = items.map((event, index) => ({ event, index })).sort((left, right) => {
    const leftTime = validTime(left.event.created_at_unix_ms) ? left.event.created_at_unix_ms : null;
    const rightTime = validTime(right.event.created_at_unix_ms) ? right.event.created_at_unix_ms : null;
    if (leftTime != null && rightTime != null) {
      const byTime = order === 'oldest-first' ? leftTime - rightTime : rightTime - leftTime;
      if (byTime !== 0) return byTime;
      // A total order for ties: events with an id first (by id), then by input position.
      const leftId = left.event.id;
      const rightId = right.event.id;
      if (leftId && rightId && leftId !== rightId) return leftId < rightId ? -1 : 1;
      if (Boolean(leftId) !== Boolean(rightId)) return leftId ? -1 : 1;
      return left.index - right.index;
    }
    if (leftTime != null || rightTime != null) return leftTime == null ? 1 : -1;
    return left.index - right.index;
  }).map(({ event }) => event);

  const seen = new Set<string>();
  const duplicateIds: string[] = [];
  const uniqueEvents: OperationEvent[] = [];
  for (const event of events) {
    if (!event.id) {
      uniqueEvents.push(event);
    } else if (seen.has(event.id)) {
      if (!duplicateIds.includes(event.id)) duplicateIds.push(event.id);
    } else {
      seen.add(event.id);
      uniqueEvents.push(event);
    }
  }

  const disclosures: OperationDisclosure[] = [];
  if (duplicateIds.length > 0) {
    disclosures.push({ code: 'duplicate-id', text: humanText('duplicateActivity', options.language), technical: { ids: [...duplicateIds] } });
  }
  if (events.some((event) => !event.id)) {
    disclosures.push({ code: 'missing-id', text: humanText('missingActivityId', options.language), technical: {} });
  }
  if (events.some((event) => !validTime(event.created_at_unix_ms))) {
    disclosures.push({ code: 'missing-time', text: humanText('missingActivityTime', options.language), technical: {} });
  }
  const unmapped = new Set<string | null>();
  for (const event of events) {
    const action = typeof event.action === 'string' ? event.action : null;
    if (action != null && OPERATION_VOCABULARY[action]) continue;
    if (unmapped.has(action)) continue;
    unmapped.add(action);
    disclosures.push({ code: 'unmapped-action', text: humanText('unmappedActivityAction', options.language), technical: { rawAction: action } });
  }

  const steps: OperationStep[] = [];
  let previous: OperationEvent | undefined;
  for (const event of uniqueEvents) {
    const kind = operationWord(event.action, options).kind;
    const eventTime = validTime(event.created_at_unix_ms) ? event.created_at_unix_ms : null;
    const previousTime = previous && validTime(previous.created_at_unix_ms) ? previous.created_at_unix_ms : null;
    const gapMs = eventTime != null && previousTime != null ? Math.abs(eventTime - previousTime) : null;
    const separated = gapMs != null && gapMs > options.gapThresholdMs;
    let step = steps.at(-1);
    if (!step || step.kind !== kind || separated) {
      step = {
        kind,
        text: '',
        count: 0,
        separatorBefore: separated,
        firstAtUnixMs: null,
        lastAtUnixMs: null,
        technical: { rawActions: [], eventIds: [] },
      };
      steps.push(step);
    }
    step.count += 1;
    step.technical.eventIds.push(event.id);
    const rawAction = typeof event.action === 'string' ? event.action : null;
    if (!step.technical.rawActions.includes(rawAction)) step.technical.rawActions.push(rawAction);
    step.firstAtUnixMs = step.firstAtUnixMs == null || (eventTime != null && eventTime < step.firstAtUnixMs) ? eventTime : step.firstAtUnixMs;
    step.lastAtUnixMs = step.lastAtUnixMs == null || (eventTime != null && eventTime > step.lastAtUnixMs) ? eventTime : step.lastAtUnixMs;
    if (separated && gapMs != null) {
      disclosures.push({ code: 'time-gap', text: humanText('activityTimeGap', options.language), technical: { gapMs } });
    }
    previous = event;
  }
  for (const step of steps) step.text = stepText(step.kind, step.count, options.language);
  return {
    events,
    uniqueEvents,
    steps,
    duplicateIds,
    disclosures,
    technical: { gapThresholdMs: options.gapThresholdMs, order },
  };
}
