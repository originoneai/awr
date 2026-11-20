import { describe, expect, it } from 'vitest';
import { deriveOverview } from '../../core/overview';
import type { OverviewReviewRound, OverviewTask } from '../../core/overview';
import { complete, reviewRound, startedTask } from '../../core/overview/__tests__/factories';
import {
  buildMyWorkModel,
  buildWorkbenchVerdict,
  metricDetailItems,
  reportAge,
  reportDeclaration,
  waitingOnText,
  workbenchInputOptions,
  type WorkbenchPeriodUpdate,
} from './model';
import {
  myWorkInput,
  WORKBENCH_NOW as NOW,
  WORKBENCH_WEEK_START as WEEK_START,
  WORKBENCH_WEEK_END as WEEK_END,
} from '../../test-fixtures/workbench';

const DAY_MS = 86_400_000;

describe('recorded report facts joined by task id', () => {
  it('joins the raw report fact by task id and keeps missing reports explicitly unread', () => {
    const input = myWorkInput();
    const model = buildMyWorkModel(deriveOverview(input), 'zh-CN', {
      personId: 'person-a',
      inputTasks: input.tasks.items,
    });
    const reports = new Map(model.agentTasks.map((task) => [task.id, task.report]));
    expect(reports.get('mine-running')).toEqual({ completed: false, reportedAtUnixMs: NOW - 60_000 });
    expect(reports.get('mine-decision')).toEqual({ completed: false, reportedAtUnixMs: NOW - 2 * DAY_MS });
    expect(reports.get('mine-external')).toBeNull();
  });

  it('leaves every report unread when the raw items are not supplied', () => {
    const model = buildMyWorkModel(deriveOverview(myWorkInput()), 'zh-CN', { personId: 'person-a' });
    expect(model.agentTasks.length).toBeGreaterThan(0);
    expect(model.agentTasks.every((task) => task.report === null)).toBe(true);
  });

  it('keeps reports unread when the raw item list is empty', () => {
    const input = myWorkInput();
    const model = buildMyWorkModel(deriveOverview(input), 'zh-CN', {
      personId: 'person-a',
      inputTasks: [],
    });
    expect(model.agentTasks.length).toBeGreaterThan(0);
    expect(model.agentTasks.every((task) => task.report === null)).toBe(true);
  });
});

describe('report age and declaration', () => {
  it('buckets ages to whole units from the fixed now of the read', () => {
    expect(reportAge(NOW - 30_000, NOW)).toEqual({ kind: 'justNow', count: 0 });
    expect(reportAge(NOW - 60_000, NOW)).toEqual({ kind: 'minutes', count: 1 });
    expect(reportAge(NOW - 59 * 60_000, NOW)).toEqual({ kind: 'minutes', count: 59 });
    expect(reportAge(NOW - 2 * 3_600_000, NOW)).toEqual({ kind: 'hours', count: 2 });
    expect(reportAge(NOW - 2 * DAY_MS, NOW)).toEqual({ kind: 'days', count: 2 });
  });

  it('refuses future times, missing times and a missing now', () => {
    expect(reportAge(NOW + 60_000, NOW)).toEqual({ kind: 'unknown', count: 0 });
    expect(reportAge(null, NOW)).toEqual({ kind: 'unknown', count: 0 });
    expect(reportAge(undefined, NOW)).toEqual({ kind: 'unknown', count: 0 });
    expect(reportAge(NOW, undefined)).toEqual({ kind: 'unknown', count: 0 });
  });

  it('maps only the completion declaration, never the report prose', () => {
    expect(reportDeclaration(null)).toBe('unknown');
    expect(reportDeclaration({ completed: true })).toBe('done');
    expect(reportDeclaration({ completed: false, reportedAtUnixMs: 1 })).toBe('inProgress');
    expect(reportDeclaration({})).toBe('noDeclaration');
  });
});

