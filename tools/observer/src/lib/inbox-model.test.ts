import { describe, expect, it } from 'vitest';
import {
  buildHandback,
  handbackVocabulary,
} from './agent-handback';
import type { TeamOverviewModelResponse } from './types';
import type { OverviewInput, OverviewPart } from '../core/overview';
import {
  DAY,
  NOW,
  approval,
  baseTask,
  complete,
  decision,
  makeInput,
  reviewRound,
} from '../core/overview/__tests__/factories';
import {
  buildInboxModel,
  inboxPendingLabel,
  inboxTaskPath,
  resolveInboxSelection,
} from './inbox-model';

const PARTS: OverviewPart[] = ['tasks', 'dependencies', 'decisions', 'reviewRounds', 'handoffs', 'approvals', 'members', 'activity'];

function model(input: OverviewInput, project = 'project-one'): TeamOverviewModelResponse {
  return {
    ok: true,
    project,
    read_at_unix_ms: NOW,
    input,
    coverage: Object.fromEntries(PARTS.map((part) => [part, {
      status: input[part].coverage,
      ...(input[part].coverage === 'complete' ? {} : { reason: input[part].reason }),
      missing: [],
    }])) as unknown as TeamOverviewModelResponse['coverage'],
    freshness: {
      report_received: { status: 'not_measured' },
      authoritative: { status: 'measured', project_revision: '1', at_unix_ms: NOW },
      page_read: { status: 'measured', at_unix_ms: NOW },
    },
  };
}

function input(overrides: Parameters<typeof makeInput>[0] = {}, project = 'project-one') {
  return makeInput({
    options: {
      nowUnixMs: NOW,
      stalledAfterDays: 3,
      views: { me: { personId: 'person-a', projectId: project } },
    },
    ...overrides,
  });
}

function allKindsInput(project = 'project-one') {
  return input({
    tasks: complete([
      baseTask('answer-task', {
        projectId: project, ownerPersonId: 'person-a', agentName: 'Desk Agent', started: true, agentActive: false,
      }),
      baseTask('other-task', {
        projectId: project, ownerPersonId: 'person-b', agentName: null, started: true, agentActive: false,
      }),
      baseTask('assignment-task', {
        projectId: project, ownerPersonId: 'person-a', agentName: null, started: false, agentActive: false,
        acceptance: [{ evidence: [{ passed: false, current: true }] }],
      }),
      baseTask('review-task', {
        projectId: project, ownerPersonId: 'person-b', started: true, agentActive: false,
        submittedResultAtUnixMs: NOW - DAY,
      }),
      baseTask('outside-task', {
        projectId: project, ownerPersonId: 'person-a', agentName: null,
        externalWait: { party: 'Outside Example', sinceUnixMs: NOW - 8 * DAY },
      }),
    ]),
    decisions: complete([
      decision('answer-task', {
        id: 'answer-record', allowedResponderIds: ['person-a'], createdAtUnixMs: NOW - 2 * DAY,
        question: 'Should the empty state explain the next step?', impact: 'The downstream checks can continue.',
      }),
      decision('other-task', {
        id: 'other-record', allowedResponderIds: ['person-b'], createdAtUnixMs: NOW - 9 * DAY,
        question: 'Should this result be reworked?',
      }),
    ]),
    reviewRounds: complete([
      reviewRound('review-task', 'open', { reviewerPersonId: 'person-a', authorPersonId: 'person-b' }),
    ]),
    handoffs: complete([{
      id: 'assignment-record', taskId: 'assignment-task', kind: 'responsibility', status: 'proposed', pending: true,
      fromPersonId: 'person-b', toPersonId: 'person-a', updatedAtUnixMs: NOW,
    }]),
    approvals: complete([
      approval('lesson-confirmation', 'person-a', {
        id: 'lesson-record', projectId: project, title: 'Check the interface version first', sinceUnixMs: NOW - 6 * DAY,
      }),
      approval('plan-approval', 'person-a', {
        id: 'plan-record', projectId: project, title: 'Recovery work breakdown', sinceUnixMs: NOW - DAY,
      }),
    ]),
  }, project);
}

