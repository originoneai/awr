import type {
  CountDetails,
  DerivedChain,
  DerivedTask,
  DerivedWait,
  OverviewDerivation,
  OverviewDerivationOptions,
  OverviewPart,
  OverviewProgressReport,
  OverviewTask,
  OverviewViewSummary,
  PersonPendingItem,
  PersonSummary,
  ViewMetric,
  ViewMetricKey,
  ViewId,
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
  { id: 'verified-previous', selection: 'previousPeriod.count is a number', sentenceKey: 'previousWeek' },
  { id: 'agents-stalled', selection: 'stalledOwned > 0 && all activity known', sentenceKey: 'stalledOther' },
  { id: 'waiting-oldest', selection: 'every waiting.durationDays is known', sentenceKey: 'waitingOldest' },
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
    waitingForViewer: string;
    generatedBy: string; generatedByNoTime: string;
    updateBy: string; updateByLead: string; updateByDepartment: string; updateByTop: string;
    updateAuthorFallback: string;
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
      waitingForViewer: '等你',
      generatedBy: '按你名下的任务和 Agent 的报告算出 · 刚刚',
      generatedByNoTime: '按你名下的任务和 Agent 的报告算出 · 刚刚',
      updateBy: '{{name}} 于 {{time}} 写的',
      updateByLead: '你今天 {{time}} 在自己的 Agent 里写的 · 第 {{week}} 周',
      updateByDepartment: '你 {{date}}在自己的 Agent 里写的 · 汇总时取最差的那个，不取平均',
      updateByTop: '取自各部门的月更新，取最差的那个 · {{date}}',
      updateAuthorFallback: '项目负责人',
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
      waitingForViewer: 'Waiting for you',
      generatedBy: 'Calculated from your tasks and Agent reports · just now',
      generatedByNoTime: 'Calculated from your tasks and Agent reports · just now',
      updateBy: 'Written by {{name}} at {{time}}',
      updateByLead: 'You wrote this in your own Agent at {{time}} today · week {{week}}',
      updateByDepartment: 'You wrote this in your own Agent on {{date}} · the aggregate uses the worst result, not an average',
      updateByTop: 'Taken from department monthly updates, using the worst result · {{date}}',
      updateAuthorFallback: 'Project lead',
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

function localClockTime(unixMs: number, language: string): string {
  return new Intl.DateTimeFormat(language.toLowerCase().startsWith('zh') ? 'zh-CN' : 'en-US', {
    hour: '2-digit', minute: '2-digit', hourCycle: 'h23',
  }).format(unixMs);
}

function localMonthDay(unixMs: number, language: string): string {
  const date = new Date(unixMs);
  if (language.toLowerCase().startsWith('zh')) return `${date.getMonth() + 1} 月 ${date.getDate()} 日`;
  return new Intl.DateTimeFormat('en-US', { month: 'long', day: 'numeric' }).format(unixMs);
}

function sameLocalDay(leftUnixMs: number, rightUnixMs: number): boolean {
  const left = new Date(leftUnixMs);
  const right = new Date(rightUnixMs);
  return left.getFullYear() === right.getFullYear()
    && left.getMonth() === right.getMonth()
    && left.getDate() === right.getDate();
}

/** ISO week number of the viewer-local calendar date, matching the prototype's week label. */
function localIsoWeek(unixMs: number): number {
  const local = new Date(unixMs);
  const date = new Date(Date.UTC(local.getFullYear(), local.getMonth(), local.getDate()));
  const day = date.getUTCDay() || 7;
  date.setUTCDate(date.getUTCDate() + 4 - day);
  const yearStart = Date.UTC(date.getUTCFullYear(), 0, 1);
  return Math.floor((date.getTime() - yearStart) / 86_400_000 / 7) + 1;
}

function recordedUpdateByline(
  update: WorkbenchPeriodUpdate,
  vocabulary: WorkbenchVocabulary,
  language: string,
  readAtUnixMs: number | undefined,
  perspective: ViewId,
): string {
  const generic = () => fill(vocabulary.verdict.updateBy, {
    name: update.authorName?.trim() || vocabulary.verdict.updateAuthorFallback,
    time: formatUnixMs(update.writtenAtUnixMs, language),
  });
  if (!Number.isFinite(update.writtenAtUnixMs)) return generic();
  if (perspective === 'lead' && readAtUnixMs !== undefined && Number.isFinite(readAtUnixMs)
    && sameLocalDay(update.writtenAtUnixMs, readAtUnixMs)) {
    return fill(vocabulary.verdict.updateByLead, {
      time: localClockTime(update.writtenAtUnixMs, language), week: localIsoWeek(readAtUnixMs),
    });
  }
  if (perspective === 'department') {
    return fill(vocabulary.verdict.updateByDepartment, { date: localMonthDay(update.writtenAtUnixMs, language) });
  }
  if (perspective === 'top') {
    return fill(vocabulary.verdict.updateByTop, { date: localMonthDay(update.writtenAtUnixMs, language) });
  }
  return generic();
}

export function buildWorkbenchVerdict(
  metrics: OverviewViewSummary['metrics'],
  update: WorkbenchPeriodUpdate | null | undefined,
  language: string,
  readAtUnixMs?: number,
  perspective: ViewId = 'me',
): WorkbenchVerdict {
  const vocabulary = workbenchVocabulary(language);
  if (update && update.sentence.trim()) {
    return {
      health: update.health,
      healthLabel: vocabulary.health[update.health],
      sentence: update.sentence,
      source: 'recorded-update',
      byline: recordedUpdateByline(update, vocabulary, language, readAtUnixMs, perspective),
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
  const waitingForViewer = pending !== null && pending > 0;
  return {
    health: waitingForViewer ? 'risk' : 'none',
    healthLabel: waitingForViewer ? vocabulary.verdict.waitingForViewer : vocabulary.health.none,
    sentence: fill(vocabulary.verdict.sentences[rule.sentenceKey], values),
    source: 'generated',
    byline: readAtUnixMs === undefined
      ? vocabulary.verdict.generatedByNoTime
      : vocabulary.verdict.generatedBy,
    updateId: null,
  };
}

export type MyMetricId = 'verified' | 'agents' | 'waiting' | 'pending';

export interface WorkbenchMetricNote {
  text: string;
  /** The prototype's .kpi .c data-d tone, derived from the same facts as the sentence. */
  tone?: 'good' | 'warn' | 'bad';
  /** The prototype's up/down arrow, only when the comparison direction is known. */
  trend?: 'up' | 'down';
}

export interface WorkbenchMetric {
  id: MyMetricId;
  key: ViewMetricKey;
  label: string;
  unit: string;
  note: WorkbenchMetricNote | null;
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

/** A derived task joined with its recorded report fact, read by id from the raw input. */
export type ReportedTask = DerivedTask & { report: OverviewProgressReport | null };

export interface MyWorkModel {
  metrics: [WorkbenchMetric, WorkbenchMetric, WorkbenchMetric, WorkbenchMetric];
  pending: CountDetails<PersonPendingItem>;
  chains: DerivedChain[];
  chainTabs: Array<{ id: string; label: string }>;
  currentPerson: PersonSummary | null;
  agentTasks: ReportedTask[];
  verdict: WorkbenchVerdict;
  coverage: WorkbenchCoverage;
}

export interface MyWorkModelOptions {
  personId: string | null;
  projectId?: string | null;
  readAtUnixMs?: number;
  latestPeriodUpdate?: WorkbenchPeriodUpdate | null;
  /** The raw overview task items; the recorded report fact is joined by task id. */
  inputTasks?: readonly OverviewTask[];
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

export function pendingDurationDays(item: PersonPendingItem, readAtUnixMs: number | undefined): number | null {
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
): WorkbenchMetricNote | null {
  const vocabulary = workbenchVocabulary(language);
  if (id === 'verified') {
    if (typeof details.previousPeriod?.count !== 'number') return null;
    const note: WorkbenchMetricNote = { text: fill(vocabulary.metrics.previousWeek, { count: details.previousPeriod.count }) };
    if (typeof details.count === 'number') {
      if (details.count > details.previousPeriod.count) {
        note.tone = 'good';
        note.trend = 'up';
      } else if (details.count < details.previousPeriod.count) {
        note.tone = 'bad';
        note.trend = 'down';
      }
    }
    return note;
  }
  if (id === 'agents') {
    if (!currentPerson) return null;
    const stalled = currentPerson.inProgress.items.filter((task) => task.status === 'stalled');
    if (stalled.some((task) => task.agentActive === null)) return null;
    const count = stalled.filter((task) => task.agentActive === false).length;
    return count > 0 ? { text: fill(vocabulary.metrics.stalledOther, { count }), tone: 'warn' } : null;
  }
  if (id === 'waiting') {
    const waits = details.items.filter(isDerivedWait);
    if (waits.length === 0 || waits.some((wait) => wait.durationDays === null)) return null;
    const longest = waits.reduce((left, right) => (left.durationDays ?? -1) >= (right.durationDays ?? -1) ? left : right);
    return { text: fill(vocabulary.metrics.waitingOldest, { days: longest.durationDays!, party: waitingOnText(longest, language) }) };
  }
  const pending = details.items.filter(isPersonPendingItem);
  if (pending.length === 0) return null;
  const durations = pending.map((item) => pendingDurationDays(item, readAtUnixMs));
  if (durations.some((days) => days === null)) return null;
  return { text: fill(vocabulary.metrics.pendingOldest, { days: Math.max(...durations as number[]) }) };
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

function reportedTasks(derived: OverviewDerivation, personId: string | null, inputTasks: readonly OverviewTask[] | undefined): ReportedTask[] {
  if (personId === null) return [];
  const reports = new Map<string, OverviewProgressReport | null>();
  if (inputTasks) {
    for (const task of inputTasks) {
      if (task.id != null) reports.set(task.id, task.report ?? null);
    }
  }
  return derived.tasks
    .filter((task) => task.owner?.id === personId && task.status !== 'verified')
    .map((task) => ({ ...task, report: task.id == null ? null : reports.get(task.id) ?? null }));
}

/** Whole elapsed minutes between a recorded time and the read's fixed "now"; never negative. */
export function elapsedMinutes(atUnixMs: number, nowUnixMs: number): number {
  if (!Number.isFinite(atUnixMs) || !Number.isFinite(nowUnixMs) || atUnixMs > nowUnixMs) return -1;
  return Math.floor((nowUnixMs - atUnixMs) / 60_000);
}

export type ReportAge = 'unknown' | 'justNow' | 'minutes' | 'hours' | 'days';

/** The prototype's rail ages ("12 分钟前", "2 天前") bucketed to the nearest whole unit. */
export function reportAge(atUnixMs: number | null | undefined, nowUnixMs: number | undefined): { kind: ReportAge; count: number } {
  if (typeof atUnixMs !== 'number' || !Number.isFinite(atUnixMs) || nowUnixMs === undefined) {
    return { kind: 'unknown', count: 0 };
  }
  const minutes = elapsedMinutes(atUnixMs, nowUnixMs);
  if (minutes < 0) return { kind: 'unknown', count: 0 };
  if (minutes < 1) return { kind: 'justNow', count: 0 };
  if (minutes < 60) return { kind: 'minutes', count: minutes };
  if (minutes < 24 * 60) return { kind: 'hours', count: Math.floor(minutes / 60) };
  return { kind: 'days', count: Math.floor(minutes / (24 * 60)) };
}

export type ReportDeclaration = 'unknown' | 'done' | 'inProgress' | 'noDeclaration';

/** What the reporter declared; the report prose itself stays in the backend. */
export function reportDeclaration(report: OverviewProgressReport | null): ReportDeclaration {
  if (!report) return 'unknown';
  if (report.completed === true) return 'done';
  if (report.completed === false) return 'inProgress';
  return 'noDeclaration';
}

function compareChainFact(left: number | null, right: number | null): number {
  if (left === null && right === null) return 0;
  if (left === null) return 1;
  if (right === null) return -1;
  return right - left;
}

/** My Work puts waits addressed to the viewer first; every other perspective keeps core order. */
export function orderWorkbenchChains(
  chains: readonly DerivedChain[],
  personId: string | null,
  perspective: ViewId = 'me',
): DerivedChain[] {
  if (perspective !== 'me') return [...chains];
  return chains.map((chain, index) => ({
    chain,
    index,
    waitingForViewer: personId !== null && chain.root.queue.items.some((wait) =>
      wait.waitingOn.some((person) => person.id === personId)),
  })).sort((left, right) => Number(right.waitingForViewer) - Number(left.waitingForViewer)
    || compareChainFact(left.chain.impact.count, right.chain.impact.count)
    || compareChainFact(left.chain.root.oldestDays, right.chain.root.oldestDays)
    || left.index - right.index)
    .map(({ chain }) => chain);
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
  const relevantChains = options.personId === null ? [] : derived.chains.filter((chain) => (
    chain.impact.items.some((task) => task.owner?.id === options.personId)
    || chain.root.queue.items.some((wait) => wait.waitingOn.some((person) => person.id === options.personId))
  ));
  const chains = orderWorkbenchChains(relevantChains, options.personId, 'me');
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
    agentTasks: reportedTasks(derived, options.personId, options.inputTasks),
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
