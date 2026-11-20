import type {
  CountDetails,
  DerivedChain,
  DerivedTask,
  DerivedWait,
  OverviewDerivation,
  OverviewDerivationOptions,
  OverviewPart,
  OverviewViewSummary,
  PersonPendingItem,
  PersonSummary,
  ViewMetric,
  ViewMetricKey,
} from '../../core/overview';
import { truncateCodePoints } from '../../lib/agent-handback';
import { formatUnixMs } from '../../lib/format';
import type { HealthStatus } from '../display';

export type VerdictSentenceKey = 'pendingAndWaiting' | 'pendingOnly' | 'waitingOnly' | 'agentsOnly' | 'calm' | 'unknown';

export interface VerdictRule {
  id: string;
  selection: string;
  sentenceKey: VerdictSentenceKey;
}

/** The documentation table is checked row-for-row against this ordered implementation table. */
export const VERDICT_RULES: readonly VerdictRule[] = [
  { id: 'generated-unknown', selection: 'pending === null || waiting === null || agents === null', sentenceKey: 'unknown' },
  { id: 'generated-pending-and-waiting', selection: 'pending > 0 && waiting > 0', sentenceKey: 'pendingAndWaiting' },
  { id: 'generated-pending', selection: 'pending > 0', sentenceKey: 'pendingOnly' },
  { id: 'generated-waiting', selection: 'waiting > 0', sentenceKey: 'waitingOnly' },
  { id: 'generated-agents', selection: 'agents > 0', sentenceKey: 'agentsOnly' },
  { id: 'generated-calm', selection: 'true', sentenceKey: 'calm' },
];

export type KpiNoteSentenceKey = 'previousWeek' | 'stalledOther' | 'waitingOldest' | 'pendingOldest';

export interface KpiNoteRule {
  id: string;
  selection: string;
  sentenceKey: KpiNoteSentenceKey;
}

/** Documentation and tests pin these fact-producing rules beside the verdict sentence table. */
export const KPI_NOTE_RULES: readonly KpiNoteRule[] = [
  { id: 'verified-previous', selection: 'previousPeriod.count !== null', sentenceKey: 'previousWeek' },
  { id: 'agents-stalled', selection: 'stalledOwned > 0 && all activity known', sentenceKey: 'stalledOther' },
  { id: 'waiting-oldest', selection: 'every waiting.durationDays !== null', sentenceKey: 'waitingOldest' },
  { id: 'pending-oldest', selection: 'every pending duration is known at read time', sentenceKey: 'pendingOldest' },
];

export interface WorkbenchPeriodUpdate {
  id: string | null;
  health: HealthStatus;
  sentence: string;
  authorName: string | null;
  writtenAtUnixMs: number;
}

interface WorkbenchVocabulary {
  health: Record<HealthStatus, string>;
  units: { tasks: string; agents: string };
  metrics: {
    verified: string; agents: string; waiting: string; pending: string;
    previousWeek: string; stalledOther: string; waitingOldest: string; pendingOldest: string;
  };
  verdict: {
    sentences: Record<VerdictSentenceKey, string>;
    generatedBy: string; generatedByNoTime: string; updateBy: string; updateAuthorFallback: string;
  };
  chains: { answer: string; decision: string; review: string; outside: string; outsideFallback: string };
  waiting: {
    reviewerFallback: string; dependencyFallback: string; decisionFallback: string; outsideFallback: string; separator: string;
  };
}

