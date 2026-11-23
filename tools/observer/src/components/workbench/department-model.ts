import { ageText } from '../../core/observation';
import { formatUnixMs } from '../../lib/format';
import type {
  PortfolioChainFact,
  PortfolioCrossGroupWait,
  PortfolioDepartmentUpdate,
  PortfolioLessonFact,
  PortfolioMonthlyVerifiedItem,
  PortfolioPendingItem,
  PortfolioProjectComplete,
  PortfolioProjectItem,
  TeamPortfolioResponse,
} from '../../lib/types';
import type { CompletionCounts, HealthStatus } from '../display';
import { workbenchDisplayText, type WorkbenchMetricNote, type WorkbenchVerdict } from './model';

const DAY_MS = 86_400_000;
const UPDATE_LATE_AFTER_DAYS = 7;

export type DepartmentVerdictSentenceKey =
  | 'empty'
  | 'partial'
  | 'off'
  | 'riskAndNoUpdate'
  | 'risk'
  | 'noUpdate'
  | 'ok';

export interface DepartmentVerdictRule {
  id: string;
  selection: string;
  sentenceKey: DepartmentVerdictSentenceKey;
}

/** Ordered deterministic rules for the department summary when no recorded update exists. */
export const DEPT_VERDICT_RULES: readonly DepartmentVerdictRule[] = [
  { id: 'dept-empty', selection: 'total === 0', sentenceKey: 'empty' },
  { id: 'dept-partial', selection: 'notRead > 0 || truncated', sentenceKey: 'partial' },
  { id: 'dept-off', selection: 'off > 0', sentenceKey: 'off' },
  { id: 'dept-risk-and-noupdate', selection: 'risk > 0 && noUpdate > 0', sentenceKey: 'riskAndNoUpdate' },
  { id: 'dept-risk', selection: 'risk > 0', sentenceKey: 'risk' },
  { id: 'dept-noupdate', selection: 'noUpdate > 0', sentenceKey: 'noUpdate' },
  { id: 'dept-ok', selection: 'true', sentenceKey: 'ok' },
];

export type DepartmentProjectHealth = HealthStatus | 'unknown';

export type DepartmentProjectSummary =
  | { kind: 'update'; text: string }
  | { kind: 'last-update'; days: number }
  | { kind: 'facts'; total: number; verified: number }
  | { kind: 'not-read' };

export interface DepartmentProjectRow {
  projectId: string;
  projectKey: string;
  title: string;
  leadName: string | null;
  groupName: string | null;
  departmentName: string | null;
  health: DepartmentProjectHealth;
  summary: DepartmentProjectSummary;
  completion: CompletionCounts | null;
  updatedAtUnixMs: number | null;
  elapsedDays: number | null;
  late: boolean;
  readComplete: boolean;
  usageDisplayText: string | null;
}

export type DepartmentMetricId = 'projects' | 'verified' | 'waits' | 'pending';

export interface DepartmentMetric<T = unknown> {
  id: DepartmentMetricId;
  label: string;
  unit: string;
  count: number | null;
  note: WorkbenchMetricNote | null;
  emphasized: boolean;
  items: T[];
}

export interface DepartmentCoverage {
  complete: boolean;
  returnedProjects: number;
  totalProjects: number;
  notReadProjects: number;
  truncated: boolean;
  hiddenWorkTotal: number;
}

export interface DepartmentWorkbenchModel {
  departmentName: string | null;
  subtitle: string;
  verdict: WorkbenchVerdict;
  metrics: [
    DepartmentMetric<DepartmentProjectRow>,
    DepartmentMetric<PortfolioMonthlyVerifiedItem>,
    DepartmentMetric<PortfolioCrossGroupWait>,
    DepartmentMetric<PortfolioPendingItem>,
  ];
  chains: PortfolioChainFact[];
  pending: {
    count: number | null;
    items: PortfolioPendingItem[];
  };
  projects: DepartmentProjectRow[];
  waits: PortfolioCrossGroupWait[] | null;
  lessons: PortfolioLessonFact[] | null;
  hasAnyProjectUsage: boolean;
  usageNote: string | null;
  coverage: DepartmentCoverage;
}

