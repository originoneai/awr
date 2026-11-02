import { describe, expect, it } from 'vitest';
import DERIVED_RUN09 from '../../../test-fixtures/activity/derived-audit-development-project-scope-all-run09.json';
import LIVE_REQUESTS from '../../../test-fixtures/activity/service-audit-requests-run09.json';
import { findForbiddenVisibleText } from '../../../lib/human-language-guard';
import { OPERATION_KIND_LABELS, OPERATION_VOCABULARY } from '../../../i18n/human-language';
import { foldOperations, operationWord } from '../index';
import type { OperationEvent } from '../index';

const THIRTY_MINUTES = 30 * 60 * 1000;
const DERIVED_ITEMS = DERIVED_RUN09.data.items as OperationEvent[];
const LIVE_ITEMS = LIVE_REQUESTS.data.items as OperationEvent[];

function event(id: string | undefined, action: string | undefined, at: number | undefined, overrides: Partial<OperationEvent> = {}): OperationEvent {
  return {
    id, action, created_at_unix_ms: at,
    actor_id: 'agent-1', client_id: null, credential_id: null, result: 'committed', work_id: 'W1',
    ...overrides,
  };
}

describe('operationWord', () => {
  it('maps real read and development actions to human verbs', () => {
    expect(operationWord('work.snapshot', { language: 'zh-CN' })).toEqual({ kind: 'read', text: '查看工作现状', technical: { rawAction: 'work.snapshot' } });
    expect(operationWord('artifact.segment', { language: 'en-US' }).text).toBe('Read outcome content');
    expect(operationWord('delivery.evidence.submit', { language: 'zh-CN' })).toMatchObject({ kind: 'evidence', text: '提交成果' });
    expect(operationWord('session.checkpoint', { language: 'zh-CN' })).toMatchObject({ kind: 'handoff', text: '保存交接' });
    expect(operationWord('work.recovery', { language: 'zh-CN' })).toMatchObject({ kind: 'recovery', text: '恢复工作' });
  });

  it('uses other operation for an unmapped action and keeps the raw name technical', () => {
    const result = operationWord('invalid.query', { language: 'zh-CN' });
    expect(result).toEqual({ kind: 'other', text: '其他操作', technical: { rawAction: 'invalid.query' } });
    expect(result.text).not.toContain('invalid.query');
    expect(findForbiddenVisibleText(result.text)).toEqual([]);
  });

  it('does not invent an action for a missing value', () => {
    expect(operationWord(undefined, { language: 'en-US' })).toEqual({ kind: 'other', text: 'other operation', technical: { rawAction: null } });
  });
});

describe('operation vocabulary', () => {
  const PROCESS_WORDS = /领取|预留|租约|写入|事实|准入|claim|lease|reserve|ingest|admission|pump/i;

  it('keeps every operation label and count word in plain business words in both languages', () => {
    for (const [name, entry] of Object.entries(OPERATION_VOCABULARY)) {
      for (const text of [entry.zh, entry.en]) {
        expect(findForbiddenVisibleText(text), `${name}: ${text}`).toEqual([]);
        expect(text, name).not.toMatch(PROCESS_WORDS);
      }
    }
    for (const [kind, labels] of Object.entries(OPERATION_KIND_LABELS)) {
      for (const text of [labels.zh, labels.en, labels.enPlural]) {
        expect(findForbiddenVisibleText(text), `${kind}: ${text}`).toEqual([]);
        expect(text, kind).not.toMatch(PROCESS_WORDS);
      }
    }
  });

  it('shows connector and system mechanics as one plain phrase with the raw name kept technical', () => {
    const mechanics = [
      'delivery.source.prepare', 'delivery.source.write', 'delivery.source.confirm', 'delivery.pump.claim', 'delivery.pump.refresh',
      'delivery.facts.ingest', 'delivery.inspection.reserve', 'delivery.integration.inspection.reserve', 'delivery.integration.lease',
      'delivery.integration.dispatch',
    ];
    for (const name of mechanics) {
      expect(operationWord(name, { language: 'zh-CN' }).text, name).toBe('系统同步交付状态');
      expect(operationWord(name, { language: 'en-US' }).text, name).toBe('System syncs delivery status');
      expect(operationWord(name, { language: 'zh-CN' }).technical.rawAction).toBe(name);
    }
  });
});