const VOCABULARY: Record<'zh-CN' | 'en-US', WorkbenchVocabulary> = {
  'zh-CN': {
    health: { ok: '正常', risk: '有风险', off: '偏离', none: '没有更新', info: '刚接管' },
    units: { tasks: '件', agents: '个' },
    metrics: {
      verified: '本周通过核验', agents: 'Agent 在跑', waiting: '我在等', pending: '等你处理',
      previousWeek: '上周 {{count}} 件', stalledOther: '另有 {{count}} 个停着',
      waitingOldest: '最久 {{days}} 天，等{{party}}', pendingOldest: '最久 {{days}} 天',
    },
    verdict: {
      sentences: {
        pendingAndWaiting: '有 {{pending}} 件事等你处理；你名下的 {{waiting}} 件事正在等别人。',
        pendingOnly: '有 {{pending}} 件事等你处理。',
        waitingOnly: '你名下的 {{waiting}} 件事正在等别人。',
        agentsOnly: '有 {{agents}} 个 Agent 正在推进你名下的工作。',
        calm: '现在没有等你处理的事，也没有你名下的等待。',
        unknown: '有些数字还没有读全；已确认的明细仍在下面。',
      },
      generatedBy: '程序按事实生成 · {{time}}', generatedByNoTime: '程序按事实生成',
      updateBy: '{{name}} 于 {{time}} 写的', updateAuthorFallback: '项目负责人',
    },
    chains: { answer: '等你回答', decision: '等一个决定', review: '等复核', outside: '等{{party}}', outsideFallback: '项目外的一方' },
    waiting: {
      reviewerFallback: '有复核职责的成员', dependencyFallback: '上游任务的负责人',
      decisionFallback: '能回答这个问题的人', outsideFallback: '项目外的一方', separator: '、',
    },
  },
  'en-US': {
    health: { ok: 'On track', risk: 'At risk', off: 'Off track', none: 'No update', info: 'Recently taken over' },
    units: { tasks: '', agents: '' },
    metrics: {
      verified: 'Verified this week', agents: 'Agents running', waiting: 'Waiting on others', pending: 'Waiting for you',
      previousWeek: '{{count}} last week', stalledOther: 'Also stopped: {{count}}',
      waitingOldest: 'Oldest {{days}} days, waiting for {{party}}', pendingOldest: 'Oldest {{days}} days',
    },
    verdict: {
      sentences: {
        pendingAndWaiting: '{{pending}} items are waiting for you; {{waiting}} of your tasks are waiting on others.',
        pendingOnly: '{{pending}} items are waiting for you.',
        waitingOnly: '{{waiting}} of your tasks are waiting on others.',
        agentsOnly: '{{agents}} Agents are moving your work forward.',
        calm: 'Nothing is waiting for you, and none of your tasks are waiting on others.',
        unknown: 'Some totals are not fully read yet; the confirmed details remain below.',
      },
      generatedBy: 'Generated from recorded facts · {{time}}', generatedByNoTime: 'Generated from recorded facts',
      updateBy: 'Written by {{name}} at {{time}}', updateAuthorFallback: 'Project lead',
    },
    chains: { answer: 'Waiting for your answer', decision: 'Waiting for a decision', review: 'Waiting for review', outside: 'Waiting for {{party}}', outsideFallback: 'outside party' },
    waiting: {
      reviewerFallback: 'Member with review responsibility', dependencyFallback: 'Owner of the upstream task',
      decisionFallback: 'Person who can answer', outsideFallback: 'Outside party', separator: ', ',
    },
  },
};

export function workbenchVocabulary(language: string): WorkbenchVocabulary {
  return VOCABULARY[language.toLowerCase().startsWith('zh') ? 'zh-CN' : 'en-US'];
}

export interface WorkbenchVerdict {
  health: HealthStatus;
  healthLabel: string;
  sentence: string;
  source: 'recorded-update' | 'generated';
  byline: string;
  updateId: string | null;
}

function fill(template: string, values: Record<string, string | number>): string {
  return template.replace(/\{\{(\w+)\}\}/g, (_match, key: string) => String(values[key] ?? ''));
}

export function buildWorkbenchVerdict(
  metrics: OverviewViewSummary['metrics'],
  update: WorkbenchPeriodUpdate | null | undefined,
  language: string,
  readAtUnixMs?: number,
): WorkbenchVerdict {
  const vocabulary = workbenchVocabulary(language);
  if (update && update.sentence.trim()) {
    return {
      health: update.health,
      healthLabel: vocabulary.health[update.health],
      sentence: update.sentence,
      source: 'recorded-update',
      byline: fill(vocabulary.verdict.updateBy, {
        name: update.authorName?.trim() || vocabulary.verdict.updateAuthorFallback,
        time: formatUnixMs(update.writtenAtUnixMs, language),
      }),
      updateId: update.id,
    };
  }

  const pending = metrics[3].count;
  const waiting = metrics[2].count;
  const agents = metrics[1].count;
  const values = { pending: pending ?? '', waiting: waiting ?? '', agents: agents ?? '' };
  let rule = VERDICT_RULES[0];
  if (pending === null || waiting === null || agents === null) rule = VERDICT_RULES[0];
  else if (pending > 0 && waiting > 0) rule = VERDICT_RULES[1];
  else if (pending > 0) rule = VERDICT_RULES[2];
  else if (waiting > 0) rule = VERDICT_RULES[3];
  else if (agents !== null && agents > 0) rule = VERDICT_RULES[4];
  else rule = VERDICT_RULES[5];
  return {
    health: 'none',
    healthLabel: vocabulary.health.none,
    sentence: fill(vocabulary.verdict.sentences[rule.sentenceKey], values),
    source: 'generated',
    byline: readAtUnixMs === undefined
      ? vocabulary.verdict.generatedByNoTime
      : fill(vocabulary.verdict.generatedBy, { time: formatUnixMs(readAtUnixMs, language) }),
    updateId: null,
  };
}

