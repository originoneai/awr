import { describe, expect, it } from 'vitest';
import type { OverviewInput, OverviewTask } from '../../core/overview';
import {
  NOW,
  baseTask,
  complete,
  makeInput,
  reviewRound,
  startedTask,
  supportedAcceptance,
} from '../../core/overview/__tests__/factories';
import type { TeamOverviewModelResponse } from '../../lib/types';
import {
  deriveProjectListItem,
  projectListFreshness,
  summarizeProjects,
  type ProjectListEntry,
  type ProjectListModel,
  type ProjectPeriodUpdate,
} from './project-list-model';

const RECORDED = Date.parse('2026-11-19T12:00:00+08:00');
const DAY_MS = 86_400_000;

function update(overrides: Partial<ProjectPeriodUpdate> = {}): ProjectPeriodUpdate {
  return {
    id: 'synthetic-update',
    startsOn: '2026-11-16',
    endsOn: '2026-11-22',
    status: 'ok',
    summary: 'A recorded phase summary.',
    recordedAtUnixMs: RECORDED,
    ...overrides,
  };
}

function model(overrides: {
  input?: OverviewInput;
  periodUpdates?: ProjectListModel['periodUpdates'];
  authoritative?: 'measured' | 'stale' | 'not_measured' | 'failed';
} = {}): ProjectListModel {
  const input = overrides.input ?? makeInput({ options: { nowUnixMs: RECORDED, stalledAfterDays: 7 } });
  const authoritative = overrides.authoritative ?? 'measured';
  return {
    ok: true,
    project: 'project-a',
    read_at_unix_ms: RECORDED,
    input,
    coverage: {
      tasks: { status: 'complete', missing: [] },
      dependencies: { status: 'complete', missing: [] },
      decisions: { status: 'complete', missing: [] },
      reviewRounds: { status: 'complete', missing: [] },
      handoffs: { status: 'complete', missing: [] },
      approvals: { status: 'complete', missing: [] },
      members: { status: 'complete', missing: [] },
      activity: { status: 'complete', missing: [] },
    },
    freshness: {
      report_received: { status: 'not_measured' },
      authoritative: { status: authoritative, ...(authoritative === 'measured' || authoritative === 'stale' ? { at_unix_ms: RECORDED } : {}) },
      page_read: { status: 'measured', at_unix_ms: RECORDED },
    },
    ...(overrides.periodUpdates === undefined ? {} : { periodUpdates: overrides.periodUpdates }),
  } satisfies TeamOverviewModelResponse;
}

function entry(overrides: Partial<ProjectListEntry> = {}): ProjectListEntry {
  return {
    key: 'project-a',
    title: 'Project Alpha',
    leadPersonId: 'person-a',
    groupName: 'Group Alpha',
    takeover: null,
    ...overrides,
  };
}