interface DepartmentVocabulary {
  health: Record<HealthStatus, string>;
  units: { projects: string; tasks: string; waits: string; pending: string };
  metrics: {
    projects: string;
    verified: string;
    waits: string;
    pending: string;
    projectsBreakdownRiskAndNoUpdate: string;
    projectsBreakdownOff: string;
    projectsBreakdownRisk: string;
    projectsBreakdownNoUpdate: string;
    projectsBreakdownOk: string;
    previousMonth: string;
    waitsOldest: string;
    pendingSingle: string;
    pendingOldest: string;
  };
  verdict: {
    sentences: Record<DepartmentVerdictSentenceKey, string>;
    generatedBy: string;
    generatedByAge: string;
    updateByViewer: string;
    updateByOther: string;
    updateAuthorFallback: string;
  };
  chains: {
    reviewTab: string;
    reviewRootTitle: string;
    reviewFooter: string;
    waitTab: string;
    waitRootTitle: string;
    waitFooter: string;
    rootKicker: string;
    projectKicker: string;
    reviewCountMeta: string;
    waitCountMeta: string;
    projectAtRisk: string;
    projectOffTrack: string;
  };
  usage: {
    partialGroups: string;
    missingOnly: string;
    noneCollected: string;
    separator: string;
  };
  monthFormat: { zhSuffix: string; unknown: string };
}

const DEPT_VOCABULARY: Record<'zh-CN' | 'en-US', DepartmentVocabulary> = {
  'zh-CN': {
    health: { ok: '正常', risk: '有风险', off: '偏离', none: '没有更新', info: '刚接管' },
    units: { projects: '个', tasks: '件', waits: '个', pending: '件' },
    metrics: {
      projects: '项目',
      verified: '本月核验通过',
      waits: '跨小组等待',
      pending: '等你处理',
      projectsBreakdownRiskAndNoUpdate: '{{risk}} 个有风险，{{noUpdate}} 个没有更新',
      projectsBreakdownOff: '{{off}} 个偏离',
      projectsBreakdownRisk: '{{risk}} 个有风险',
      projectsBreakdownNoUpdate: '{{noUpdate}} 个没有更新',
      projectsBreakdownOk: '全部正常',
      previousMonth: '上月 {{count}} 件',
      waitsOldest: '最久 {{days}} 天，等{{party}}',
      pendingSingle: '等了 {{days}} 天',
      pendingOldest: '最久 {{days}} 天',
    },
    verdict: {
      sentences: {
        empty: '当前授权范围内还没有部门项目。',
        partial: '已读到 {{read}} 个项目，另有 {{unread}} 个没有读全；汇总状态按已读项目里最差的那个计算。',
        off: '{{total}} 个项目里 {{off}} 个偏离。',
        riskAndNoUpdate: '{{total}} 个项目里 {{risk}} 个有风险，{{noUpdate}} 个没有更新。',
        risk: '{{total}} 个项目里 {{risk}} 个有风险。',
        noUpdate: '{{total}} 个项目里 {{noUpdate}} 个没有更新。',
        ok: '{{total}} 个项目目前均按计划推进。',
      },
      generatedBy: '程序按各项目事实生成 · 汇总时取最差的那个，不取平均',
      generatedByAge: '程序按各项目事实生成 · 汇总时取最差的那个，不取平均 · {{age}}',
      updateByViewer: '你 {{date}}在自己的 Agent 里写的 · 汇总时取最差的那个，不取平均',
      updateByOther: '{{name}} 于 {{time}} 写的 · 汇总时取最差的那个，不取平均',
      updateAuthorFallback: '部门负责人',
    },
    chains: {
      reviewTab: '复核人手',
      reviewRootTitle: '复核排队中',
      reviewFooter: '部分项目的成果正在等待独立复核，点开对应项目可查看详情。',
      waitTab: '等待协调',
      waitRootTitle: '项目存在开放等待',
      waitFooter: '以下项目存在未解决的决定或上下游等待，点开项目可查看详情。',
      rootKicker: '卡在这里',
      projectKicker: '项目',
      reviewCountMeta: '{{count}} 件在等复核',
      waitCountMeta: '{{count}} 项等待处理',
      projectAtRisk: '最新一期状态有风险',
      projectOffTrack: '最新一期状态偏离',
    },
    usage: {
      partialGroups: '投入只采集到{{collected}}的用量，{{missing}}缺失，所以这里不显示费用。',
      missingOnly: '{{missing}}尚未采集用量，这里不显示费用。',
      noneCollected: '还没有采集各小组的用量，这里不显示费用。',
      separator: '、',
    },
    monthFormat: { zhSuffix: '月', unknown: '本月' },
  },
  'en-US': {
    health: { ok: 'On track', risk: 'At risk', off: 'Off track', none: 'No update', info: 'Recently taken over' },
    units: { projects: '', tasks: '', waits: '', pending: '' },
    metrics: {
      projects: 'Projects',
      verified: 'Verified this month',
      waits: 'Cross-group waits',
      pending: 'Waiting for you',
      projectsBreakdownRiskAndNoUpdate: '{{risk}} at risk, {{noUpdate}} without updates',
      projectsBreakdownOff: '{{off}} off track',
      projectsBreakdownRisk: '{{risk}} at risk',
      projectsBreakdownNoUpdate: '{{noUpdate}} without updates',
      projectsBreakdownOk: 'All on track',
      previousMonth: '{{count}} last month',
      waitsOldest: 'Oldest {{days}} days, waiting for {{party}}',
      pendingSingle: 'Waiting {{days}} days',
      pendingOldest: 'Oldest {{days}} days',
    },
    verdict: {
      sentences: {
        empty: 'There are no department projects in your authorized scope yet.',
        partial: '{{read}} projects were read, and {{unread}} could not be read in full; the rollup uses the worst status among read projects.',
        off: '{{off}} of {{total}} projects are off track.',
        riskAndNoUpdate: '{{risk}} of {{total}} projects are at risk, and {{noUpdate}} have no update.',
        risk: '{{risk}} of {{total}} projects are at risk.',
        noUpdate: '{{noUpdate}} of {{total}} projects have no update.',
        ok: 'All {{total}} projects are on track.',
      },
      generatedBy: 'Generated from project facts · the aggregate uses the worst result, not an average',
      generatedByAge: 'Generated from project facts · the aggregate uses the worst result, not an average · {{age}}',
      updateByViewer: 'You wrote this in your own Agent on {{date}} · the aggregate uses the worst result, not an average',
      updateByOther: 'Written by {{name}} at {{time}} · the aggregate uses the worst result, not an average',
      updateAuthorFallback: 'Department lead',
    },
    chains: {
      reviewTab: 'Review capacity',
      reviewRootTitle: 'Reviews queued',
      reviewFooter: 'Submitted deliverables are waiting for independent review. Open a project for details.',
      waitTab: 'Open waits',
      waitRootTitle: 'Projects have open waits',
      waitFooter: 'Open decisions or dependency waits remain across these projects. Open a project for details.',
      rootKicker: 'Blocked here',
      projectKicker: 'Project',
      reviewCountMeta: '{{count}} waiting for review',
      waitCountMeta: '{{count}} open waits',
      projectAtRisk: 'Latest update is at risk',
      projectOffTrack: 'Latest update is off track',
    },
    usage: {
      partialGroups: 'Usage was collected only for {{collected}}; usage is missing for {{missing}}, so no cost figures are shown here.',
      missingOnly: 'Usage has not been collected for {{missing}}, so no cost figures are shown here.',
      noneCollected: 'Usage has not been collected for any group yet, so no cost figures are shown here.',
      separator: ', ',
    },
    monthFormat: { zhSuffix: '', unknown: 'This month' },
  },
};

