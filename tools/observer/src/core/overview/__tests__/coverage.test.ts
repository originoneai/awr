import { describe, expect, it } from 'vitest';
import { deriveOverview } from '../index';
import { NOW, baseTask, complete, makeInput, startedTask } from './factories';

describe('read coverage and unknown counts', () => {
  it('treats a fully read empty collection as known zero', () => {
    const result = deriveOverview(makeInput());
    expect(result.taskCounts.all.count).toBe(0);
    expect(result.waitCounts.all.count).toBe(0);
    expect(result.coverage.complete).toBe(true);
    expect(result.coverage.sections.status.complete).toBe(true);
    expect(result.coverage.sections.waits.complete).toBe(true);
  });

  it('does not treat an unread task list as an empty list, even when a caller retained old rows', () => {
    const result = deriveOverview(makeInput({
      tasks: { coverage: 'unread', reason: 'read-failed', items: [baseTask('stale')] },
    }));
    expect(result.tasks).toEqual([]);
    expect(result.taskCounts.all.count).toBeNull();
    expect(result.taskCounts.all.items).toEqual([]);
    expect(result.waitCounts.all.count).toBeNull();
    expect(result.coverage.parts.tasks.state).toBe('unread');
    expect(result.coverage.sections.status.complete).toBe(false);
    expect(result.coverage.sections.waits.complete).toBe(false);
  });

  it('keeps a decision wait total unknown when decisions were not read', () => {
    const result = deriveOverview(makeInput({
      tasks: complete([startedTask('known')]),
      decisions: { coverage: 'unread', reason: 'permission-restricted' },
    }));
    expect(result.taskCounts.byStatus.unknown.count).toBe(1);
    expect(result.waitCounts.byKind.decision.count).toBeNull();
    expect(result.waitCounts.byKind.decision.count).not.toBe(0);
    expect(result.waitCounts.all.count).toBeNull();
    expect(result.coverage.parts.decisions.reason).toBe('permission-restricted');
  });

  it('keeps known status rows but marks category totals unknown when another row is unknown', () => {
    const result = deriveOverview(makeInput({
      tasks: complete([baseTask('ready'), baseTask('unknown', { started: undefined })]),
    }));
    expect(result.taskCounts.all.count).toBe(2);
    expect(result.taskCounts.byStatus.ready.count).toBeNull();
    expect(result.taskCounts.byStatus.ready.items.map((task) => task.id)).toEqual(['ready']);
    expect(result.taskCounts.byStatus.unknown.count).toBe(1);
    expect(result.taskCounts.byStatus.unknown.items.map((task) => task.id)).toEqual(['unknown']);
  });

  it('marks missing fields unknown rather than zero', () => {
    const missingExternal = deriveOverview(makeInput({
      tasks: complete([{ ...baseTask('missing'), externalWait: undefined }]),
    }));
    expect(missingExternal.tasks[0].status).toBe('unknown');
    expect(missingExternal.waitCounts.byKind.external.count).toBeNull();

    const missingSatisfaction = deriveOverview(makeInput({
      tasks: complete([baseTask('consumer'), baseTask('provider')]),
      dependencies: complete([{ consumerTaskId: 'consumer', providerTaskId: 'provider', sinceUnixMs: NOW - 1 }]),
    }));
    expect(missingSatisfaction.tasks[0].status).toBe('unknown');
    expect(missingSatisfaction.waitCounts.byKind.dependency.count).toBeNull();
  });

  it('reports unused future planes without contaminating an otherwise complete status section', () => {
    const result = deriveOverview(makeInput({
      tasks: complete([baseTask('ready')]),
      handoffs: { coverage: 'unread', reason: 'not-read' },
    }));
    expect(result.coverage.complete).toBe(false);
    expect(result.coverage.parts.handoffs.state).toBe('unread');
    expect(result.coverage.sections.status.complete).toBe(true);
    expect(result.coverage.sections.waits.complete).toBe(true);
  });
});
