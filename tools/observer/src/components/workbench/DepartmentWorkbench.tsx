import { useRef, useState, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { useNavigate, useSearchParams } from 'react-router';
import type { ChainNode } from '../../core/overview';
import { absoluteTimeText } from '../../core/observation';
import { copyText, handbackVocabulary } from '../../lib/agent-handback';
import { PerspectiveLimitationNote } from '../../lib/perspective/PerspectiveLimitationNote';
import { projectPath } from '../../lib/project-path';
import type {
  PortfolioChainFact,
  PortfolioChainNodeFact,
  PortfolioCrossGroupWait,
  PortfolioMonthlyVerifiedItem,
  PortfolioPendingItem,
  TeamFreshness,
} from '../../lib/types';
import { FreshnessBadge } from '../FreshnessBadge';
import {
  Chain,
  CompletionBar,
  CompletionLegend,
  DataTable,
  DetailPanel,
  DisplayIcon,
  EmptyState,
  HealthTag,
  KpiStrip,
  Notice,
  PageHeader,
  RowList,
  StatusLamp,
  TodoRail,
  healthLampState,
  type DataTableColumn,
  type DataTableRow,
  type TodoItem,
} from '../display';
import {
  ArrowLeftIcon,
  ArrowRightIcon,
  CircleCheckIcon,
  CopyIcon,
  TerminalIcon,
} from '../icons/prototype-icons';
import {
  type DepartmentMetric,
  type DepartmentProjectRow,
  type DepartmentWorkbenchModel,
} from './department-model';
import { workbenchDisplayText } from './model';
import type { WorkbenchFrameState } from './WorkbenchFrame';
import './workbench.css';

interface DepartmentWorkbenchProps {
  model: DepartmentWorkbenchModel;
  freshness?: TeamFreshness;
  referenceUnixMs?: number;
  state: WorkbenchFrameState;
}

function validIndex(value: string | null, count: number): number {
  const parsed = Number(value ?? 0);
  return Number.isInteger(parsed) && parsed >= 0 && parsed < count ? parsed : 0;
}

function isGroupOrExternal(name: string): boolean {
  return name === '甲方' || name.endsWith('小组') || name.endsWith('改造') || name.endsWith('部');
}

function avatarInitial(name: string): string {
  if (name === '甲方') return '外';
  return name.slice(0, 1).toUpperCase();
}

function SmallAvatar({ name }: { name: string }) {
  const ext = isGroupOrExternal(name);
  return (
    <span className="obs-workbench-avatar-sm" data-external={ext || undefined} aria-hidden="true">
      {avatarInitial(name)}
    </span>
  );
}

/** Local read-only handback card for a department pending item. Never submits any action. */
function DepartmentHandback({ item }: { item: PortfolioPendingItem }) {
  const { t, i18n } = useTranslation();
  const [status, setStatus] = useState<'idle' | 'copied' | 'fallback'>('idle');
  const textRef = useRef<HTMLSpanElement | null>(null);
  const line = item.copy_text?.trim() || t('workbench.department.handbackFallback', {
    project: item.project_title ?? item.project_key,
    title: item.title,
  });
  const guidance = item.agent_guidance?.trim() || t('workbench.department.handbackGuidanceDefault');

  async function copyLine() {
    const result = await copyText(line);
    if (result.ok) {
      setStatus('copied');
      return;
    }
    if (textRef.current) {
      const selection = window.getSelection();
      const range = document.createRange();
      range.selectNodeContents(textRef.current);
      selection?.removeAllRanges();
      selection?.addRange(range);
    }
    setStatus('fallback');
  }

  return (
    <div className="obs-agent-handback" data-testid="agent-handback" data-language={i18n.language}>
      <div className="obs-agent-handback-title">
        <DisplayIcon icon={TerminalIcon} />
        <span>{t('workbench.home.handbackTitle')}</span>
      </div>
      <p>{guidance}</p>
      <div className="obs-agent-handback-line">
        <span ref={textRef}>{line}</span>
        <button type="button" onClick={copyLine}>
          <DisplayIcon icon={CopyIcon} />
          {t('workbench.home.handbackCopy')}
        </button>
      </div>
      <div className="obs-agent-handback-note">{t('workbench.home.handbackReadOnly')}</div>
      {status === 'idle' ? null : (
        <div className="obs-agent-handback-status" role="status">
          {status === 'copied' ? t('workbench.home.handbackCopied') : t('workbench.home.handbackCopyFailed')}
        </div>
      )}
    </div>
  );
}

/** Read-only Department Projects workbench (prototype VIEWS[2] & unitRows()). */
export function DepartmentWorkbench({ model, freshness, referenceUnixMs, state }: DepartmentWorkbenchProps) {
  const { t, i18n } = useTranslation();
  const navigate = useNavigate();
  const [searchParams, setSearchParams] = useSearchParams();
  const handbackWords = handbackVocabulary(i18n.language);

  const activeChainIndex = validIndex(searchParams.get('chain'), model.chains.length);
  const activeChain: PortfolioChainFact | undefined = model.chains[activeChainIndex];
  const activeTab = ['units', 'waits', 'lessons'].includes(searchParams.get('tab') ?? '')
    ? searchParams.get('tab')!
    : 'units';
  const detailMetric = model.metrics.find((metric) => metric.id === searchParams.get('detail') && metric.count !== null);

  function updateParams(values: Record<string, string | null>) {
    const next = new URLSearchParams(searchParams);
    for (const [key, value] of Object.entries(values)) {
      if (value === null) next.delete(key);
      else next.set(key, value);
    }
    setSearchParams(next);
  }

  function openPendingItem(item: PortfolioPendingItem) {
    const scope = encodeURIComponent(item.project_key);
    const suffix = item.id ? `&item=${encodeURIComponent(item.id)}` : '';
    navigate(`/inbox?scope=${scope}${suffix}`);
  }

  function projectMetaText(row: DepartmentProjectRow): string {
    if (row.leadName && row.groupName) return `${row.leadName} · ${row.groupName}`;
    if (row.leadName) return row.leadName;
    if (row.groupName) return row.groupName;
    return t('workbench.department.groupUnregistered');
  }

  function projectSummaryText(row: DepartmentProjectRow): string {
    const summary = row.summary;
    if (summary.kind === 'update') return summary.text.replace(/。$/, '');
    if (summary.kind === 'last-update') return t('workbench.department.lastUpdateDays', { days: summary.days });
    if (summary.kind === 'facts') {
      return summary.total === 0
        ? t('workbench.department.noTasksFact')
        : t('workbench.department.taskFactSummary', { total: summary.total, verified: summary.verified });
    }
    return t('workbench.department.projectNotRead');
  }

  function projectUpdatedText(row: DepartmentProjectRow): string {
    if (!row.readComplete) return t('workbench.department.updatedUnknown');
    if (row.updatedAtUnixMs === null) return t('workbench.department.noUpdateYet');
    if (row.elapsedDays === null) {
      return absoluteTimeText(row.updatedAtUnixMs, { language: i18n.language }).text;
    }
    return row.elapsedDays === 0
      ? t('workbench.department.updatedToday')
      : t('workbench.department.updatedDaysAgo', { days: row.elapsedDays });
  }

  // Convert portfolio chain nodes into core ChainNode shells so the shared Chain component renders them.
  const chainNodeMap = new Map<string, PortfolioChainNodeFact>();
  const chainNodes: ChainNode[] = (activeChain?.nodes ?? []).map((node) => {
    chainNodeMap.set(node.id, node);
    return {
      id: node.id,
      kind: node.kind === 'task' ? 'task' : node.kind === 'root' ? 'root' : 'commitment',
      root: Boolean(node.root),
      tone: node.root ? 'negative' : null,
      task: null,
      commitment: null,
    };
  });

  function presentChainNode(node: ChainNode) {
    const fact = chainNodeMap.get(node.id);
    if (!fact) return { title: '' };
    return {
      kicker: fact.kicker,
      title: workbenchDisplayText(fact.title, t('workbench.home.unnamedTask')),
      meta: fact.meta ? workbenchDisplayText(fact.meta, '') : null,
      status: fact.status ?? null,
      health: fact.health ?? null,
      ownerName: fact.owner_name ?? null,
      ownerExternal: fact.owner_external,
      queue: fact.queue,
      footNote: fact.foot_note ?? null,
      open: Boolean(fact.to),
    };
  }

  function openChainNode(node: ChainNode) {
    const fact = chainNodeMap.get(node.id);
    if (fact?.to) navigate(fact.to);
  }

  function pendingTodoItem(item: PortfolioPendingItem, index: number): TodoItem {
    const kindLabel = handbackWords.kind[item.kind] ?? handbackWords.kind.decision;
    const age = item.days !== null ? t('workbench.home.pendingAgeDays', { count: item.days }) : undefined;
    return {
      key: `${item.id}-${index}`,
      kind: kindLabel,
      title: workbenchDisplayText(item.title, t('workbench.home.unnamedPending')),
      detail: item.description ? workbenchDisplayText(item.description, '', 240) : undefined,
      from: item.from ? workbenchDisplayText(item.from, t('workbench.home.pendingFromUnknown')) : undefined,
      age,
      onOpen: () => openPendingItem(item),
    };
  }

  // Projects table (prototype unitRows())
  const unitColumns: DataTableColumn[] = [
    { key: 'project', title: t('workbench.department.unitColumns.project'), width: '24%' },
    { key: 'status', title: t('workbench.department.unitColumns.status'), width: 'var(--obs-dept-col-status)' },
    { key: 'sentence', title: t('workbench.department.unitColumns.sentence') },
    { key: 'completion', title: t('workbench.department.unitColumns.completion'), width: 'var(--obs-dept-col-completion)' },
    ...(model.hasAnyProjectUsage ? [{
      key: 'usage',
      title: t('workbench.department.unitColumns.usage'),
      width: 'var(--obs-dept-col-usage)',
      align: 'right' as const,
      hideOnSmall: true,
      numeric: true,
    }] : []),
    {
      key: 'updated',
      title: t('workbench.department.unitColumns.updated'),
      width: 'var(--obs-dept-col-updated)',
      align: 'right',
      hideOnSmall: true,
    },
  ];

  const unitRows: DataTableRow[] = model.projects.map((row) => ({
    key: row.projectKey,
    highlighted: row.health === 'risk' || row.health === 'off',
    onOpen: () => navigate(projectPath(row.projectKey)),
    cells: [
      <div className="obs-workbench-unit-cell">
        <div className="obs-workbench-unit-name">{row.title}</div>
        <div className="obs-workbench-unit-meta">{projectMetaText(row)}</div>
      </div>,
      row.health === 'unknown' ? (
        <span className="obs-tag" title={t('workbench.department.statusUnknownMeaning')}>
          <StatusLamp lamp="none" />
          {t('workbench.department.statusUnknown')}
        </span>
      ) : (
        <HealthTag status={row.health} />
      ),
      projectSummaryText(row),
      row.completion ? (
        <CompletionBar counts={row.completion} />
      ) : (
        <span className="obs-workbench-dim">{t('workbench.department.completionNotRead')}</span>
      ),
      ...(model.hasAnyProjectUsage ? [
        row.usageDisplayText
          ? <span className="obs-workbench-unit-usage">{row.usageDisplayText}</span>
          : <span className="obs-workbench-dim">{t('workbench.department.usageNotCollected')}</span>,
      ] : []),
      <div className="obs-workbench-unit-updated" data-late={row.late || undefined}>
        <div className="obs-workbench-unit-time">{projectUpdatedText(row)}</div>
        {row.late ? <div className="obs-workbench-unit-late">{t('workbench.department.needsUpdate')}</div> : null}
      </div>,
    ],
  }));

  // Cross-group waits table (prototype waitsTable())
  const waitItems = model.waits ?? [];
  const longestWait = Math.max(0, ...waitItems.map((wait) => wait.days ?? 0));
  const waitRows: DataTableRow[] = waitItems.map((wait) => ({
    key: wait.id,
    onOpen: wait.task_id
      ? () => navigate(`/tasks/${encodeURIComponent(wait.task_id!)}?scope=${encodeURIComponent(wait.project_key)}`)
      : () => navigate(projectPath(wait.project_key)),
    cells: [
      <span className="obs-workbench-wait-people">
        <SmallAvatar name={wait.who} />
        <span className="obs-workbench-wait-person">{wait.who}</span>
        <DisplayIcon icon={ArrowRightIcon} />
        <SmallAvatar name={wait.on} />
        <span className="obs-workbench-wait-person">{wait.on}</span>
      </span>,
      workbenchDisplayText(wait.what, t('workbench.home.waitWhatFallback')),
      <span className="obs-tag">{t(`workbench.home.waitKinds.${wait.kind}`)}</span>,
      wait.days === null ? t('workbench.home.durationUnknown') : (
        <span className="obs-workbench-wait-meter" data-hot={wait.days >= 5 || undefined}>
          {longestWait > 0 ? (
            <span className="obs-workbench-wait-bar">
              <i style={{ width: `${Math.round((100 * wait.days) / longestWait)}%` }} />
            </span>
          ) : null}
          <span>{t('workbench.home.durationDays', { count: wait.days })}</span>
        </span>
      ),
      wait.behind === null ? t('workbench.home.countUnknown') : t('workbench.home.behindCount', { count: wait.behind }),
    ],
  }));

  // Lessons table (prototype learnTable())
  const lessonRows: DataTableRow[] = (model.lessons ?? []).map((lesson) => {
    const stageText = t(`workbench.department.lessonStages.${lesson.stage}`);
    const authorText = lesson.author_name
      ? t('workbench.department.lessonBy', { stage: stageText, author: lesson.author_name })
      : stageText;
    const usageText = lesson.used === 0
      ? null
      : lesson.not_applicable > 0
        ? t('workbench.department.lessonUsageWithNotApplicable', {
          used: lesson.used,
          passed: lesson.passed,
          notApplicable: lesson.not_applicable,
        })
        : t('workbench.department.lessonUsage', { used: lesson.used, passed: lesson.passed });
    return {
      key: lesson.id,
      onOpen: () => navigate(`/lessons?item=${encodeURIComponent(lesson.id)}`),
      cells: [
        <div className="obs-workbench-unit-cell">
          <div className="obs-workbench-unit-name">{workbenchDisplayText(lesson.title, t('workbench.department.unnamedLesson'))}</div>
          <div className="obs-workbench-unit-meta">{authorText}</div>
        </div>,
        lesson.why,
        lesson.used === 0 ? (
          <span className="obs-workbench-dim">{t('workbench.department.lessonNotUsed')}</span>
        ) : (
          <span className="obs-workbench-lesson-result">
            <span
              className="obs-workbench-lesson-dots"
              role="img"
              aria-label={t('workbench.department.lessonUsedLabel', { count: lesson.used })}
            >
              {Array.from({ length: lesson.used }, (_, dotIndex) => (
                <i
                  key={dotIndex}
                  data-no={dotIndex >= lesson.used - lesson.not_applicable || undefined}
                />
              ))}
            </span>
            <span className="obs-workbench-lesson-usage">{usageText}</span>
          </span>
        ),
      ],
    };
  });

  function metricDetailRow(metric: DepartmentMetric, item: unknown, index: number) {
    if (metric.id === 'projects') {
      const row = item as DepartmentProjectRow;
      return {
        key: `project-${row.projectKey}-${index}`,
        title: row.title,
        meta: `${projectMetaText(row)} · ${projectSummaryText(row)}`,
        leading: row.health === 'unknown' ? <StatusLamp lamp="none" /> : <HealthTag status={row.health} />,
        onOpen: () => navigate(projectPath(row.projectKey)),
      };
    }
    if (metric.id === 'verified') {
      const verified = item as PortfolioMonthlyVerifiedItem;
      const metaParts = [verified.project_title ?? verified.project_key, verified.owner_name].filter(Boolean);
      return {
        key: `verified-${verified.id}-${index}`,
        title: workbenchDisplayText(verified.title, t('workbench.home.unnamedTask')),
        meta: metaParts.join(' · '),
        onOpen: () => navigate(`/tasks/${encodeURIComponent(verified.id)}?scope=${encodeURIComponent(verified.project_key)}`),
      };
    }
    if (metric.id === 'waits') {
      const wait = item as PortfolioCrossGroupWait;
      return {
        key: `wait-${wait.id}-${index}`,
        title: `${wait.who} → ${wait.on}`,
        meta: `${workbenchDisplayText(wait.what, t('workbench.home.waitWhatFallback'))}${wait.days !== null ? ` · ${t('workbench.home.durationDays', { count: wait.days })}` : ''}`,
        onOpen: wait.task_id
          ? () => navigate(`/tasks/${encodeURIComponent(wait.task_id!)}?scope=${encodeURIComponent(wait.project_key)}`)
          : () => navigate(projectPath(wait.project_key)),
      };
    }
    const pending = item as PortfolioPendingItem;
    return {
      key: `pending-${pending.id}-${index}`,
      title: workbenchDisplayText(pending.title, t('workbench.home.unnamedPending')),
      meta: pending.from ?? pending.project_title ?? t('workbench.home.taskOpenFallback'),
      onOpen: () => openPendingItem(pending),
    };
  }

  let detailContent: ReactNode;
  if (detailMetric) {
    detailContent = (
      <section
        className="obs-detail-panel obs-workbench-metric-detail"
        data-testid="workbench-metric-detail"
        data-count={detailMetric.items.length}
      >
        <div className="obs-workbench-detail-head">
          <button type="button" className="obs-workbench-back" onClick={() => updateParams({ detail: null })}>
            <DisplayIcon icon={ArrowLeftIcon} />
            {t('workbench.home.metricDetailBack')}
          </button>
          <h2>{detailMetric.label}</h2>
          <span>{detailMetric.count}{detailMetric.unit}</span>
        </div>
        <RowList
          items={detailMetric.items.map((item, index) => metricDetailRow(detailMetric, item, index))}
          empty={<EmptyState title={t('workbench.home.metricDetailEmpty')} />}
        />
      </section>
    );
  } else {
    detailContent = (
      <DetailPanel
        activeKey={activeTab}
        onChange={(tab) => updateParams({ tab, detail: null })}
        extra={activeTab === 'units' ? <CompletionLegend /> : undefined}
        tabs={[
          {
            key: 'units',
            label: t('workbench.department.tabs.projects'),
            count: model.projects.length,
            content: (
              <DataTable
                columns={unitColumns}
                rows={unitRows}
                empty={(
                  <EmptyState
                    title={t('workbench.department.emptyProjectsTitle')}
                    description={t('workbench.department.emptyProjectsDescription')}
                  />
                )}
              />
            ),
            footer: model.usageNote ? (
              <span data-testid="dept-usage-note">{model.usageNote}</span>
            ) : undefined,
          },
          {
            key: 'waits',
            label: t('workbench.department.tabs.waits'),
            count: model.waits !== null ? model.waits.length : undefined,
            content: model.waits === null ? (
              <EmptyState
                title={t('workbench.department.waitsUnreadTitle')}
                description={t('workbench.department.waitsUnreadDescription')}
              />
            ) : (
              <DataTable
                columns={[
                  { key: 'who', title: t('workbench.home.waitColumns.who'), width: '32%' },
                  { key: 'what', title: t('workbench.home.waitColumns.what') },
                  { key: 'kind', title: t('workbench.home.waitColumns.kind'), width: 'var(--craft-workbench-column-wait-kind)', hideOnSmall: true },
                  { key: 'duration', title: t('workbench.home.waitColumns.duration'), width: 'var(--craft-workbench-column-wait-duration)' },
                  { key: 'behind', title: t('workbench.home.waitColumns.behind'), width: 'var(--craft-workbench-column-behind)', align: 'right', hideOnSmall: true, numeric: true },
                ]}
                rows={waitRows}
                empty={<EmptyState title={t('workbench.home.noWaits')} />}
              />
            ),
          },
          {
            key: 'lessons',
            label: t('workbench.department.tabs.lessons'),
            content: model.lessons === null ? (
              <EmptyState
                title={t('workbench.home.lessonsEmptyTitle')}
                description={t('workbench.home.lessonsEmptyDescription')}
              />
            ) : (
              <DataTable
                columns={[
                  { key: 'lesson', title: t('workbench.department.lessonColumns.lesson') },
                  { key: 'why', title: t('workbench.department.lessonColumns.why'), width: 'var(--obs-dept-col-lesson-why)' },
                  { key: 'result', title: t('workbench.department.lessonColumns.result'), width: '30%', hideOnSmall: true },
                ]}
                rows={lessonRows}
                empty={<EmptyState title={t('workbench.department.emptyLessons')} />}
              />
            ),
          },
        ]}
      />
    );
  }

  const partialParts: string[] = [];
  if (model.coverage.notReadProjects > 0) {
    partialParts.push(t('workbench.department.partialNotRead', {
      notRead: model.coverage.notReadProjects,
      total: model.coverage.totalProjects,
    }));
  }
  if (model.coverage.truncated) {
    partialParts.push(t('workbench.department.partialTruncated', {
      returned: model.coverage.returnedProjects,
      total: model.coverage.totalProjects,
    }));
  }

  const showBasisAction = Boolean(model.verdict.updateId) || model.verdict.source === 'generated';

  return (
    <div
      className="obs-workbench"
      data-testid="workbench-frame"
      data-perspective="dept"
      data-state={state}
      data-reference-clock={referenceUnixMs}
    >
      <div data-block="heading">
        <PageHeader title={t('workbench.department.heading')} subtitle={model.subtitle} />
        <PerspectiveLimitationNote />
        {state === 'unknown' ? (
          <div data-testid="workbench-ordering-unknown">
            <Notice tone="warning" title={t('workbench.home.unknownTitle')}>
              {t('workbench.home.unknownDescription')}
            </Notice>
          </div>
        ) : null}
        {partialParts.length > 0 ? (
          <div data-testid="workbench-partial">
            <Notice tone="warning" title={t('workbench.home.partialTitle')}>
              {partialParts.join(' ')}
            </Notice>
          </div>
        ) : null}
      </div>
      <div data-block="freshness" className="obs-workbench-freshness">
        <FreshnessBadge freshness={freshness} />
        <span className="obs-workbench-coverage-line" data-testid="dept-read-coverage">
          {model.coverage.complete
            ? t('workbench.department.coverageComplete', { total: model.coverage.totalProjects })
            : t('workbench.department.coveragePartial', {
              returned: Math.max(0, model.coverage.returnedProjects - model.coverage.notReadProjects),
              total: model.coverage.totalProjects,
            })}
          {model.coverage.hiddenWorkTotal > 0
            ? ` · ${t('workbench.department.coverageHiddenTasks', { count: model.coverage.hiddenWorkTotal })}`
            : ''}
        </span>
      </div>
      <section
        className="obs-workbench-instrument"
        data-block="instrument"
        aria-label={t('workbench.department.heading')}
      >
        <div className="obs-workbench-verdict-top" data-health={model.verdict.health}>
          <div className="obs-workbench-verdict">
            <span
              className="obs-workbench-health-word"
              data-health={model.verdict.health}
              title={t(`display.healthStatus.${model.verdict.health}.meaning`)}
            >
              <StatusLamp lamp={healthLampState(model.verdict.health)} />
              {model.verdict.healthLabel}
            </span>
            <p className="obs-workbench-sentence">{model.verdict.sentence}</p>
            <div className="obs-workbench-byline">{model.verdict.byline}</div>
            {searchParams.get('basis') === '1' ? (
              <div className="obs-workbench-verdict-basis" data-testid="workbench-verdict-basis">
                {model.verdict.source === 'generated'
                  ? t('workbench.department.verdictGeneratedBasis')
                  : t('workbench.home.verdictUpdateBasis')}
              </div>
            ) : null}
          </div>
          {showBasisAction ? (
            <div className="obs-workbench-verdict-actions">
              <button
                type="button"
                className="obs-workbench-evidence-button"
                onClick={() => model.verdict.updateId
                  ? navigate(`/updates/${encodeURIComponent(model.verdict.updateId)}`)
                  : updateParams({ basis: searchParams.get('basis') === '1' ? null : '1' })}
              >
                {searchParams.get('basis') === '1' && !model.verdict.updateId
                  ? t('workbench.home.closeBasis')
                  : t('workbench.home.verdictBasis')}
                <DisplayIcon icon={ArrowRightIcon} />
              </button>
            </div>
          ) : null}
        </div>
        <KpiStrip
          items={model.metrics.map((metric) => ({
            key: metric.id,
            label: metric.label,
            value: metric.count === null ? '—' : String(metric.count),
            unit: metric.count === null ? undefined : (metric.unit || undefined),
            change: metric.note === null ? undefined : {
              text: metric.note.text,
              tone: metric.note.tone,
              trend: metric.note.trend,
            },
            emphasized: metric.emphasized,
            onOpen: metric.count === null ? undefined : () => updateParams({ detail: metric.id, tab: null }),
          }))}
        />
        <div className="obs-workbench-body">
          <Chain
            nodes={chainNodes}
            header={(
              <div className="obs-workbench-chain-head">
                <h2>{t('workbench.home.chainTitle')}</h2>
                {model.chains.length > 0 ? (
                  <span className="obs-tag">{t('workbench.home.chainCount', { count: model.chains.length })}</span>
                ) : null}
                {model.chains.length > 0 ? (
                  <div className="obs-workbench-chain-switch" role="group" aria-label={t('workbench.home.chainGroup')}>
                    {model.chains.map((chain, index) => (
                      <button
                        key={chain.id}
                        type="button"
                        aria-pressed={index === activeChainIndex}
                        onClick={() => updateParams({ chain: String(index) })}
                      >
                        {chain.tab}
                      </button>
                    ))}
                  </div>
                ) : null}
              </div>
            )}
            empty={(
              <div className="obs-workbench-calm">
                <DisplayIcon icon={CircleCheckIcon} />
                <div>
                  <strong>{t('workbench.home.emptyChainTitle')}</strong>
                  <p>{t('workbench.home.emptyChainDescription')}</p>
                </div>
              </div>
            )}
            present={presentChainNode}
            onOpen={openChainNode}
            footer={activeChain ? (
              <>
                <span>{activeChain.footer}</span>
                {activeChain.cta ? (
                  <button
                    type="button"
                    className="obs-workbench-chain-cta"
                    onClick={() => navigate(activeChain.cta!.to)}
                  >
                    {activeChain.cta.label}
                    <DisplayIcon icon={ArrowRightIcon} />
                  </button>
                ) : null}
              </>
            ) : undefined}
          />
          <TodoRail
            title={t('workbench.home.railTitle')}
            items={model.pending.items.map(pendingTodoItem)}
            count={model.pending.count}
            onFirstAgent={() => updateParams({ agent: searchParams.get('agent') === '1' ? null : '1' })}
            firstAccessory={searchParams.get('agent') === '1' && model.pending.items[0]
              ? <DepartmentHandback item={model.pending.items[0]} />
              : undefined}
            onViewAll={() => navigate('/inbox')}
            empty={model.pending.count === null ? (
              <EmptyState
                title={t('workbench.home.railUnknownTitle')}
                description={t('workbench.home.railUnknownDescription')}
              />
            ) : (
              <EmptyState
                title={t('workbench.home.railEmptyTitle')}
                description={t('workbench.home.railEmptyDescription')}
              />
            )}
          />
        </div>
      </section>
      <div data-block="detail">{detailContent}</div>
    </div>
  );
}