export function departmentVocabulary(language: string): DepartmentVocabulary {
  return DEPT_VOCABULARY[language.toLowerCase().startsWith('zh') ? 'zh-CN' : 'en-US'];
}

function fill(template: string, values: Record<string, string | number>): string {
  return template.replace(/\{\{(\w+)\}\}/g, (_match, key: string) => String(values[key] ?? ''));
}

function isCompleteProject(item: PortfolioProjectItem): item is PortfolioProjectComplete {
  return item.read.status === 'complete';
}

function validDateString(value: string | null | undefined): value is string {
  return typeof value === 'string' && /^\d{4}-\d{2}-\d{2}$/.test(value) && Number.isFinite(Date.parse(`${value}T00:00:00Z`));
}

function zonedDateKey(unixMs: number, timeZone: string): string | null {
  if (!Number.isFinite(unixMs) || unixMs < 0) return null;
  const parts = new Intl.DateTimeFormat('en-US', {
    year: 'numeric', month: '2-digit', day: '2-digit', timeZone,
  }).formatToParts(new Date(unixMs));
  const map = Object.fromEntries(parts.map((part) => [part.type, part.value]));
  return `${map.year}-${map.month}-${map.day}`;
}

function zonedMonthLabel(unixMs: number | undefined, timeZone: string, language: string): string {
  const vocab = departmentVocabulary(language);
  if (typeof unixMs !== 'number' || !Number.isFinite(unixMs)) return vocab.monthFormat.unknown;
  if (language.toLowerCase().startsWith('zh')) {
    const monthPart = new Intl.DateTimeFormat('en-US', { month: 'numeric', timeZone }).format(new Date(unixMs));
    return `${Number(monthPart)} ${vocab.monthFormat.zhSuffix}`;
  }
  return new Intl.DateTimeFormat('en-US', { month: 'long', timeZone }).format(new Date(unixMs));
}