describe('project status from a phase update and program facts', () => {
  it('uses the current recorded risk judgement without deriving risk from waiting tasks', () => {
    const result = deriveProjectListItem(entry(), model({
      periodUpdates: { coverage: 'complete', items: [update({ status: 'risk', summary: 'The lead recorded a risk.' })] },
    }), 'UTC');
    expect(result.health).toBe('risk');
    expect(result.summary).toEqual({ kind: 'update', text: 'The lead recorded a risk.' });
    expect(result.late).toBe(false);
    expect(result.updatedAtUnixMs).toBe(RECORDED);
  });

  it.each(['ok', 'off'] as const)('keeps the recorded %s grade', (status) => {
    expect(deriveProjectListItem(entry(), model({ periodUpdates: { coverage: 'complete', items: [update({ status })] } })).health).toBe(status);
  });

  it('uses only the age fact when the latest update is outside the current local period', () => {
    const result = deriveProjectListItem(entry(), model({
      periodUpdates: {
        coverage: 'complete',
        items: [update({
          startsOn: '2026-11-02', endsOn: '2026-11-08', status: 'ok',
          summary: 'This old judgement must not become the current health.', recordedAtUnixMs: RECORDED - 12 * DAY_MS,
        })],
      },
    }), 'Asia/Shanghai');
    expect(result.health).toBe('none');
    expect(result.summary).toEqual({ kind: 'last-update', days: 12 });
    expect(result.late).toBe(true);
  });

  it('uses the fixed read time and the viewer local date, not the wall clock or a UTC-only week', () => {
    const instant = Date.parse('2026-11-18T16:30:00Z');
    const input = makeInput({ options: { nowUnixMs: instant, stalledAfterDays: 7 } });
    const old = update({ startsOn: '2026-11-16', endsOn: '2026-11-18', recordedAtUnixMs: instant - DAY_MS });
    expect(deriveProjectListItem(entry(), model({ input, periodUpdates: { coverage: 'complete', items: [old] } }), 'Asia/Shanghai').health).toBe('none');
    expect(deriveProjectListItem(entry(), model({ input, periodUpdates: { coverage: 'complete', items: [old] } }), 'UTC').health).toBe('ok');
  });

  it('shows facts only when the read model has no phase-update source', () => {
    const result = deriveProjectListItem(entry(), model());
    expect(result.health).toBe('none');
    expect(result.summary.kind).toBe('facts');
    expect(result.updatedAtUnixMs).toBeNull();
    expect(result.late).toBe(false);
  });

  it('does not turn an unread update slice into a healthy or no-update judgement', () => {
    const result = deriveProjectListItem(entry(), model({ periodUpdates: { coverage: 'unread', reason: 'read-failed' } }));
    expect(result.health).toBe('unknown');
    expect(result.summary.kind).toBe('unknown');
    expect(result.updatedAtUnixMs).toBeNull();
  });

  it('does not invent a health grade for an unrecognised recorded value', () => {
    const result = deriveProjectListItem(entry(), model({
      periodUpdates: { coverage: 'complete', items: [update({ status: 'not-a-health-grade' })] },
    }));
    expect(result.health).toBe('unknown');
    expect(result.summary.kind).toBe('unknown');
  });

  it('requires an explicit takeover fact and its zero accepted count for the takeover completion message', () => {
    const takeover = deriveProjectListItem(entry({
      takeover: { resultId: 'synthetic-result', verifiedTasks: 0, workstreams: 4, questions: 4, recordedAtUnixMs: RECORDED },
    }), undefined);
    expect(takeover.health).toBe('info');
    expect(takeover.showTakeoverCompletion).toBe(true);
    expect(takeover.summary).toEqual({ kind: 'takeover', workstreams: 4, verified: 0, questions: 4 });
    const ordinary = deriveProjectListItem(entry(), model());
    expect(ordinary.health).not.toBe('info');
    expect(ordinary.showTakeoverCompletion).toBe(false);
  });
  it('marks an update late only after more than seven whole days', () => {
    for (const { age, expected } of [{ age: 7, expected: false }, { age: 8, expected: true }, { age: 7.9, expected: false }]) {
      const result = deriveProjectListItem(entry(), model({
        periodUpdates: {
          coverage: 'complete',
          items: [update({
            startsOn: '2026-11-02', endsOn: '2026-11-08', recordedAtUnixMs: RECORDED - age * DAY_MS,
          })],
        },
      }), 'UTC');
      expect(result.late, `update age ${age}`).toBe(expected);
    }
  });

  it('chooses the latest overlapping current update independent of input order', () => {
    const earlier = update({
      id: 'earlier', startsOn: '2026-11-16', endsOn: '2026-11-22', status: 'ok', summary: 'Earlier healthy summary',
    });
    const later = update({
      id: 'later', startsOn: '2026-11-18', endsOn: '2026-11-24', status: 'risk', summary: 'Later risk summary',
    });
    for (const items of [[earlier, later], [later, earlier]]) {
      const result = deriveProjectListItem(entry(), model({ periodUpdates: { coverage: 'complete', items } }), 'UTC');
      expect(result.health).toBe('risk');
      expect(result.summary).toEqual({ kind: 'update', text: 'Later risk summary' });
      expect(result.updateId).toBe('later');
    }
  });

  it('treats a partial phase-update slice as unknown even with a current healthy update', () => {
    const result = deriveProjectListItem(entry(), model({
      periodUpdates: { coverage: 'partial', reason: 'read-failed', items: [update({ status: 'ok' })] },
    }), 'UTC');
    expect(result.health).toBe('unknown');
    expect(result.summary.kind).toBe('unknown');
    expect(result.updatedAtUnixMs).toBeNull();
  });

  it.each([3, null, undefined])('shows the zero-verification takeover state only for an explicit zero, not %s', (verifiedTasks) => {
    const result = deriveProjectListItem(entry({
      takeover: { resultId: 'synthetic-result', verifiedTasks, workstreams: 4, questions: 4 },
    }), undefined);
    expect(result.showTakeoverCompletion).toBe(false);
  });

  it('floors previous update age in whole days', () => {
    for (const { age, expected } of [{ age: 12.6, expected: 12 }, { age: 0.9, expected: 0 }]) {
      const result = deriveProjectListItem(entry(), model({
        periodUpdates: {
          coverage: 'complete',
          items: [update({
            startsOn: '2026-11-02', endsOn: '2026-11-08', recordedAtUnixMs: RECORDED - age * DAY_MS,
          })],
        },
      }), 'UTC');
      expect(result.summary).toEqual({ kind: 'last-update', days: expected });
    }
  });
});