describe('My Work model', () => {
  it('uses the me view metrics and keeps every count tied to the exact detail array', () => {
    const derived = deriveOverview(myWorkInput());
    const model = buildMyWorkModel(derived, 'zh-CN');
    expect(model.metrics.map((metric) => metric.label)).toEqual(['本周通过核验', 'Agent 在跑', '我在等', '等你处理']);
    // 我在等 counts 2 since the 063d demo-shapes merge: the self-addressed decision wait
    // (owner waiting on their own Agent's answer) is pending for the owner, not a wait on others.
    expect(model.metrics.map((metric) => metric.count)).toEqual([1, 1, 2, 4]);
    expect(model.pending.count).toBe(4);
    expect(derived.views.lead.metrics[3].count).not.toBe(model.pending.count);
    for (const metric of model.metrics) {
      expect(metric.count).toBe(metricDetailItems(metric).length);
      expect(metric.details.count).toBe(metric.details.items.length);
    }
  });

  it('never turns unread approvals into a zero pending count', () => {
    const input = myWorkInput({ approvals: { coverage: 'unread', reason: 'not-read' } });
    const model = buildMyWorkModel(deriveOverview(input), 'zh-CN');
    expect(model.pending.count).toBeNull();
    expect(model.pending.items).toHaveLength(3);
    expect(model.metrics[3].count).toBeNull();
    expect(model.coverage.approvalsUnread).toBe(true);
  });

  it('selects only the three My Work chain kinds and keeps the derived red root', () => {
    const model = buildMyWorkModel(deriveOverview(myWorkInput()), 'zh-CN', { personId: 'person-a' });
    expect(model.chains.length).toBeGreaterThan(0);
    expect(model.chains.every((chain) => ['decision', 'review', 'external'].includes(chain.root.kind))).toBe(true);
    for (const chain of model.chains) {
      expect(chain.nodes.filter((node) => node.root)).toHaveLength(1);
      expect(chain.nodes.find((node) => node.root)?.tone).toBe('negative');
    }
    expect(model.chainTabs.map((tab) => tab.label)).toEqual(expect.arrayContaining(['等你回答', '等复核', '等Example Client']));
  });

  it('names a reviewer only when read and otherwise uses the review-responsibility role, never the task owner', () => {
    const known = deriveOverview(myWorkInput());
    const knownWait = known.waits.find((wait) => wait.kind === 'review')!;
    expect(waitingOnText(knownWait, 'zh-CN')).toBe('Owner One');
    const input = myWorkInput({
      reviewRounds: complete([reviewRound('other-review', 'open', { reviewerPersonId: null, reviewerActorId: null })]),
    });
    const unknown = deriveOverview(input);
    const unknownWait = unknown.waits.find((wait) => wait.kind === 'review')!;
    expect(unknownWait.task.owner?.displayName).toBe('Owner Three');
    expect(waitingOnText(unknownWait, 'zh-CN')).toBe('有复核职责的成员');
    expect(waitingOnText(unknownWait, 'zh-CN')).not.toContain('Owner Three');
  });

  it('opens one detail row per Agent rather than flattening an Agent\'s tasks into a different count', () => {
    const model = buildMyWorkModel(deriveOverview(myWorkInput()), 'zh-CN');
    const metric = model.metrics[1];
    expect(metric.count).toBe(1);
    expect(metricDetailItems(metric)).toHaveLength(1);
    expect(metricDetailItems(metric)[0]).toMatchObject({ agentName: 'Agent One' });
  });
});

describe('fact-based KPI notes', () => {
  it('derives previous, stalled, longest-wait and pending notes from the metric arrays', () => {
    const base = myWorkInput();
    const input = {
      ...base,
      tasks: complete([
        ...(base.tasks.items ?? []),
        startedTask('mine-stalled', {
          ownerPersonId: 'person-a', agentName: 'Stopped Agent', agentActive: false, projectId: 'project-one',
          report: { completed: false, reportedAtUnixMs: NOW - 4 * 86_400_000 },
        }),
      ]),
    };
    const model = buildMyWorkModel(deriveOverview(input), 'zh-CN', { personId: 'person-a', readAtUnixMs: NOW });
    expect(model.metrics.map((metric) => metric.note)).toEqual([
      // Two verified this week against zero last week: the prototype's good tone and up arrow.
      { text: '上周 0 件', tone: 'good', trend: 'up' },
      { text: '另有 1 个停着', tone: 'warn' },
      { text: '最久 5 天，等Example Client' },
      { text: '最久 2 天' },
    ]);
  });

  it('leaves every note blank when a required fact is unknown', () => {
    const base = myWorkInput();
    const input = {
      ...base,
      tasks: complete([
        ...(base.tasks.items ?? []).map((task: OverviewTask) => task.id === 'mine-external' && task.externalWait
          ? { ...task, externalWait: { ...task.externalWait, sinceUnixMs: null } }
          : task),
        startedTask('mine-stalled-unknown', {
          ownerPersonId: 'person-a', agentName: 'Unknown Agent', agentActive: null, projectId: 'project-one',
          report: { completed: false, reportedAtUnixMs: NOW - 4 * 86_400_000 },
        }),
      ]),
      reviewRounds: complete((base.reviewRounds.items ?? []).map((round: OverviewReviewRound) => round.taskId === 'other-review'
        ? { ...round, openedAtUnixMs: null }
        : round)),
      options: {
        ...base.options,
        views: { ...base.options.views, previousWeek: null },
      },
    };
    const model = buildMyWorkModel(deriveOverview(input), 'zh-CN', { personId: 'person-a', readAtUnixMs: NOW });
    expect(model.metrics.map((metric) => metric.note)).toEqual([null, null, null, null]);
  });
});