function zonedMonthDay(unixMs: number, timeZone: string, language: string): string {
  if (language.toLowerCase().startsWith('zh')) {
    const parts = new Intl.DateTimeFormat('en-US', { month: 'numeric', day: 'numeric', timeZone }).formatToParts(new Date(unixMs));
    const month = Number(parts.find((p) => p.type === 'month')?.value);
    const day = Number(parts.find((p) => p.type === 'day')?.value);
    return `${month} 月 ${day} 日`;
  }
  return new Intl.DateTimeFormat('en-US', { month: 'long', day: 'numeric', timeZone }).format(new Date(unixMs));
}

function elapsedWholeDays(atUnixMs: number | null | undefined, nowUnixMs: number | undefined): number | null {
  if (typeof atUnixMs !== 'number' || !Number.isFinite(atUnixMs) || atUnixMs < 0
    || typeof nowUnixMs !== 'number' || !Number.isFinite(nowUnixMs) || atUnixMs > nowUnixMs) {
    return null;
  }
  return Math.floor((nowUnixMs - atUnixMs) / DAY_MS);
}

const HEALTH_SEVERITY: Record<HealthStatus, number> = {
  off: 4,
  risk: 3,
  none: 2,
  info: 2,
  ok: 1,
};

/**
 * Rollup strictly uses the worst status, never an average.
 * Severity: off > risk > none (or unreadable) > ok.
 */
export function worstProjectHealth(
  projects: readonly Pick<DepartmentProjectRow, 'health'>[],
  recordedHealth?: HealthStatus | null,
): HealthStatus {
  if (projects.length === 0) return recordedHealth ?? 'none';
  let worst: HealthStatus = 'ok';
  for (const project of projects) {
    const mapped: HealthStatus = project.health === 'unknown' ? 'none' : project.health;
    if (HEALTH_SEVERITY[mapped] > HEALTH_SEVERITY[worst]) worst = mapped;
  }
  if (recordedHealth && HEALTH_SEVERITY[recordedHealth] > HEALTH_SEVERITY[worst]) {
    worst = recordedHealth;
  }
  return worst;
}

/** Clock offset between the portfolio reply's authoritative as_of clock and read_at_unix_ms. */
export function portfolioClockOffsetUnixMs(response: TeamPortfolioResponse | null | undefined): number | null {
  const asOf = response?.result?.data?.as_of?.at_unix_ms;
  const readAt = response?.read_at_unix_ms;
  return typeof asOf === 'number' && Number.isFinite(asOf) && typeof readAt === 'number' && Number.isFinite(readAt)
    ? asOf - readAt
    : null;
}

/** The moment the portfolio read answered, projected into the reply's display clock. */
export function portfolioReadMomentUnixMs(response: TeamPortfolioResponse | null | undefined): number | undefined {
  const readAt = response?.read_at_unix_ms;
  if (typeof readAt !== 'number' || !Number.isFinite(readAt)) return undefined;
  return readAt + (portfolioClockOffsetUnixMs(response) ?? 0);
}