describe('foldOperations real replies', () => {
  it('retains all 212 derived run-09 events and every unique id', () => {
    // Derived from service source shape and run-09 rows, not a live reply.
    const result = foldOperations(DERIVED_ITEMS, { language: 'zh-CN', gapThresholdMs: THIRTY_MINUTES });
    expect(DERIVED_ITEMS).toHaveLength(212);
    expect(result.events).toHaveLength(212);
    expect(result.uniqueEvents).toHaveLength(212);
    expect(new Set(result.events.map((item) => item.id))).toEqual(new Set(DERIVED_ITEMS.map((item) => item.id)));
    expect(result.duplicateIds).toEqual([]);
    expect(result.steps.flatMap((step) => step.technical.eventIds)).toHaveLength(212);
    expect(result.events[0].id).toBe(DERIVED_ITEMS[0].id);
    expect(result.events.at(-1)?.id).toBe(DERIVED_ITEMS.at(-1)?.id);
    const text = result.steps.map((step) => step.text).join(' · ');
    expect(text).not.toContain('个执行');
    expect(text).not.toContain('份证据');
    expect(findForbiddenVisibleText(text)).toEqual([]);
  });

  it('retains all 50 live request events including unmapped operations', () => {
    const result = foldOperations(LIVE_ITEMS, { language: 'zh-CN', gapThresholdMs: THIRTY_MINUTES });
    expect(result.events).toHaveLength(50);
    expect(new Set(result.events.map((item) => item.id))).toEqual(new Set(LIVE_ITEMS.map((item) => item.id)));
    expect(result.disclosures.some((item) => item.code === 'unmapped-action' && item.technical.rawAction === 'invalid.query')).toBe(true);
    expect(result.steps.map((step) => step.text).join(' ')).toContain('读取');
  });
});