describe('verdict sources and fixed sentence table', () => {
  it('uses a recorded period update with its health, author and time', () => {
    const derived = deriveOverview(myWorkInput());
    const update: WorkbenchPeriodUpdate = {
      id: 'update-one', health: 'risk', sentence: 'The recorded synthetic update.',
      authorName: 'Project Lead', writtenAtUnixMs: NOW - 60_000,
    };
    const verdict = buildWorkbenchVerdict(derived.views.me.metrics, update, 'zh-CN', NOW);
    expect(verdict.health).toBe('risk');
    expect(verdict.sentence).toBe(update.sentence);
    expect(verdict.source).toBe('recorded-update');
    expect(verdict.byline).toContain('Project Lead');
    expect(verdict.byline).toContain('2026');
    expect(verdict.updateId).toBe('update-one');
  });

  it('never claims a healthy status when there is no period update', () => {
    const derived = deriveOverview(myWorkInput());
    const verdict = buildWorkbenchVerdict(derived.views.me.metrics, null, 'zh-CN', NOW);
    expect(verdict.health).toBe('none');
    expect(verdict.healthLabel).toBe('没有更新');
    expect(verdict.healthLabel).not.toBe('正常');
    expect(verdict.source).toBe('generated');
    expect(verdict.byline).toContain('程序按事实生成');
    expect(verdict.sentence).toContain('4 件');
  });

  it('uses a fixed unknown sentence when pending and waiting totals are unknown', () => {
    const input = myWorkInput({ approvals: { coverage: 'unread', reason: 'not-read' } });
    const derived = deriveOverview(input);
    const verdict = buildWorkbenchVerdict(derived.views.me.metrics, null, 'en-US', NOW);
    expect(verdict.health).toBe('none');
    expect(verdict.sentence).toBe('Some totals are not fully read yet; the confirmed details remain below.');
    expect(verdict.byline).toContain('Generated from recorded facts');
  });
});

describe('caller-owned view options', () => {
  it('fills periods without guessing an unknown member and preserves supplied member identity', () => {
    const unknown = workbenchInputOptions(myWorkInput().options, 'project-one', null);
    expect(unknown.views?.me).toEqual({ personId: null, projectId: 'project-one' });
    expect(unknown.views?.currentWeek).toEqual({ startUnixMs: WEEK_START, endUnixMs: WEEK_END });
    const known = workbenchInputOptions(myWorkInput().options, 'project-one', 'person-a');
    expect(known.views?.me?.personId).toBe('person-a');
    expect(known.views?.lead?.personId).toBe('person-a');
  });

  it.each([
    ['Asia/Shanghai', '2026-11-15T16:00:00.000Z', '2026-11-22T15:59:59.999Z', '2026-10-31T16:00:00.000Z', '2026-11-30T15:59:59.999Z'],
    ['America/New_York', '2026-11-16T05:00:00.000Z', '2026-11-23T04:59:59.999Z', '2026-11-01T04:00:00.000Z', '2026-12-01T04:59:59.999Z'],
  ])('uses %s local midnight and month boundaries across a UTC date change', (timeZone, weekStart, weekEnd, monthStart, monthEnd) => {
    const options = workbenchInputOptions({ nowUnixMs: Date.parse('2026-11-20T01:30:00.000Z'), stalledAfterDays: 1 },
      'project-one', 'person-a', timeZone);
    expect(options.views?.currentWeek).toEqual({ startUnixMs: Date.parse(weekStart), endUnixMs: Date.parse(weekEnd) });
    expect(options.views?.currentMonth).toEqual({ startUnixMs: Date.parse(monthStart), endUnixMs: Date.parse(monthEnd) });
  });

  it('uses the local Monday, not the UTC Monday, at the week boundary', () => {
    const nowUnixMs = Date.parse('2026-11-23T01:30:00.000Z');
    const newYork = workbenchInputOptions({ nowUnixMs, stalledAfterDays: 1 }, 'project-one', 'person-a', 'America/New_York');
    expect(newYork.views?.currentWeek).toEqual({
      startUnixMs: Date.parse('2026-11-16T05:00:00.000Z'), endUnixMs: Date.parse('2026-11-23T04:59:59.999Z'),
    });
    const shanghai = workbenchInputOptions({ nowUnixMs, stalledAfterDays: 1 }, 'project-one', 'person-a', 'Asia/Shanghai');
    expect(shanghai.views?.currentWeek).toEqual({
      startUnixMs: Date.parse('2026-11-22T16:00:00.000Z'), endUnixMs: Date.parse('2026-11-29T15:59:59.999Z'),
    });
  });
});