function deriveProjectRow(
  item: PortfolioProjectItem,
  response: TeamPortfolioResponse,
  nowUnixMs: number,
  timeZone: string,
): DepartmentProjectRow {
  const facts = response.department_facts;
  const recordedTitle = facts?.project_titles?.[item.project_key];
  const title = workbenchDisplayText(recordedTitle ?? item.project_key, item.project_key);
  const recordedLead = facts?.project_leads?.[item.project_key] ?? null;
  const rawUsageText = facts?.usage?.by_project?.[item.project_key]?.display_text;
  const usageDisplayText = typeof rawUsageText === 'string' && rawUsageText.trim().length > 0
    ? rawUsageText.trim()
    : null;

  if (!isCompleteProject(item)) {
    return {
      projectId: item.project_id,
      projectKey: item.project_key,
      title,
      leadName: recordedLead,
      groupName: null,
      departmentName: null,
      health: 'unknown',
      summary: { kind: 'not-read' },
      completion: null,
      updatedAtUnixMs: null,
      elapsedDays: null,
      late: false,
      readComplete: false,
      usageDisplayText: null,
    };
  }

  const leadName = item.responsible_person?.display_name?.trim() || recordedLead || null;
  const groupName = item.directory.group?.name?.trim() || null;
  const departmentName = item.directory.departments?.[0]?.name?.trim() || null;

  const works = item.works;
  const completion: CompletionCounts = {
    verified: Math.max(0, works.verified),
    reported: Math.max(0, works.delivered_unverified + works.completed_without_independent_review),
    inProgress: Math.max(0, works.in_progress),
    notStarted: Math.max(0, works.not_started),
  };

  const latest = item.latest_update;
  const today = zonedDateKey(nowUnixMs, timeZone);
  const updatedAtUnixMs = latest && typeof latest.recorded_at_unix_ms === 'number'
    && Number.isFinite(latest.recorded_at_unix_ms) && latest.recorded_at_unix_ms >= 0
    ? latest.recorded_at_unix_ms
    : null;
  const elapsedDays = elapsedWholeDays(updatedAtUnixMs, nowUnixMs);

  let health: DepartmentProjectHealth = 'none';
  let summary: DepartmentProjectSummary = { kind: 'facts', total: works.total, verified: works.verified };
  let late = false;

  if (latest && validDateString(latest.starts_on) && validDateString(latest.ends_on) && today !== null
    && latest.starts_on <= today && today <= latest.ends_on) {
    if (latest.status === 'ok' || latest.status === 'risk' || latest.status === 'off') {
      health = latest.status;
    } else {
      health = 'unknown';
    }
    const text = typeof latest.summary === 'string' ? latest.summary.trim() : '';
    summary = text ? { kind: 'update', text } : { kind: 'facts', total: works.total, verified: works.verified };
  } else if (latest) {
    health = 'none';
    summary = elapsedDays !== null
      ? { kind: 'last-update', days: elapsedDays }
      : { kind: 'facts', total: works.total, verified: works.verified };
    late = elapsedDays !== null && elapsedDays > UPDATE_LATE_AFTER_DAYS;
  }

  return {
    projectId: item.project_id,
    projectKey: item.project_key,
    title,
    leadName,
    groupName,
    departmentName,
    health,
    summary,
    completion,
    updatedAtUnixMs,
    elapsedDays,
    late,
    readComplete: true,
    usageDisplayText,
  };
}

function buildDepartmentVerdict(
  projects: readonly DepartmentProjectRow[],
  coverage: DepartmentCoverage,
  departmentUpdate: PortfolioDepartmentUpdate | null | undefined,
  viewerPersonId: string | null | undefined,
  language: string,
  readAtUnixMs: number | undefined,
  referenceUnixMs: number | undefined,
  timeZone: string,
): WorkbenchVerdict {
  const vocab = departmentVocabulary(language);
  const rolledHealth = worstProjectHealth(projects, departmentUpdate?.health ?? null);

  if (departmentUpdate && departmentUpdate.sentence.trim()) {
    const viewerAuthored = typeof departmentUpdate.author_person_id === 'string'
      && departmentUpdate.author_person_id.length > 0
      && typeof viewerPersonId === 'string' && viewerPersonId.length > 0
      && departmentUpdate.author_person_id === viewerPersonId;
    const byline = viewerAuthored && Number.isFinite(departmentUpdate.written_at_unix_ms)
      ? fill(vocab.verdict.updateByViewer, { date: zonedMonthDay(departmentUpdate.written_at_unix_ms, timeZone, language) })
      : fill(vocab.verdict.updateByOther, {
        name: departmentUpdate.author_name?.trim() || vocab.verdict.updateAuthorFallback,
        time: formatUnixMs(departmentUpdate.written_at_unix_ms, language),
      });
    return {
      health: rolledHealth,
      healthLabel: vocab.health[rolledHealth],
      sentence: departmentUpdate.sentence.trim(),
      source: 'recorded-update',
      byline,
      updateId: departmentUpdate.id,
    };
  }

  const total = coverage.totalProjects;
  const readCount = projects.filter((p) => p.readComplete).length;
  const unreadCount = Math.max(0, total - readCount);
  const off = projects.filter((p) => p.health === 'off').length;
  const risk = projects.filter((p) => p.health === 'risk').length;
  const noUpdate = projects.filter((p) => p.health === 'none').length;

  let rule = DEPT_VERDICT_RULES[6];
  if (total === 0) rule = DEPT_VERDICT_RULES[0];
  else if (coverage.notReadProjects > 0 || coverage.truncated) rule = DEPT_VERDICT_RULES[1];
  else if (off > 0) rule = DEPT_VERDICT_RULES[2];
  else if (risk > 0 && noUpdate > 0) rule = DEPT_VERDICT_RULES[3];
  else if (risk > 0) rule = DEPT_VERDICT_RULES[4];
  else if (noUpdate > 0) rule = DEPT_VERDICT_RULES[5];
  else rule = DEPT_VERDICT_RULES[6];

  const hasClock = typeof readAtUnixMs === 'number' && Number.isFinite(readAtUnixMs)
    && typeof referenceUnixMs === 'number' && Number.isFinite(referenceUnixMs);
  const byline = hasClock
    ? fill(vocab.verdict.generatedByAge, {
      age: ageText(readAtUnixMs, { language, nowUnixMs: referenceUnixMs, timeZone }).text,
    })
    : vocab.verdict.generatedBy;

  return {
    health: rolledHealth,
    healthLabel: vocab.health[rolledHealth],
    sentence: fill(vocab.verdict.sentences[rule.sentenceKey], {
      total,
      read: readCount,
      unread: unreadCount,
      off,
      risk,
      noUpdate,
    }),
    source: 'generated',
    byline,
    updateId: null,
  };
}

