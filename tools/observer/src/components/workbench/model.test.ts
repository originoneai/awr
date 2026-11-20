import { describe, expect, it } from 'vitest';
import { deriveOverview } from '../../core/overview';
import type { DerivedChain, OverviewReviewRound, OverviewTask, ViewId } from '../../core/overview';
import { complete, reviewRound, startedTask } from '../../core/overview/__tests__/factories';
import {
  buildMyWorkModel,
  buildWorkbenchVerdict,
  metricDetailItems,
  orderWorkbenchChains,
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
  it('uses minutes through 59 minutes and hours at exactly 60 minutes', () => {
    expect(reportAge(NOW - 59 * 60_000, NOW)).toEqual({ kind: 'minutes', count: 59 });
    expect(reportAge(NOW - 60 * 60_000, NOW)).toEqual({ kind: 'hours', count: 1 });
  });

  it('uses hours through 23 hours 59 minutes and days at exactly 24 hours', () => {
    expect(reportAge(NOW - (23 * 60 + 59) * 60_000, NOW)).toEqual({ kind: 'hours', count: 23 });
    expect(reportAge(NOW - DAY_MS, NOW)).toEqual({ kind: 'days', count: 1 });
  });

  it('treats a report at the read time as just now and one millisecond later as unknown', () => {
    expect(reportAge(NOW, NOW)).toEqual({ kind: 'justNow', count: 0 });
    expect(reportAge(NOW + 1, NOW)).toEqual({ kind: 'unknown', count: 0 });
  });

  it('floors partial hours instead of rounding them up', () => {
    expect(reportAge(NOW - 119 * 60_000, NOW)).toEqual({ kind: 'hours', count: 1 });
  });

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
  it('keeps an equal week neutral and marks only upward or downward comparisons', () => {
    const noteForCounts = (currentCount: number, previousCount: number) => {
      const base = myWorkInput();
      const template = base.tasks.items?.find((task) => task.id === 'mine-verified')!;
      const tasks = (base.tasks.items ?? []).filter((task) => task.id !== 'mine-verified');
      const reviewRounds = (base.reviewRounds.items ?? []).filter((round) => round.taskId !== 'mine-verified');
      const addVerified = (id: string, verifiedAtUnixMs: number) => {
        tasks.push({ ...template, id, verifiedAtUnixMs });
        reviewRounds.push(reviewRound(id, 'approved', { openedAtUnixMs: verifiedAtUnixMs - 60_000 }));
      };
      for (let index = 0; index < currentCount; index += 1) {
        addVerified(`current-verified-${index}`, NOW - DAY_MS);
      }
      for (let index = 0; index < previousCount; index += 1) {
        addVerified(`previous-verified-${index}`, WEEK_START - DAY_MS);
      }
      const input = { ...base, tasks: complete(tasks), reviewRounds: complete(reviewRounds) };
      return buildMyWorkModel(deriveOverview(input), 'zh-CN', {
        personId: 'person-a', readAtUnixMs: NOW,
      }).metrics[0].note;
    };
    expect(noteForCounts(1, 1)).toEqual({ text: '上周 1 件' });
    expect(noteForCounts(2, 1)).toEqual({ text: '上周 1 件', tone: 'good', trend: 'up' });
    expect(noteForCounts(1, 2)).toEqual({ text: '上周 2 件', tone: 'bad', trend: 'down' });
  });

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

describe('My Work chain order', () => {
  function chain(id: string, impact: number, oldestDays: number, waitingOn: readonly string[]): DerivedChain {
    return {
      id,
      root: {
        kind: 'decision', person: null, externalParty: null, oldestDays, tone: 'negative',
        queue: {
          count: 1, unknownItems: [],
          items: [{ waitingOn: waitingOn.map((personId) => ({ id: personId, displayName: personId })) }],
        },
      },
      impact: { count: impact, items: [], unknownItems: [] },
    } as unknown as DerivedChain;
  }

  it('puts the viewer first, then impact and oldest wait, without mutating the input', () => {
    const input = [
      chain('other-large', 9, 9, ['person-b']),
      chain('mine-small', 2, 8, ['person-a']),
      chain('mine-large-newer', 5, 2, ['person-a']),
      chain('mine-large-older', 5, 7, ['person-a']),
      chain('other-small', 1, 99, ['person-b']),
    ];
    const original = input.map((item) => item.id);
    expect(orderWorkbenchChains(input, 'person-a', 'me').map((item) => item.id)).toEqual([
      'mine-large-older', 'mine-large-newer', 'mine-small', 'other-large', 'other-small',
    ]);
    expect(input.map((item) => item.id)).toEqual(original);
  });

  it.each<ViewId>(['lead', 'department', 'top'])('keeps the %s perspective in its derived order', (perspective) => {
    const input = [chain('other', 8, 8, ['person-b']), chain('mine', 1, 1, ['person-a'])];
    expect(orderWorkbenchChains(input, 'person-a', perspective)).toEqual(input);
  });

  it('applies the viewer priority in the page model instead of changing core derivation', () => {
    const input = myWorkInput({
      reviewRounds: complete([reviewRound('other-review', 'open', { reviewerPersonId: 'person-b' })]),
    });
    const derived = deriveOverview(input);
    expect(derived.chains[0].root.kind).toBe('review');
    const model = buildMyWorkModel(derived, 'zh-CN', { personId: 'person-a' });
    expect(model.chains[0].root.kind).toBe('decision');
    expect(model.chains[0].root.queue.items[0].waitingOn.map((person) => person.id)).toContain('person-a');
    expect(derived.chains[0].root.kind).toBe('review');
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

  it('uses the prototype waiting word and risk tone when facts are pending for the viewer', () => {
    const derived = deriveOverview(myWorkInput());
    const verdict = buildWorkbenchVerdict(derived.views.me.metrics, null, 'zh-CN', NOW);
    expect(verdict.health).toBe('risk');
    expect(verdict.healthLabel).toBe('等你');
    expect(verdict.healthLabel).not.toBe('正常');
    expect(verdict.source).toBe('generated');
    expect(verdict.byline).toBe('按你名下的任务和 Agent 的报告算出 · 刚刚');
    expect(verdict.sentence).toContain('4 件');
  });

  it('keeps no-update wording and tone when nothing is pending for the viewer', () => {
    const input = myWorkInput({
      decisions: complete([]), handoffs: complete([]), approvals: complete([]),
      reviewRounds: complete([reviewRound('other-review', 'open', { reviewerPersonId: 'person-b' })]),
    });
    const derived = deriveOverview(input);
    expect(derived.views.me.metrics[3].count).toBe(0);
    const verdict = buildWorkbenchVerdict(derived.views.me.metrics, null, 'zh-CN', NOW);
    expect(verdict.health).toBe('none');
    expect(verdict.healthLabel).toBe('没有更新');
    expect(verdict.byline).toBe('按你名下的任务和 Agent 的报告算出 · 刚刚');
  });

  it('keeps no-update wording and the fixed unknown sentence when pending is unknown', () => {
    const input = myWorkInput({ approvals: { coverage: 'unread', reason: 'not-read' } });
    const derived = deriveOverview(input);
    const verdict = buildWorkbenchVerdict(derived.views.me.metrics, null, 'en-US', NOW);
    expect(verdict.health).toBe('none');
    expect(verdict.healthLabel).toBe('No update');
    expect(verdict.sentence).toBe('Some totals are not fully read yet; the confirmed details remain below.');
    expect(verdict.byline).toBe('Calculated from your tasks and Agent reports · just now');
  });

  it('uses the prototype source line for every recorded-update perspective', () => {
    const derived = deriveOverview(myWorkInput());
    const readAt = new Date(2026, 10, 19, 12, 0).getTime();
    const update: WorkbenchPeriodUpdate = {
      id: 'update-one', health: 'risk', sentence: 'A synthetic update.', authorName: 'Project Lead',
      writtenAtUnixMs: new Date(2026, 10, 19, 9, 30).getTime(),
    };
    expect(buildWorkbenchVerdict(derived.views.lead.metrics, update, 'zh-CN', readAt, 'lead').byline)
      .toBe('你今天 09:30 在自己的 Agent 里写的 · 第 47 周');
    const earlier = { ...update, writtenAtUnixMs: new Date(2026, 10, 17, 9, 30).getTime() };
    expect(buildWorkbenchVerdict(derived.views.department.metrics, earlier, 'zh-CN', readAt, 'department').byline)
      .toBe('你 11 月 17 日在自己的 Agent 里写的 · 汇总时取最差的那个，不取平均');
    expect(buildWorkbenchVerdict(derived.views.top.metrics, earlier, 'zh-CN', readAt, 'top').byline)
      .toBe('取自各部门的月更新，取最差的那个 · 11 月 17 日');
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
