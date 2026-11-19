import { describe, expect, it } from 'vitest';
import { deriveOverview } from '../index';
import type { OverviewApproval, OverviewViewOptions } from '../index';
import { DAY, NOW, approval, complete, decision, makeInput, startedTask } from './factories';

function approvalViews(personId: string, projectIds: string[]): OverviewViewOptions {
  return {
    me: { personId, projectId: projectIds[0] },
    lead: { personId, projectId: projectIds[0] },
    department: { personId, projectIds },
    top: { personId, goalIds: ['goal-one'] },
    currentWeek: { startUnixMs: NOW - 7 * DAY, endUnixMs: NOW },
    previousWeek: { startUnixMs: NOW - 14 * DAY, endUnixMs: NOW - 7 * DAY - 1 },
    currentMonth: { startUnixMs: NOW - 30 * DAY, endUnixMs: NOW },
    previousMonth: { startUnixMs: NOW - 60 * DAY, endUnixMs: NOW - 30 * DAY - 1 },
  };
}

describe('plan approvals and lesson confirmations as pending inputs', () => {
  it('counts both approval kinds beside waits in every pending queue except the top decisions metric', () => {
    const result = deriveOverview(makeInput({
      tasks: complete([startedTask('decided-one', { ownerPersonId: 'person-b', projectId: 'project-one' })]),
      decisions: complete([decision('decided-one', { allowedResponderIds: ['person-a'] })]),
      approvals: complete([
        approval('plan-approval', 'person-a', { projectId: 'project-one' }),
        approval('lesson-confirmation', 'person-a', { projectId: 'project-one' }),
      ]),
      options: { nowUnixMs: NOW, stalledAfterDays: 3, views: approvalViews('person-a', ['project-one']) },
    }));
    const owner = result.people.find((summary) => summary.person.id === 'person-a')!;
    expect(owner.pending.count).toBe(3);
    expect(owner.pending.items.map((item) => item.kind)).toEqual(['wait', 'approval', 'approval']);
    expect(owner.pending.items.filter((item) => item.kind === 'approval')
      .map((item) => item.approval!.kind)).toEqual(['plan-approval', 'lesson-confirmation']);
    expect(result.views.me.metrics[3].count).toBe(3);
    expect(result.views.lead.metrics[3].count).toBe(3);
    expect(result.views.department.metrics[3].count).toBe(3);
    // The top view's fourth metric counts decisions only, never approvals.
    expect(result.views.top.metrics[3].count).toBe(1);
    expect(result.views.top.metrics[3].items).toHaveLength(1);
  });

  it('keeps an approval without a recipient unknown for everyone instead of the viewer\'s own queue', () => {
    const result = deriveOverview(makeInput({
      approvals: complete([approval('plan-approval', null, { projectId: 'project-one', id: 'orphan-approval' })]),
      options: { nowUnixMs: NOW, stalledAfterDays: 3, views: approvalViews('person-a', ['project-one']) },
    }));
    for (const summary of result.people) {
      expect(summary.pending.count, summary.person.id).toBeNull();
      expect(summary.pending.items).toHaveLength(0);
    }
    const mine = result.views.me.metrics[3];
    expect(mine.count).toBeNull();
    expect(mine.count).not.toBe(0);
    expect(mine.items).toHaveLength(0);
    expect(mine.unknownItems.some((issue) => issue.reason === 'approval-recipient-unknown')).toBe(true);
    expect(result.coverage.parts.approvals.state).toBe('complete');
    expect(result.coverage.sections.people.complete).toBe(false);
    expect(result.coverage.sections.views.complete).toBe(false);
  });

  it('keeps pending unknown when the approvals read is incomplete instead of dropping the queue to zero', () => {
    const result = deriveOverview(makeInput({
      approvals: {
        coverage: 'partial',
        items: [approval('lesson-confirmation', 'person-a', { projectId: 'project-one' })],
        reason: 'truncated',
      },
      options: { nowUnixMs: NOW, stalledAfterDays: 3, views: approvalViews('person-a', ['project-one']) },
    }));
    const owner = result.people.find((summary) => summary.person.id === 'person-a')!;
    expect(owner.pending.count).toBeNull();
    expect(owner.pending.items).toHaveLength(1);
    const mine = result.views.me.metrics[3];
    expect(mine.count).toBeNull();
    expect(mine.count).not.toBe(0);
    expect(mine.items).toHaveLength(1);
    expect(result.coverage.parts.approvals.state).toBe('partial');
    expect(result.coverage.parts.approvals.reason).toBe('truncated');
    expect(result.coverage.complete).toBe(false);
    expect(result.coverage.sections.status.complete).toBe(true);
    expect(result.coverage.sections.people.complete).toBe(false);
  });

  it('reports an approval whose kind is not one of the two as a coverage gap, not a pending item', () => {
    const malformed = {
      ...approval('plan-approval', 'person-a', { projectId: 'project-one' }),
      kind: 'mystery',
    } as unknown as OverviewApproval;
    const result = deriveOverview(makeInput({
      approvals: complete([malformed]),
      options: { nowUnixMs: NOW, stalledAfterDays: 3, views: approvalViews('person-a', ['project-one']) },
    }));
    const owner = result.people.find((summary) => summary.person.id === 'person-a')!;
    expect(owner.pending.count).toBeNull();
    expect(owner.pending.items).toHaveLength(0);
    expect(owner.pending.unknownItems.some((issue) => issue.reason === 'approval-kind-unknown')).toBe(true);
    expect(result.coverage.sections.people.complete).toBe(false);
  });

  it('scopes me, lead and department pending queues by the approval\'s own project', () => {
    const result = deriveOverview(makeInput({
      approvals: complete([
        approval('plan-approval', 'person-a', { projectId: 'project-two' }),
        approval('lesson-confirmation', 'person-a', { projectId: 'project-one' }),
      ]),
      options: {
        nowUnixMs: NOW,
        stalledAfterDays: 3,
        views: approvalViews('person-a', ['project-one', 'project-two']),
      },
    }));
    // The person-level summary is project-agnostic and holds both approvals.
    const owner = result.people.find((summary) => summary.person.id === 'person-a')!;
    expect(owner.pending.count).toBe(2);
    // Me and lead see only their own project's approval; the count stays known.
    const mine = result.views.me.metrics[3];
    expect(mine.count).toBe(1);
    expect(mine.items.map((item) => (item as { approval: { kind: string } }).approval.kind))
      .toEqual(['lesson-confirmation']);
    expect(result.views.lead.metrics[3].count).toBe(1);
    // The department view spans both projects and counts both.
    expect(result.views.department.metrics[3].count).toBe(2);
  });

  it('keeps a scoped pending count unknown for an approval without a project instead of dropping it silently', () => {
    const result = deriveOverview(makeInput({
      approvals: complete([approval('plan-approval', 'person-a', { projectId: undefined })]),
      options: { nowUnixMs: NOW, stalledAfterDays: 3, views: approvalViews('person-a', ['project-one']) },
    }));
    // The person-level summary needs no project and stays known.
    const owner = result.people.find((summary) => summary.person.id === 'person-a')!;
    expect(owner.pending.count).toBe(1);
    const mine = result.views.me.metrics[3];
    expect(mine.count).toBeNull();
    expect(mine.count).not.toBe(0);
    expect(mine.items).toHaveLength(0);
    expect(mine.unknownItems.some((issue) => issue.reason === 'view-approval-project-unknown')).toBe(true);
  });
});