function deriveLivePortfolioChains(
  rawProjects: readonly PortfolioProjectItem[],
  rows: readonly DepartmentProjectRow[],
  language: string,
): PortfolioChainFact[] {
  const vocab = departmentVocabulary(language);
  const rowByKey = new Map(rows.map((r) => [r.projectKey, r]));
  const chains: PortfolioChainFact[] = [];

  const reviewProjects = rawProjects.filter((p): p is PortfolioProjectComplete =>
    isCompleteProject(p) && p.waits.review > 0);
  if (reviewProjects.length > 0) {
    const totalReviews = reviewProjects.reduce((sum, p) => sum + p.waits.review, 0);
    const primary = reviewProjects[0];
    const row = rowByKey.get(primary.project_key);
    chains.push({
      id: 'live-review-capacity',
      tab: vocab.chains.reviewTab,
      footer: vocab.chains.reviewFooter,
      cta: null,
      nodes: [
        {
          id: 'live-review-root',
          kind: 'root',
          root: true,
          kicker: vocab.chains.rootKicker,
          title: vocab.chains.reviewRootTitle,
          meta: fill(vocab.chains.reviewCountMeta, { count: totalReviews }),
          to: `/projects/${encodeURIComponent(primary.project_key)}?tab=acceptance`,
        },
        ...reviewProjects.slice(0, 3).map((p) => {
          const projRow = rowByKey.get(p.project_key);
          const health: 'ok' | 'risk' | 'off' | 'none' = projRow?.health === 'ok' || projRow?.health === 'risk' || projRow?.health === 'off'
            ? projRow.health
            : 'none';
          return {
            id: `live-review-proj-${p.project_key}`,
            kind: 'project' as const,
            kicker: vocab.chains.projectKicker,
            title: projRow?.title ?? p.project_key,
            meta: health === 'off' ? vocab.chains.projectOffTrack : health === 'risk' ? vocab.chains.projectAtRisk : null,
            health,
            to: `/projects/${encodeURIComponent(p.project_key)}`,
          };
        }),
      ],
    });
    void row;
  }

  const otherWaitProjects = rawProjects.filter((p): p is PortfolioProjectComplete =>
    isCompleteProject(p) && (p.waits.decision > 0 || p.waits.dependency > 0));
  if (otherWaitProjects.length > 0) {
    const totalWaits = otherWaitProjects.reduce((sum, p) => sum + p.waits.decision + p.waits.dependency, 0);
    const primary = otherWaitProjects[0];
    chains.push({
      id: 'live-open-waits',
      tab: vocab.chains.waitTab,
      footer: vocab.chains.waitFooter,
      cta: null,
      nodes: [
        {
          id: 'live-wait-root',
          kind: 'root',
          root: true,
          kicker: vocab.chains.rootKicker,
          title: vocab.chains.waitRootTitle,
          meta: fill(vocab.chains.waitCountMeta, { count: totalWaits }),
          to: `/projects/${encodeURIComponent(primary.project_key)}?tab=deps`,
        },
        ...otherWaitProjects.slice(0, 3).map((p) => {
          const projRow = rowByKey.get(p.project_key);
          const health: 'ok' | 'risk' | 'off' | 'none' = projRow?.health === 'ok' || projRow?.health === 'risk' || projRow?.health === 'off'
            ? projRow.health
            : 'none';
          return {
            id: `live-wait-proj-${p.project_key}`,
            kind: 'project' as const,
            kicker: vocab.chains.projectKicker,
            title: projRow?.title ?? p.project_key,
            meta: fill(vocab.chains.waitCountMeta, { count: p.waits.decision + p.waits.dependency }),
            health,
            to: `/projects/${encodeURIComponent(p.project_key)}`,
          };
        }),
      ],
    });
  }

  return chains;
}