export type MyMetricId = 'verified' | 'agents' | 'waiting' | 'pending';

export interface WorkbenchMetric {
  id: MyMetricId;
  key: ViewMetricKey;
  label: string;
  unit: string;
  note: string | null;
  count: number | null;
  emphasized: boolean;
  details: ViewMetric<unknown>;
}

export interface WorkbenchCoverage {
  partial: OverviewPart[];
  unread: OverviewPart[];
  incompleteFacts: boolean;
  approvalsUnread: boolean;
}

export interface MyWorkModel {
  metrics: [WorkbenchMetric, WorkbenchMetric, WorkbenchMetric, WorkbenchMetric];
  pending: CountDetails<PersonPendingItem>;
  chains: DerivedChain[];
  chainTabs: Array<{ id: string; label: string }>;
  currentPerson: PersonSummary | null;
  agentTasks: DerivedTask[];
  verdict: WorkbenchVerdict;
  coverage: WorkbenchCoverage;
}

export interface MyWorkModelOptions {
  personId: string | null;
  projectId?: string | null;
  readAtUnixMs?: number;
  latestPeriodUpdate?: WorkbenchPeriodUpdate | null;
}

export function metricDetailItems(metric: WorkbenchMetric): unknown[] {
  return metric.details.items;
}

function isDerivedWait(value: unknown): value is DerivedWait {
  return Boolean(value && typeof value === 'object' && 'kind' in value && 'task' in value && 'behind' in value);
}

function isPersonPendingItem(value: unknown): value is PersonPendingItem {
  return Boolean(value && typeof value === 'object' && 'kind' in value && 'task' in value && 'wait' in value && 'handoff' in value && 'approval' in value);
}

function elapsedWholeDays(atUnixMs: number | null | undefined, readAtUnixMs: number | undefined): number | null {
  if (typeof atUnixMs !== 'number' || !Number.isFinite(atUnixMs) || atUnixMs < 0 || readAtUnixMs === undefined || atUnixMs > readAtUnixMs) return null;
  return Math.floor((readAtUnixMs - atUnixMs) / 86_400_000);
}

function pendingDurationDays(item: PersonPendingItem, readAtUnixMs: number | undefined): number | null {
  if (item.kind === 'wait') return item.wait?.durationDays ?? null;
  if (item.kind === 'handoff') return elapsedWholeDays(item.handoff?.updatedAtUnixMs, readAtUnixMs);
  return elapsedWholeDays(item.approval?.sinceUnixMs, readAtUnixMs);
}

function metricNote(
  id: MyMetricId,
  details: ViewMetric<unknown>,
  language: string,
  currentPerson: PersonSummary | null,
  readAtUnixMs: number | undefined,
): string | null {
  const vocabulary = workbenchVocabulary(language);
  if (id === 'verified') {
    return typeof details.previousPeriod?.count === 'number'
      ? fill(vocabulary.metrics.previousWeek, { count: details.previousPeriod.count })
      : null;
  }
  if (id === 'agents') {
    if (!currentPerson) return null;
    const stalled = currentPerson.inProgress.items.filter((task) => task.status === 'stalled');
    if (stalled.some((task) => task.agentActive === null)) return null;
    const count = stalled.filter((task) => task.agentActive === false).length;
    return count > 0 ? fill(vocabulary.metrics.stalledOther, { count }) : null;
  }
  if (id === 'waiting') {
    const waits = details.items.filter(isDerivedWait);
    if (waits.length === 0 || waits.some((wait) => wait.durationDays === null)) return null;
    const longest = waits.reduce((left, right) => (left.durationDays ?? -1) >= (right.durationDays ?? -1) ? left : right);
    return fill(vocabulary.metrics.waitingOldest, { days: longest.durationDays!, party: waitingOnText(longest, language) });
  }
  const pending = details.items.filter(isPersonPendingItem);
  if (pending.length === 0) return null;
  const durations = pending.map((item) => pendingDurationDays(item, readAtUnixMs));
  if (durations.some((days) => days === null)) return null;
  return fill(vocabulary.metrics.pendingOldest, { days: Math.max(...durations as number[]) });
}

