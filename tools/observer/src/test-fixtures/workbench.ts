import type { OverviewInput } from '../core/overview';
import { DAY, approval, baseTask, complete, decision, dependency, reviewRound, startedTask, supportedAcceptance } from '../core/overview/__tests__/factories';

export const WORKBENCH_NOW = Date.parse('2026-11-19T12:00:00.000Z');
export const WORKBENCH_WEEK_START = Date.parse('2026-11-16T00:00:00.000Z');
export const WORKBENCH_WEEK_END = Date.parse('2026-11-22T23:59:59.999Z');
export const WORKBENCH_PREVIOUS_START = Date.parse('2026-11-09T00:00:00.000Z');
export const WORKBENCH_PREVIOUS_END = Date.parse('2026-11-15T23:59:59.999Z');

/** Public-safe synthetic organization used by the workbench component tests. */
export function myWorkInput(overrides: Partial<OverviewInput> = {}): OverviewInput {
  const mineVerified = startedTask('mine-verified', {
    ownerPersonId: 'person-a', agentName: 'Agent One', agentActive: false,
    verifiedAtUnixMs: WORKBENCH_NOW - DAY, acceptance: supportedAcceptance(1), integrated: true,
    submittedResultAtUnixMs: WORKBENCH_NOW - 2 * DAY,
    report: { completed: true, reportedAtUnixMs: WORKBENCH_NOW - 2 * DAY },
  });
  const mineRunning = startedTask('mine-running', {
    ownerPersonId: 'person-a', agentName: 'Agent One', agentActive: true,
    report: { completed: false, reportedAtUnixMs: WORKBENCH_NOW - 60_000 },
  });
  const mineDecision = baseTask('mine-decision', {
    ownerPersonId: 'person-a', agentName: 'Agent Two',
    report: { completed: false, reportedAtUnixMs: WORKBENCH_NOW - 2 * DAY },
  });
  const mineExternal = baseTask('mine-external', {
    ownerPersonId: 'person-a', agentName: null,
    externalWait: { party: 'Example Client', what: 'Confirm export fields', sinceUnixMs: WORKBENCH_NOW - 5 * DAY },
  });
  const review = startedTask('other-review', {
    ownerPersonId: 'person-c', agentName: 'Agent Three', agentActive: false,
    submittedResultAtUnixMs: WORKBENCH_NOW - DAY,
  });
  const mineDependent = startedTask('mine-dependent', {
    ownerPersonId: 'person-a', agentName: 'Agent One', agentActive: true,
    report: { completed: false, reportedAtUnixMs: WORKBENCH_NOW - 60_000 },
  });
  return {
    tasks: complete([mineVerified, mineRunning, mineDecision, mineExternal, review, mineDependent]
      .map((task) => ({ ...task, projectId: 'project-one' }))),
    dependencies: complete([dependency('mine-dependent', 'other-review', { sinceUnixMs: WORKBENCH_NOW - 2 * DAY })]),
    decisions: complete([decision('mine-decision', { allowedResponderIds: ['person-a'], createdAtUnixMs: WORKBENCH_NOW - 2 * DAY })]),
    reviewRounds: complete([
      reviewRound('mine-verified', 'approved', { openedAtUnixMs: WORKBENCH_NOW - 3 * DAY }),
      reviewRound('other-review', 'open', { reviewerPersonId: 'person-a', openedAtUnixMs: WORKBENCH_NOW - 2 * DAY }),
    ]),
    handoffs: complete([{
      id: 'handoff-one', taskId: 'mine-running', kind: 'execution', status: 'proposed', pending: true,
      fromPersonId: 'person-c', toPersonId: 'person-a', updatedAtUnixMs: WORKBENCH_NOW - DAY,
    }]),
    approvals: complete([approval('plan-approval', 'person-a', { projectId: 'project-one', sinceUnixMs: WORKBENCH_NOW - 2 * DAY })]),
    members: complete([
      { personId: 'person-a', displayName: 'Owner One', groupId: 'group-one' },
      { personId: 'person-b', displayName: 'Reviewer Two', groupId: 'group-one' },
      { personId: 'person-c', displayName: 'Owner Three', groupId: 'group-one' },
    ]),
    activity: complete([
      { id: 'activity-running', taskId: 'mine-running', action: 'session.checkpoint', actorId: 'actor-a', atUnixMs: WORKBENCH_NOW - 60_000 },
      { id: 'activity-dependent', taskId: 'mine-dependent', action: 'session.checkpoint', actorId: 'actor-a', atUnixMs: WORKBENCH_NOW - 60_000 },
    ]),
    options: {
      nowUnixMs: WORKBENCH_NOW,
      stalledAfterDays: 1,
      views: {
        me: { personId: 'person-a', projectId: 'project-one' },
        lead: { personId: 'person-b', projectId: 'project-one' },
        currentWeek: { startUnixMs: WORKBENCH_WEEK_START, endUnixMs: WORKBENCH_WEEK_END },
        previousWeek: { startUnixMs: WORKBENCH_PREVIOUS_START, endUnixMs: WORKBENCH_PREVIOUS_END },
      },
    },
    ...overrides,
  };
}