function buildUsageNote(
  projects: readonly DepartmentProjectRow[],
  response: TeamPortfolioResponse,
  language: string,
): string | null {
  const vocab = departmentVocabulary(language);
  const usage = response.department_facts?.usage;
  const collected = (usage?.collected_groups ?? []).map((g) => g.trim()).filter(Boolean);
  let missing = (usage?.missing_groups ?? []).map((g) => g.trim()).filter(Boolean);

  // When usage facts are not supplied at all, infer missing group names from visible projects.
  if (!usage) {
    const allGroups = [...new Set(projects.map((p) => p.groupName).filter((g): g is string => Boolean(g)))];
    const projectsWithUsage = new Set(projects.filter((p) => p.usageDisplayText !== null).map((p) => p.groupName));
    missing = allGroups.filter((g) => !projectsWithUsage.has(g));
  }

  if (collected.length > 0 && missing.length > 0) {
    return fill(vocab.usage.partialGroups, {
      collected: collected.join(vocab.usage.separator),
      missing: missing.join(vocab.usage.separator),
    });
  }
  if (missing.length > 0) {
    return fill(vocab.usage.missingOnly, { missing: missing.join(vocab.usage.separator) });
  }
  if (projects.length > 0 && projects.every((p) => p.usageDisplayText === null)) {
    return vocab.usage.noneCollected;
  }
  return null;
}

export interface DepartmentModelOptions {
  readAtUnixMs?: number;
  referenceUnixMs?: number;
  timeZone?: string;
}