function metric(
  id: MyMetricId,
  details: ViewMetric<unknown>,
  language: string,
  currentPerson: PersonSummary | null,
  readAtUnixMs: number | undefined,
): WorkbenchMetric {
  const vocabulary = workbenchVocabulary(language);
  const labels = {
    verified: vocabulary.metrics.verified,
    agents: vocabulary.metrics.agents,
    waiting: vocabulary.metrics.waiting,
    pending: vocabulary.metrics.pending,
  };
  return {
    id,
    key: details.key,
    label: labels[id],
    unit: id === 'agents' ? vocabulary.units.agents : vocabulary.units.tasks,
    note: metricNote(id, details, language, currentPerson, readAtUnixMs),
    count: details.count,
    emphasized: id === 'pending',
    details,
  };
}

export function buildMyWorkModel(
  derived: OverviewDerivation,
  language: string,
  options: MyWorkModelOptions = { personId: null },
): MyWorkModel {
  const currentPerson = options.personId === null
    ? null
    : derived.people.find((summary) => summary.person.id === options.personId) ?? null;
  const metrics = derived.views.me.metrics.map((item, index) => metric(
    (['verified', 'agents', 'waiting', 'pending'] as const)[index], item, language, currentPerson, options.readAtUnixMs,
  )) as MyWorkModel['metrics'];
  const chains = options.personId === null ? [] : derived.chains.filter((chain) => (
    chain.impact.items.some((task) => task.owner?.id === options.personId)
    || chain.root.queue.items.some((wait) => wait.waitingOn.some((person) => person.id === options.personId))
  ));
  const vocabulary = workbenchVocabulary(language);
  const chainTabs = chains.map((chain) => {
    let label = vocabulary.chains.review;
    if (chain.root.kind === 'decision') {
      label = chain.root.queue.items.some((wait) => wait.waitingOn.some((person) => person.id === options.personId))
        ? vocabulary.chains.answer
        : vocabulary.chains.decision;
    } else if (chain.root.kind === 'external') {
      label = fill(vocabulary.chains.outside, { party: chain.root.externalParty ?? vocabulary.chains.outsideFallback });
    }
    return { id: chain.id, label };
  });
  return {
    metrics,
    pending: metrics[3].details as CountDetails<PersonPendingItem> & { key: ViewMetricKey },
    chains,
    chainTabs,
    currentPerson,
    agentTasks: options.personId === null ? [] : derived.tasks.filter((task) => (
      task.owner?.id === options.personId && task.status !== 'verified'
    )),
    verdict: buildWorkbenchVerdict(derived.views.me.metrics, options.latestPeriodUpdate, language, options.readAtUnixMs),
    coverage: {
      partial: (Object.entries(derived.coverage.parts) as Array<[OverviewPart, { state: string }]>)
        .filter(([, part]) => part.state === 'partial').map(([part]) => part),
      unread: (Object.entries(derived.coverage.parts) as Array<[OverviewPart, { state: string }]>)
        .filter(([, part]) => part.state === 'unread').map(([part]) => part),
      incompleteFacts: !derived.coverage.complete,
      approvalsUnread: derived.coverage.parts.approvals.state === 'unread',
    },
  };
}

export function workbenchDisplayText(value: string | null | undefined, fallback: string, budget = 120): string {
  const normalized = typeof value === 'string' ? value.replace(/\s+/g, ' ').trim() : '';
  return normalized ? truncateCodePoints(normalized, budget).text : fallback;
}