describe('completion and summary count/detail equality', () => {
  it('builds every segment from the same detail array that supplies its count', () => {
    const verified = baseTask('verified', {
      acceptance: supportedAcceptance(1), integrated: true, submittedResultAtUnixMs: NOW,
      report: { completed: true, reportedAtUnixMs: NOW },
    });
    const claimed = baseTask('claimed', { report: { completed: true, reportedAtUnixMs: NOW }, submittedResultAtUnixMs: null });
    const running = startedTask('running', { report: { completed: false, reportedAtUnixMs: NOW } });
    const ready = baseTask('ready');
    const planned = baseTask('planned', { acceptance: [] });
    const tasks: OverviewTask[] = [verified, claimed, running, ready, planned];
    const input = makeInput({
      tasks: complete(tasks),
      reviewRounds: complete([reviewRound('verified', 'approved')]),
      activity: complete([{ id: 'activity', taskId: 'running', atUnixMs: NOW }]),
    });
    const result = deriveProjectListItem(entry(), model({ input }));
    expect(result.completion.counts).toEqual({ verified: 1, reported: 1, inProgress: 1, notStarted: 2 });
    for (const details of [result.completion.verified, result.completion.reported, result.completion.inProgress, result.completion.notStarted, result.completion.all]) {
      expect(details.count).toBe(details.items.length);
    }
    expect(result.completion.all.count).toBe(5);
  });

  it('counts waiting tasks as in progress alongside running tasks', () => {
    const waiting = baseTask('waiting', {
      externalWait: { party: 'Outside Example', what: 'Confirm a synthetic value', sinceUnixMs: NOW - DAY_MS },
    });
    const running = startedTask('running', { report: { completed: false, reportedAtUnixMs: NOW } });
    const input = makeInput({
      tasks: complete([waiting, running]),
      activity: complete([{ id: 'activity-running', taskId: 'running', atUnixMs: NOW }]),
    });
    const result = deriveProjectListItem(entry(), model({ input }));
    expect(result.completion.inProgress.count).toBe(2);
    expect(result.completion.inProgress.count).toBe(result.completion.inProgress.items.length);
    expect(result.completion.inProgress.items.map((task) => task.id)).toEqual(expect.arrayContaining(['waiting', 'running']));
    expect(result.completion.counts?.inProgress).toBe(2);
  });

  it('keeps unread segment counts unknown instead of displaying zero', () => {
    const input = makeInput({ tasks: { coverage: 'unread', reason: 'read-failed' } });
    const result = deriveProjectListItem(entry(), model({ input }));
    expect(result.completion.counts).toBeNull();
    expect(result.completion.all.count).toBeNull();
    expect(result.completion.all.items).toEqual([]);
  });

  it('makes every known summary count equal its filtered details and unknown counts null', () => {
    const risk = deriveProjectListItem(entry(), model({ periodUpdates: { coverage: 'complete', items: [update({ status: 'risk' })] } }));
    const unknown = deriveProjectListItem(entry({ key: 'project-b', takeover: undefined }), model({ periodUpdates: { coverage: 'unread', reason: 'read-failed' } }));
    const summary = summarizeProjects([risk, unknown]);
    expect(summary.all.count).toBe(2);
    expect(summary.all.count).toBe(summary.all.items.length);
    expect(summary.risk.count).toBeNull();
    expect(summary.risk.items).toEqual([risk]);
    expect(summary.noUpdate.count).toBeNull();
    expect(summary.takeover.count).toBeNull();
    const knownZero = summarizeProjects([deriveProjectListItem(entry(), model())]);
    expect(knownZero.risk.count).toBe(0);
    expect(knownZero.risk.items).toEqual([]);
    expect(knownZero.noUpdate.count).toBe(1);
    expect(knownZero.takeover.count).toBe(0);
  });
});

describe('identity, freshness and unknown results', () => {
  it('never guesses the project lead from the viewer scope or an actor identifier', () => {
    const input = makeInput({
      options: { nowUnixMs: RECORDED, stalledAfterDays: 7, views: { me: { personId: 'person-a', projectId: 'project-a' } } },
    });
    const result = deriveProjectListItem(entry({ leadPersonId: undefined }), model({ input }));
    expect(result.leadName).toBeNull();
    expect(result.groupName).toBe('Group Alpha');
  });

  it('resolves a recorded lead only through the member person-id space', () => {
    const result = deriveProjectListItem(entry({ leadPersonId: 'person-b' }), model());
    expect(result.leadName).toBe('Reviewer Two');
  });

  it('retains measured, stale, failed and unmeasured freshness without calling them data health', () => {
    expect(projectListFreshness([model()]).authoritative.status).toBe('measured');
    expect(projectListFreshness([model({ authoritative: 'stale' })]).authoritative.status).toBe('stale');
    expect(projectListFreshness([model({ authoritative: 'failed' })]).authoritative.status).toBe('failed');
    expect(projectListFreshness([model({ authoritative: 'not_measured' })]).authoritative.status).toBe('not_measured');
    expect(projectListFreshness([model(), model({ authoritative: 'not_measured' })]).authoritative.status).toBe('not_measured');
  });
});