/** Build the complete read-only Department Projects model from a single portfolio response. */
export function buildDepartmentWorkbenchModel(
  response: TeamPortfolioResponse,
  language: string,
  options: DepartmentModelOptions = {},
): DepartmentWorkbenchModel {
  const vocab = departmentVocabulary(language);
  const timeZone = options.timeZone ?? 'UTC';
  const asOfUnixMs = response.result.data.as_of.at_unix_ms;
  const referenceUnixMs = typeof options.referenceUnixMs === 'number' && Number.isFinite(options.referenceUnixMs)
    ? options.referenceUnixMs
    : asOfUnixMs;

  const rawProjects = response.result.data.projects;
  const projects = rawProjects.map((item) => deriveProjectRow(item, response, referenceUnixMs, timeZone));
  const hiddenWorkTotal = rawProjects.reduce(
    (sum, item) => sum + (isCompleteProject(item) ? item.works.hidden_work_count : 0),
    0,
  );
  const coverage: DepartmentCoverage = {
    complete: response.coverage.status === 'complete' && !response.coverage.truncated && response.coverage.not_read_projects === 0,
    returnedProjects: response.coverage.returned_projects,
    totalProjects: response.coverage.total_projects,
    notReadProjects: response.coverage.not_read_projects,
    truncated: response.coverage.truncated,
    hiddenWorkTotal,
  };

  const facts = response.department_facts;
  const verdict = buildDepartmentVerdict(
    projects,
    coverage,
    facts?.department_update,
    facts?.viewer_person_id,
    language,
    options.readAtUnixMs,
    referenceUnixMs,
    timeZone,
  );

  // Metric 1: Projects
  const offCount = projects.filter((p) => p.health === 'off').length;
  const riskCount = projects.filter((p) => p.health === 'risk').length;
  const noUpdateCount = projects.filter((p) => p.health === 'none').length;
  let projectsNote: WorkbenchMetricNote | null = null;
  if (coverage.complete && projects.length > 0) {
    if (offCount > 0) {
      projectsNote = { text: fill(vocab.metrics.projectsBreakdownOff, { off: offCount }), tone: 'bad' };
    } else if (riskCount > 0 && noUpdateCount > 0) {
      projectsNote = {
        text: fill(vocab.metrics.projectsBreakdownRiskAndNoUpdate, { risk: riskCount, noUpdate: noUpdateCount }),
        tone: 'warn',
      };
    } else if (riskCount > 0) {
      projectsNote = { text: fill(vocab.metrics.projectsBreakdownRisk, { risk: riskCount }), tone: 'warn' };
    } else if (noUpdateCount > 0) {
      projectsNote = { text: fill(vocab.metrics.projectsBreakdownNoUpdate, { noUpdate: noUpdateCount }), tone: 'warn' };
    } else {
      projectsNote = { text: vocab.metrics.projectsBreakdownOk, tone: 'good' };
    }
  }
  const projectsMetric: DepartmentMetric<DepartmentProjectRow> = {
    id: 'projects',
    label: vocab.metrics.projects,
    unit: vocab.units.projects,
    count: coverage.truncated ? null : projects.length,
    note: projectsNote,
    emphasized: false,
    items: projects,
  };

  // Metric 2: Verified this month (vs previous month)
  const monthly = facts?.monthly_verified ?? null;
  let verifiedNote: WorkbenchMetricNote | null = null;
  if (monthly) {
    const prev = monthly.previous.count;
    const cur = monthly.current.count;
    verifiedNote = {
      text: fill(vocab.metrics.previousMonth, { count: prev }),
      ...(cur > prev ? { tone: 'good' as const, trend: 'up' as const }
        : cur < prev ? { tone: 'bad' as const, trend: 'down' as const } : {}),
    };
  }
  const verifiedMetric: DepartmentMetric<PortfolioMonthlyVerifiedItem> = {
    id: 'verified',
    label: vocab.metrics.verified,
    unit: vocab.units.tasks,
    count: monthly ? monthly.current.items.length : null,
    note: verifiedNote,
    emphasized: false,
    items: monthly ? monthly.current.items : [],
  };

  // Metric 3: Cross-group waits
  const waits = facts?.cross_group_waits ?? null;
  let waitsNote: WorkbenchMetricNote | null = null;
  if (waits && waits.length > 0 && waits.every((w) => typeof w.days === 'number' && Number.isFinite(w.days))) {
    const longest = waits.reduce((a, b) => ((a.days ?? -1) >= (b.days ?? -1) ? a : b));
    waitsNote = { text: fill(vocab.metrics.waitsOldest, { days: longest.days!, party: longest.on }) };
  }
  const waitsMetric: DepartmentMetric<PortfolioCrossGroupWait> = {
    id: 'waits',
    label: vocab.metrics.waits,
    unit: vocab.units.waits,
    count: waits ? waits.length : null,
    note: waitsNote,
    emphasized: false,
    items: waits ?? [],
  };

  // Metric 4: Waiting for you
  const pendingItems = facts?.pending ?? null;
  let pendingNote: WorkbenchMetricNote | null = null;
  if (pendingItems && pendingItems.length > 0) {
    const daysList = pendingItems.map((item) => item.days ?? elapsedWholeDays(item.since_unix_ms, referenceUnixMs));
    if (daysList.every((d): d is number => typeof d === 'number' && Number.isFinite(d))) {
      const oldest = Math.max(...daysList);
      pendingNote = {
        text: fill(pendingItems.length === 1 ? vocab.metrics.pendingSingle : vocab.metrics.pendingOldest, { days: oldest }),
      };
    }
  }
  const pendingMetric: DepartmentMetric<PortfolioPendingItem> = {
    id: 'pending',
    label: vocab.metrics.pending,
    unit: vocab.units.pending,
    count: pendingItems ? pendingItems.length : null,
    note: pendingNote,
    emphasized: true,
    items: pendingItems ?? [],
  };

  const chains = facts?.chains ?? deriveLivePortfolioChains(rawProjects, projects, language);
  const departmentName = facts?.department_name
    ?? projects.find((p) => p.departmentName)?.departmentName
    ?? null;

  return {
    departmentName,
    subtitle: zonedMonthLabel(referenceUnixMs, timeZone, language),
    verdict,
    metrics: [projectsMetric, verifiedMetric, waitsMetric, pendingMetric],
    chains,
    pending: {
      count: pendingItems ? pendingItems.length : null,
      items: pendingItems ?? [],
    },
    projects,
    waits,
    lessons: facts?.lessons ?? null,
    hasAnyProjectUsage: projects.some((p) => p.usageDisplayText !== null),
    usageNote: buildUsageNote(projects, response, language),
    coverage,
  };
}