describe('foldOperations boundaries', () => {
  it('stable-sorts out-of-order input by time and id without mutating it', () => {
    const input = [
      event('b', 'work.snapshot', 2000),
      event('a', 'work.snapshot', 2000),
      event('new', 'work.snapshot', 3000),
      event('missing', 'work.snapshot', undefined),
    ];
    const original = [...input];
    const result = foldOperations(input, { language: 'zh-CN', gapThresholdMs: THIRTY_MINUTES });
    expect(result.events.map((item) => item.id)).toEqual(['new', 'a', 'b', 'missing']);
    expect(input).toEqual(original);
    expect(foldOperations(input, { language: 'zh-CN', gapThresholdMs: THIRTY_MINUTES, order: 'oldest-first' }).events.map((item) => item.id)).toEqual(['a', 'b', 'new', 'missing']);
  });

  it('keeps the input order for events that share a time and cannot be told apart by id', () => {
    const at = 5000;
    const marked = (id: string | undefined, mark: string) => event(id, 'work.snapshot', at, { actor_id: mark });
    const options = { language: 'zh-CN', gapThresholdMs: THIRTY_MINUTES };
    const noIds = [marked(undefined, 'first'), marked(undefined, 'second'), marked(undefined, 'third')];
    for (const order of ['newest-first', 'oldest-first'] as const) {
      expect(foldOperations(noIds, { ...options, order }).events.map((item) => item.actor_id)).toEqual(['first', 'second', 'third']);
    }
    const sameId = [marked('dup', 'first'), marked('dup', 'second'), marked('dup', 'third')];
    const folded = foldOperations(sameId, options);
    expect(folded.events.map((item) => item.actor_id)).toEqual(['first', 'second', 'third']);
    expect(folded.uniqueEvents.map((item) => item.actor_id)).toEqual(['first']);
    const mixed = [marked('b', 'b'), marked(undefined, 'no id'), marked('a', 'a')];
    for (const input of [mixed, [...mixed].reverse()]) {
      expect(foldOperations(input, { ...options, order: 'oldest-first' }).events.map((item) => item.actor_id)).toEqual(['a', 'b', 'no id']);
    }
  });

  it('folds consecutive kinds into operation counts, not object counts', () => {
    const result = foldOperations([
      event('r1', 'work.snapshot', 1000),
      event('r2', 'review.inspect', 1001),
      event('r3', 'evidence.inspect', 1002),
      event('x1', 'execution.start', 1003),
    ], { language: 'zh-CN', gapThresholdMs: THIRTY_MINUTES, order: 'oldest-first' });
    expect(result.steps.map((step) => step.text)).toEqual(['3 次读取', '1 次执行操作']);
    expect(result.steps[0].technical.rawActions).toEqual(['work.snapshot', 'review.inspect', 'evidence.inspect']);
    expect(result.steps[0].technical.eventIds).toEqual(['r1', 'r2', 'r3']);
  });

  it('separates only when the gap is greater than the required threshold', () => {
    const input = [event('first', 'work.snapshot', 1000), event('second', 'work.snapshot', 1000 + THIRTY_MINUTES)];
    expect(foldOperations(input, { language: 'zh-CN', gapThresholdMs: THIRTY_MINUTES, order: 'oldest-first' }).steps).toHaveLength(1);
    const later = [input[0], event('second', 'work.snapshot', 1000 + THIRTY_MINUTES + 1)];
    const result = foldOperations(later, { language: 'zh-CN', gapThresholdMs: THIRTY_MINUTES, order: 'oldest-first' });
    expect(result.steps).toHaveLength(2);
    expect(result.steps[1].separatorBefore).toBe(true);
    expect(result.disclosures.some((item) => item.code === 'time-gap' && item.technical.gapMs === THIRTY_MINUTES + 1)).toBe(true);
  });

  it('keeps duplicate events but counts each audit id once and discloses duplicates', () => {
    const result = foldOperations([
      event('dup', 'work.snapshot', 1000),
      event('dup', 'work.snapshot', 1000),
      event('other', 'execution.start', 1001),
    ], { language: 'zh-CN', gapThresholdMs: THIRTY_MINUTES, order: 'oldest-first' });
    expect(result.events).toHaveLength(3);
    expect(result.uniqueEvents).toHaveLength(2);
    expect(result.duplicateIds).toEqual(['dup']);
    expect(result.steps.map((step) => step.count)).toEqual([1, 1]);
    expect(result.disclosures.find((item) => item.code === 'duplicate-id')?.technical.ids).toEqual(['dup']);
  });

  it('keeps missing ids, times and actions without inferred separators or labels', () => {
    const result = foldOperations([
      event(undefined, undefined, undefined),
      event('known', 'work.snapshot', 1000 + THIRTY_MINUTES * 4),
    ], { language: 'zh-CN', gapThresholdMs: THIRTY_MINUTES, order: 'oldest-first' });
    expect(result.events).toHaveLength(2);
    expect(result.uniqueEvents).toHaveLength(2);
    expect(result.steps.every((step) => !step.separatorBefore)).toBe(true);
    expect(result.disclosures.some((item) => item.code === 'missing-id')).toBe(true);
    expect(result.disclosures.some((item) => item.code === 'missing-time')).toBe(true);
    expect(result.disclosures.some((item) => item.code === 'unmapped-action' && item.technical.rawAction === null)).toBe(true);
    expect(result.steps.map((step) => step.text).join(' ')).toContain('其他操作');
  });

  it('requires a finite non-negative threshold and does not invent an empty summary', () => {
    expect(() => foldOperations([], { language: 'zh-CN', gapThresholdMs: Number.NaN })).toThrow(RangeError);
    expect(() => foldOperations([], { language: 'zh-CN', gapThresholdMs: -1 })).toThrow(RangeError);
    expect(foldOperations([], { language: 'zh-CN', gapThresholdMs: THIRTY_MINUTES })).toMatchObject({ events: [], uniqueEvents: [], steps: [], duplicateIds: [] });
  });
});