export function workbenchAgentName(value: string | null | undefined, fallback: string): string {
  const withoutOwnerSuffix = typeof value === 'string'
    ? value.replace(/（[^（）]*的 Agent）$/, '').replace(/\([^()]*'s Agent\)$/, '').trim()
    : '';
  return workbenchDisplayText(withoutOwnerSuffix, fallback);
}

export function waitingOnText(wait: DerivedWait, language: string): string {
  const vocabulary = workbenchVocabulary(language).waiting;
  const names = wait.waitingOn.map((person) => workbenchDisplayText(person.displayName, '', 80)).filter(Boolean);
  if (names.length > 0) return names.join(vocabulary.separator);
  if (wait.kind === 'review') return vocabulary.reviewerFallback;
  if (wait.kind === 'external') return workbenchDisplayText(wait.externalParty, vocabulary.outsideFallback);
  if (wait.kind === 'dependency') return vocabulary.dependencyFallback;
  return vocabulary.decisionFallback;
}

const DAY_MS = 86_400_000;

interface ZonedDateParts {
  year: number;
  month: number;
  day: number;
  daysFromMonday: number;
}

function zonedDateParts(nowUnixMs: number, timeZone: string): ZonedDateParts {
  const parts = new Intl.DateTimeFormat('en-US', {
    timeZone, year: 'numeric', month: '2-digit', day: '2-digit', weekday: 'short',
  }).formatToParts(nowUnixMs);
  const value = (type: Intl.DateTimeFormatPartTypes) => parts.find((part) => part.type === type)?.value ?? '';
  const weekdays = ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat'];
  return {
    year: Number(value('year')),
    month: Number(value('month')),
    day: Number(value('day')),
    daysFromMonday: (weekdays.indexOf(value('weekday')) + 6) % 7,
  };
}

function timeZoneOffsetMs(nowUnixMs: number, timeZone: string): number {
  const parts = new Intl.DateTimeFormat('en-US', {
    timeZone,
    year: 'numeric', month: '2-digit', day: '2-digit',
    hour: '2-digit', minute: '2-digit', second: '2-digit', hourCycle: 'h23',
  }).formatToParts(nowUnixMs);
  const value = (type: Intl.DateTimeFormatPartTypes) => Number(parts.find((part) => part.type === type)?.value ?? 0);
  const asUtc = Date.UTC(value('year'), value('month') - 1, value('day'), value('hour'), value('minute'), value('second'));
  return asUtc - Math.floor(nowUnixMs / 1000) * 1000;
}

/** Convert a calendar midnight in the viewer's zone to its epoch timestamp, including DST changes. */
function zonedCalendarToUtc(calendarUnixMs: number, timeZone: string): number {
  let converted = calendarUnixMs;
  for (let iteration = 0; iteration < 2; iteration += 1) {
    converted = calendarUnixMs - timeZoneOffsetMs(converted, timeZone);
  }
  return converted;
}

function zonedDayCalendar(nowUnixMs: number, timeZone: string): number {
  const parts = zonedDateParts(nowUnixMs, timeZone);
  return Date.UTC(parts.year, parts.month - 1, parts.day);
}

function weekPeriod(nowUnixMs: number, weeksBefore = 0, timeZone = 'UTC') {
  const dayCalendar = zonedDayCalendar(nowUnixMs, timeZone);
  const { daysFromMonday } = zonedDateParts(nowUnixMs, timeZone);
  const startCalendar = dayCalendar - (daysFromMonday + weeksBefore * 7) * DAY_MS;
  return {
    startUnixMs: zonedCalendarToUtc(startCalendar, timeZone),
    endUnixMs: zonedCalendarToUtc(startCalendar + 7 * DAY_MS, timeZone) - 1,
  };
}

function monthPeriod(nowUnixMs: number, monthsBefore = 0, timeZone = 'UTC') {
  const { year, month } = zonedDateParts(nowUnixMs, timeZone);
  const startCalendar = Date.UTC(year, month - 1 - monthsBefore, 1);
  return {
    startUnixMs: zonedCalendarToUtc(startCalendar, timeZone),
    endUnixMs: zonedCalendarToUtc(Date.UTC(year, month - monthsBefore, 1), timeZone) - 1,
  };
}

/** Fill caller-owned scopes and periods; an absent member remains explicitly unknown. */
export function workbenchInputOptions(
  options: OverviewDerivationOptions,
  projectId: string,
  personId: string | null,
  timeZone = Intl.DateTimeFormat().resolvedOptions().timeZone,
): OverviewDerivationOptions {
  const supplied = options.views;
  return {
    ...options,
    views: {
      ...supplied,
      me: { personId, projectId },
      lead: { personId, projectId },
      department: supplied?.department ?? { personId, projectIds: [projectId] },
      top: supplied?.top ?? { personId, goalIds: null },
      currentWeek: supplied?.currentWeek ?? weekPeriod(options.nowUnixMs, 0, timeZone),
      previousWeek: supplied?.previousWeek ?? weekPeriod(options.nowUnixMs, 1, timeZone),
      currentMonth: supplied?.currentMonth ?? monthPeriod(options.nowUnixMs, 0, timeZone),
      previousMonth: supplied?.previousMonth ?? monthPeriod(options.nowUnixMs, 1, timeZone),
    },
  };
}