const projects = [
  { key: 'project-one', title: 'Customer Portal' },
  { key: 'project-two', title: 'Notification Service' },
];

describe('cross-project inbox read model', () => {
  it('keeps only decisions, assignments, plan approvals and lesson confirmations, never chain waits', () => {
    const result = buildInboxModel({ models: [model(allKindsInput())], projects });

    expect(result.viewer).toEqual({ id: 'person-a', displayName: 'Owner One' });
    expect(result.entries.map((entry) => [entry.kind, entry.title])).toEqual([
      ['lesson', 'Check the interface version first'],
      ['answer', 'Should the empty state explain the next step?'],
      ['plan-approval', 'Recovery work breakdown'],
      ['assignment', 'Synthetic assignment-task'],
    ]);
    expect(result.entries.map((entry) => entry.durationDays)).toEqual([6, 2, 1, 0]);
    expect(result.entries.some((entry) => entry.wait?.kind === 'review')).toBe(false);
    expect(result.entries.some((entry) => entry.wait?.kind === 'external')).toBe(false);
    expect(result.entries.some((entry) => entry.wait?.kind === 'dependency')).toBe(false);
    expect(result.count).toBe(result.entries.length);
    expect(result.count).toBe(4);
    expect(result.partial).toBe(false);

    const other = result.allEntries.find((entry) => entry.title === 'Should this result be reworked?');
    expect(other?.mine).toBe(false);
    expect(other?.recipient.displayName).toBe('Reviewer Two');
    expect(result.entries.some((entry) => entry.key === other?.key)).toBe(false);
  });

  it('combines projects without a selected scope and deduplicates approvals repeated in each project reply', () => {
    const first = model(allKindsInput());
    const secondInput = input({
      tasks: complete([baseTask('cross-task', {
        projectId: 'project-two', ownerPersonId: 'person-a', agentName: null, started: true,
      })]),
      decisions: complete([decision('cross-task', {
        id: 'cross-record', allowedResponderIds: ['person-a'], createdAtUnixMs: NOW - 4 * DAY,
        question: 'Should the second project keep the old channel?',
      })]),
      // The demo and the real composed plane can repeat an approval already present in another reply.
      approvals: allKindsInput().approvals,
    }, 'project-two');
    const second = model(secondInput, 'project-two');
    const result = buildInboxModel({ models: [first, second], projects });

    expect(result.entries.filter((entry) => entry.kind === 'lesson')).toHaveLength(1);
    expect(result.entries.filter((entry) => entry.kind === 'plan-approval')).toHaveLength(1);
    expect(result.entries.some((entry) => entry.projectName === 'Notification Service'
      && entry.title === 'Should the second project keep the old channel?')).toBe(true);
    expect(result.entries[0].durationDays).toBe(6);
    expect(result.entries.map((entry) => entry.durationDays).slice(0, 3)).toEqual([6, 4, 2]);
    expect(result.count).toBe(result.entries.length);
  });

  it('uses only the explicit viewer option, never actor ids, owner fields or the wall clock', () => {
    const source = allKindsInput();
    const withoutViewer = structuredClone(source);
    withoutViewer.options.views = {};
    const withDecoyIdentity = Object.assign(model(withoutViewer), {
      identity: { actor_id: 'person-b' },
    });
    const result = buildInboxModel({ models: [withDecoyIdentity], projects });

    expect(result.viewer).toBeNull();
    expect(result.entries).toEqual([]);
    expect(result.count).toBeNull();
    expect(inboxPendingLabel(result)).toBeNull();

    const fixed = buildInboxModel({ models: [model(source)], projects });
    expect(fixed.entries.map((entry) => entry.durationDays)).toEqual([6, 2, 1, 0]);
  });

  it('keeps unknown counts unread, known zero as zero, and partial confirmed counts marked with plus', () => {
    const emptyInput = input();
    const empty = buildInboxModel({ models: [model(emptyInput)], projects });
    expect(empty.count).toBe(0);
    expect(empty.entries).toEqual([]);
    expect(inboxPendingLabel(empty)).toBe('0');

    const partialInput = allKindsInput();
    partialInput.handoffs = { coverage: 'partial', items: [], reason: 'read-failed' };
    const partial = buildInboxModel({ models: [model(partialInput)], projects });
    expect(partial.count).toBe(partial.entries.length);
    expect(partial.count).toBe(3);
    expect(partial.partial).toBe(true);
    expect(inboxPendingLabel(partial)).toBe('3+');

    const unreadInput = input({
      tasks: { coverage: 'unread', reason: 'not-read' },
      decisions: { coverage: 'unread', reason: 'not-read' },
      handoffs: { coverage: 'unread', reason: 'not-read' },
      approvals: { coverage: 'unread', reason: 'not-read' },
    });
    const unread = buildInboxModel({ models: [model(unreadInput)], projects });
    expect(unread.count).toBeNull();
    expect(unread.entries).toEqual([]);
    expect(inboxPendingLabel(unread)).toBeNull();

    const failed = buildInboxModel({ models: [], projects, failedReads: 1 });
    expect(failed.count).toBeNull();
    expect(inboxPendingLabel(failed)).toBeNull();
  });

  it('sorts unknown ages last without coercing them to zero or producing NaN', () => {
    const source = allKindsInput();
    if (source.decisions.coverage === 'complete') {
      const other = source.decisions.items.find((item) => item.id === 'answer-record');
      if (other) other.createdAtUnixMs = null;
    }
    if (source.approvals.coverage === 'complete') {
      const lesson = source.approvals.items.find((item) => item.kind === 'lesson-confirmation');
      if (lesson) lesson.sinceUnixMs = null;
    }
    const result = buildInboxModel({ models: [model(source)], projects });
    const unknown = result.entries.find((entry) => entry.kind === 'answer');
    const unknownLesson = result.entries.find((entry) => entry.kind === 'lesson');
    expect(unknown?.durationDays).toBeNull();
    expect(unknownLesson?.durationDays).toBeNull();
    expect(result.entries.filter((entry) => entry.durationDays == null).map((entry) => entry.key))
      .toEqual(result.entries.slice(-2).map((entry) => entry.key));
    expect(result.entries.every((entry) => entry.durationDays === null || Number.isFinite(entry.durationDays))).toBe(true);
  });

  it('resolves a linked non-mine item but clamps a bogus detail to the first queue entry', () => {
    const result = buildInboxModel({ models: [model(allKindsInput())], projects });
    const other = result.allEntries.find((entry) => !entry.mine)!;
    expect(resolveInboxSelection(result, other.key)?.key).toBe(other.key);
    expect(resolveInboxSelection(result, 'bogus')?.key).toBe(result.entries[0].key);
    expect(resolveInboxSelection(result, '99')?.key).toBe(result.entries[0].key);
    expect(resolveInboxSelection(result, '-1')?.key).toBe(result.entries[0].key);
  });

  it('encodes every task link through one helper and uses the existing handback builder', () => {
    expect(inboxTaskPath('folder/task ?Which one')).toBe('/tasks/folder%2Ftask%20%3FWhich%20one');
    const result = buildInboxModel({ models: [model(allKindsInput())], projects });
    const answer = result.entries.find((entry) => entry.kind === 'answer')!;
    const item = {
      kind: answer.kind,
      projectName: answer.projectName,
      taskName: answer.taskName,
      question: answer.question,
    };
    // Rule 1 names the Agent only when the task's execution Agent is one of the member's
    // connections; the page passes the connections it has actually read.
    const named = buildHandback(item, {
      person: { agentConnections: [{ name: 'Desk Agent', kind: null, lastSeenUnixMs: null }] },
      task: { executionAgentName: answer.executionAgentName },
    }, 'zh-CN');
    expect(named.sentence).toContain('Should the empty state explain the next step?');
    expect(named.agent.name).toBe('Desk Agent');
    expect(named.where).toBe('在你的「Desk Agent」里说这一句');
    const unnamed = buildHandback(item, {
      person: { agentConnections: [] },
      task: { executionAgentName: answer.executionAgentName },
    }, 'zh-CN');
    expect(unnamed.agent.name).toBeNull();
    expect(unnamed.where).toBe('在你的任意一个 Agent 里说一句');
    expect(handbackVocabulary('en-US').kind[answer.kind]).toBe('Answer a question');
  });
});
