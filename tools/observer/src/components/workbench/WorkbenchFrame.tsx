import { ArrowLeft, ArrowRight, Bot, CheckCircle2 } from 'lucide-react';
import { useMemo, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { useNavigate, useSearchParams } from 'react-router';
import type {
  ChainNode,
  DerivedChain,
  DerivedTask,
  DerivedWait,
  OverviewCommitment,
  PersonPendingItem,
  ViewAgentSummary,
} from '../../core/overview';
import { handbackVocabulary } from '../../lib/agent-handback';
import { PerspectiveLimitationNote } from '../../lib/perspective/PerspectiveLimitationNote';
import type { TeamFreshness } from '../../lib/types';
import { FreshnessBadge } from '../FreshnessBadge';
import {
  Chain,
  DataTable,
  DetailPanel,
  DisplayIcon,
  EmptyState,
  KpiStrip,
  Notice,
  PageHeader,
  RowList,
  StatusLamp,
  TaskStatusTag,
  TodoRail,
  healthLampState,
  type DataTableRow,
  type TodoItem,
} from '../display';
import { AgentHandback } from './AgentHandback';
import {
  metricDetailItems,
  reportAge,
  reportDeclaration,
  waitingOnText,
  workbenchAgentName,
  workbenchDisplayText,
  type MyWorkModel,
  type ReportDeclaration,
  type WorkbenchMetric,
} from './model';
import './workbench.css';

export type WorkbenchFrameState = 'data' | 'empty' | 'partial' | 'unknown';

interface WorkbenchFrameProps {
  model: MyWorkModel;
  projectKey: string;
  projectName: string;
  /** Display names of the projects the viewer can read, keyed by project key. */
  projectTitles?: Readonly<Record<string, string>>;
  freshness?: TeamFreshness;
  readAtUnixMs?: number;
  state?: WorkbenchFrameState;
}

function isTask(item: unknown): item is DerivedTask {
  return Boolean(item && typeof item === 'object' && 'status' in item && 'agentActive' in item);
}

function isWait(item: unknown): item is DerivedWait {
  return Boolean(item && typeof item === 'object' && 'kind' in item && 'task' in item && 'behind' in item);
}

function isAgent(item: unknown): item is ViewAgentSummary {
  return Boolean(item && typeof item === 'object' && 'agentName' in item && 'tasks' in item);
}

function isPending(item: unknown): item is PersonPendingItem {
  return Boolean(item && typeof item === 'object' && 'kind' in item && 'task' in item && 'wait' in item && 'handoff' in item && 'approval' in item);
}

function validChain(value: string | null, count: number): number {
  const parsed = Number(value ?? 0);
  return Number.isInteger(parsed) && parsed >= 0 && parsed < count ? parsed : 0;
}

/** The four perspectives reuse this frame; this PR supplies only the My Work configuration. */
export function WorkbenchFrame({ model, projectKey, projectName, projectTitles, freshness, readAtUnixMs, state }: WorkbenchFrameProps) {
  const { t, i18n } = useTranslation();
  const navigate = useNavigate();
  const [searchParams, setSearchParams] = useSearchParams();
  const handbackWords = handbackVocabulary(i18n.language);
  const activeChainIndex = validChain(searchParams.get('chain'), model.chains.length);
  const activeChain = model.chains[activeChainIndex];
  const activeTab = ['agents', 'waits', 'lessons'].includes(searchParams.get('tab') ?? '')
    ? searchParams.get('tab')!
    : 'agents';
  const detailMetric = model.metrics.find((metric) => metric.id === searchParams.get('detail') && metric.count !== null);
  const empty = model.metrics.every((metric) => metric.count === 0) && model.agentTasks.length === 0 && model.chains.length === 0;
  const resolvedState = state ?? (empty ? 'empty' : 'data');
  const scopeQuery = `scope=${encodeURIComponent(projectKey)}`;

  /** What the reporter declared, in the shared vocabulary; the prose stays in the backend. */
  function reportDeclarationText(declaration: ReportDeclaration): string {
    if (declaration === 'done') return t('workbench.home.reportDone');
    if (declaration === 'inProgress') return t('workbench.home.reportInProgress');
    if (declaration === 'noDeclaration') return t('workbench.home.reportNoDeclaration');
    return t('workbench.home.reportNotRead');
  }

  function reportAgeText(age: ReturnType<typeof reportAge>): string {
    if (age.kind === 'minutes') return t('workbench.home.reportAgeMinutes', { count: age.count });
    if (age.kind === 'hours') return t('workbench.home.reportAgeHours', { count: age.count });
    if (age.kind === 'days') return t('workbench.home.reportAgeDays', { count: age.count });
    if (age.kind === 'justNow') return t('workbench.home.reportAgeJustNow');
    return t('workbench.home.reportNotRead');
  }

  /** A commitment's project key, taken from the impact task that carries it — never parsed
   * from the commitment's own id, which is a record key, not a display name. */
  const commitmentProjects = useMemo(() => {
    const map = new Map<string, string>();
    if (!activeChain) return map;
    for (const task of activeChain.impact.items) {
      if (task.projectId == null || task.commitments == null) continue;
      for (const commitment of task.commitments) {
        if (commitment.id != null && !map.has(commitment.id)) map.set(commitment.id, task.projectId);
      }
    }
    return map;
  }, [activeChain]);

  function commitmentTitle(commitment: OverviewCommitment | null): string {
    if (commitment?.kind === 'goal') return t('workbench.home.commitmentGoalFallback');
    const projectKeyOfCommitment = commitment?.id != null ? commitmentProjects.get(commitment.id) : undefined;
    const title = projectKeyOfCommitment != null ? projectTitles?.[projectKeyOfCommitment] : undefined;
    return workbenchDisplayText(title ?? null, t('workbench.home.commitmentProjectFallback'));
  }

  function updateParams(values: Record<string, string | null>) {
    const next = new URLSearchParams(searchParams);
    for (const [key, value] of Object.entries(values)) {
      if (value === null) next.delete(key);
      else next.set(key, value);
    }
    setSearchParams(next);
  }

  function openTask(task: DerivedTask | null) {
    if (task?.id) navigate(`/tasks/${encodeURIComponent(task.id)}?${scopeQuery}`);
  }

  function openPending(item: PersonPendingItem) {
    if (item.kind === 'wait' && item.wait && !['decision', 'review'].includes(item.wait.kind)) {
      openTask(item.task);
      return;
    }
    const source = item.wait?.sourceId ?? item.handoff?.id ?? item.approval?.id;
    const suffix = source ? `&item=${encodeURIComponent(source)}` : '';
    navigate(`/inbox?${scopeQuery}${suffix}`);
  }

  function openCommitment(chain: DerivedChain, commitment: OverviewCommitment | null) {
    if (!commitment?.id) return;
    if (commitment.kind === 'goal') {
      navigate(`/goals/${encodeURIComponent(commitment.id)}?${scopeQuery}`);
      return;
    }
    const task = chain.impact.items.find((candidate) => candidate.projectId
      && candidate.commitments?.some((item) => item.id === commitment.id));
    if (task?.projectId) navigate(`/projects/${encodeURIComponent(task.projectId)}?tab=plan`);
  }

  function chainNodeView(chain: DerivedChain, node: ChainNode) {
    if (node.root) {
      const root = chain.root;
      const knownPerson = root.person?.displayName?.trim()
        ? workbenchDisplayText(root.person.displayName, t('workbench.home.rootReviewUnknown'))
        : null;
      const firstWait = root.queue.items[0];
      let title = t('workbench.home.rootDecisionUnknown');
      if (root.kind === 'review') {
        title = knownPerson
          ? t('workbench.home.rootReviewQueue', { name: knownPerson })
          : t('workbench.home.rootReviewUnknown');
      } else if (root.kind === 'external') {
        title = root.externalParty
          ? t('workbench.home.rootExternal', {
            party: workbenchDisplayText(root.externalParty, t('workbench.home.rootExternalPartyFallback')),
          })
          : t('workbench.home.rootExternalUnknown');
      } else if (root.queue.items.some((wait) => wait.waitingOn.some((person) => person.id === model.currentPerson?.person.id))) {
        title = t('workbench.home.rootDecisionYou');
      } else if (knownPerson) {
        title = t('workbench.home.rootDecisionOther', { name: knownPerson });
      }
      return {
        kicker: t('display.chain.rootKicker'),
        title,
        meta: workbenchDisplayText(firstWait?.what, t('workbench.home.rootWhatFallback')),
        ownerName: knownPerson ?? root.externalParty,
        you: root.queue.items.some((wait) => wait.waitingOn.some((person) => person.id === model.currentPerson?.person.id)),
        queue: root.queue.items.map((wait) => ({
          title: workbenchDisplayText(wait.task.title, t('workbench.home.unnamedTask')), days: wait.durationDays,
        })),
        queueNote: root.queue.count === null ? null : { queued: root.queue.count, oldestDays: root.oldestDays },
        footNote: root.queue.count === null ? t('workbench.home.countUnknown') : null,
        open: root.person?.id != null || firstWait?.task.id != null,
      };
    }
    if (node.task) {
      return {
        kicker: t('display.chain.taskKicker'),
        title: workbenchDisplayText(node.task.title, t('workbench.home.unnamedTask')),
        meta: workbenchDisplayText(node.task.owner?.displayName, t('workbench.home.currentMemberUnknown')),
        status: node.task.status,
        ownerName: node.task.owner?.displayName,
        you: node.task.owner?.id === model.currentPerson?.person.id,
        open: node.task.id != null,
      };
    }
    return {
      kicker: t(node.commitment?.kind === 'goal' ? 'display.chain.goalKicker' : 'display.chain.projectKicker'),
      title: commitmentTitle(node.commitment),
      open: node.commitment?.id != null,
    };
  }

  function openChainNode(chain: DerivedChain, node: ChainNode) {
    if (node.root) {
      if (chain.root.kind === 'review' && chain.root.person?.id) {
        navigate(`/members/${encodeURIComponent(chain.root.person.id)}?${scopeQuery}`);
      } else if (chain.root.kind === 'decision') {
        const first = chain.root.queue.items[0];
        if (first) openPending({ kind: 'wait', task: first.task, wait: first, handoff: null, approval: null });
      } else {
        openTask(chain.root.queue.items[0]?.task ?? null);
      }
    } else if (node.task) openTask(node.task);
    else openCommitment(chain, node.commitment);
  }

  function pendingView(item: PersonPendingItem, index: number): TodoItem {
    const kind = item.kind === 'approval'
      ? handbackWords.kind[item.approval?.kind === 'lesson-confirmation' ? 'lesson' : 'plan-approval']
      : item.kind === 'handoff'
        ? handbackWords.kind.assignment
        : item.wait?.kind === 'review'
          ? t('workbench.home.waitKinds.review')
          : handbackWords.kind.decision;
    const title = workbenchDisplayText(item.approval?.title ?? item.task?.title, t('workbench.home.unnamedPending'));
    const ownerName = item.task?.owner?.displayName;
    return {
      key: `${item.kind}-${index}`,
      kind,
      title,
      detail: workbenchDisplayText(
        item.wait?.what,
        item.wait?.kind === 'review' ? t('workbench.home.waitReviewFallback') : title,
      ),
      from: ownerName
        ? t('workbench.home.pendingFrom', { name: workbenchDisplayText(ownerName, t('workbench.home.pendingFromUnknown')) })
        : t('workbench.home.pendingFromUnknown'),
      age: item.wait?.durationDays == null ? undefined : t('workbench.home.pendingAgeDays', { count: item.wait.durationDays }),
      onOpen: () => openPending(item),
    };
  }

  const coverageText: string[] = [];
  if (model.coverage.partial.length > 0) {
    coverageText.push(t('workbench.home.coveragePartial', {
      parts: model.coverage.partial.map((part) => t(`workbench.home.coverageParts.${part}`)).join(t('common.listSeparator')),
    }));
  }
  if (model.coverage.unread.length > 0) {
    coverageText.push(t('workbench.home.coverageUnread', {
      parts: model.coverage.unread.map((part) => t(`workbench.home.coverageParts.${part}`)).join(t('common.listSeparator')),
    }));
  }
  if (model.coverage.incompleteFacts && model.coverage.partial.length === 0 && model.coverage.unread.length === 0) {
    coverageText.push(t('workbench.home.coverageIncomplete'));
  }
  const coverageWarning = model.coverage.partial.length > 0
    || (model.coverage.unread.length === 0 && model.coverage.incompleteFacts);

  const waitItems = model.metrics[2].details.items.filter(isWait);
  const longestWait = Math.max(0, ...waitItems.map((wait) => wait.durationDays ?? 0));
  const waitRows: DataTableRow[] = waitItems.map((wait, index) => {
    const owner = workbenchDisplayText(wait.task.owner?.displayName, t('workbench.home.currentMemberUnknown'));
    const waitingOn = waitingOnText(wait, i18n.language);
    const what = workbenchDisplayText(
      wait.what ?? wait.providerTask?.title,
      wait.kind === 'review' ? t('workbench.home.waitReviewFallback') : t('workbench.home.waitWhatFallback'),
    );
    return {
      key: `${wait.kind}-${wait.task.id ?? index}-${index}`,
      cells: [
        <span className="obs-workbench-wait-people"><span>{owner}</span><DisplayIcon icon={ArrowRight} /><span>{waitingOn}</span></span>,
        what,
        <span className="obs-tag">{t(`workbench.home.waitKinds.${wait.kind}`)}</span>,
        wait.durationDays === null ? t('workbench.home.durationUnknown') : (
          <span className="obs-workbench-wait-meter" data-hot={wait.durationDays >= 5 || undefined}>
            {longestWait > 0 ? <span className="obs-workbench-wait-bar"><i style={{ width: `${Math.round(100 * wait.durationDays / longestWait)}%` }} /></span> : null}
            <span>{t('workbench.home.durationDays', { count: wait.durationDays })}</span>
          </span>
        ),
        wait.behind.count === null ? t('workbench.home.countUnknown') : t('workbench.home.behindCount', { count: wait.behind.count }),
      ],
      onOpen: wait.task.id ? () => openTask(wait.task) : undefined,
      highlighted: wait.durationDays !== null && wait.durationDays >= 5,
    };
  });

  const agentRows: DataTableRow[] = model.agentTasks.map((task, index) => {
    const declaration = reportDeclaration(task.report);
    const age = reportAge(task.report?.reportedAtUnixMs, readAtUnixMs);
    return {
      key: task.id ?? `task-${index}`,
      cells: [
        task.agentName
          ? (
            <span className="obs-workbench-agent-name">
              <span className="obs-workbench-agent-badge" aria-hidden="true"><DisplayIcon icon={Bot} /></span>
              <span>{workbenchAgentName(task.agentName, t('workbench.home.noAgent'))}</span>
            </span>
          )
          : <span className="obs-workbench-dim">{t('workbench.home.noAgent')}</span>,
        workbenchDisplayText(task.title, t('workbench.home.unnamedTask')),
        declaration === 'unknown'
          ? <span className="obs-workbench-dim">{reportDeclarationText(declaration)}</span>
          : reportDeclarationText(declaration),
        <TaskStatusTag status={task.status} live={task.agentActive === true} />,
        <span className="obs-workbench-dim">{reportAgeText(age)}</span>,
      ],
      onOpen: task.id ? () => openTask(task) : undefined,
    };
  });

  function metricRow(item: unknown, metric: WorkbenchMetric, index: number) {
    if (metric.id === 'agents' && isAgent(item)) {
      const taskNames = item.tasks.map((task) => workbenchDisplayText(task.title, '')).filter(Boolean);
      return {
        key: `agent-${index}`,
        title: workbenchAgentName(item.agentName, t('workbench.home.unnamedAgent')),
        meta: taskNames.length > 0
          ? t('workbench.home.metricAgentTasks', { tasks: taskNames.join(t('common.listSeparator')) })
          : t('workbench.home.metricNoTasks'),
        leading: <DisplayIcon icon={Bot} />,
        onOpen: item.tasks[0]?.id ? () => openTask(item.tasks[0]) : undefined,
      };
    }
    if (isWait(item)) {
      return {
        key: `wait-${index}`,
        title: workbenchDisplayText(item.task.title, t('workbench.home.unnamedTask')),
        meta: `${workbenchDisplayText(item.task.owner?.displayName, t('workbench.home.currentMemberUnknown'))} → ${waitingOnText(item, i18n.language)} · ${workbenchDisplayText(item.what, t('workbench.home.waitWhatFallback'))}`,
        onOpen: item.task.id ? () => openTask(item.task) : undefined,
      };
    }
    if (isPending(item)) {
      return {
        key: `pending-${index}`,
        title: workbenchDisplayText(item.approval?.title ?? item.task?.title, t('workbench.home.unnamedPending')),
        meta: workbenchDisplayText(item.wait?.what, t('workbench.home.taskOpenFallback')),
        onOpen: () => openPending(item),
      };
    }
    if (isTask(item)) {
      return {
        key: `task-${index}`,
        title: workbenchDisplayText(item.title, t('workbench.home.unnamedTask')),
        meta: workbenchAgentName(item.agentName, t('workbench.home.noAgent')),
        leading: <TaskStatusTag status={item.status} live={item.agentActive === true} />,
        onOpen: item.id ? () => openTask(item) : undefined,
      };
    }
    return { key: `unknown-${index}`, title: t('workbench.home.unnamedPending') };
  }

  let detailContent: ReactNode;
  if (detailMetric) {
    const items = metricDetailItems(detailMetric);
    detailContent = (
      <section className="obs-detail-panel obs-workbench-metric-detail" data-testid="workbench-metric-detail" data-count={items.length}>
        <div className="obs-workbench-detail-head">
          <button type="button" className="obs-workbench-back" onClick={() => updateParams({ detail: null })}>
            <DisplayIcon icon={ArrowLeft} />{t('workbench.home.metricDetailBack')}
          </button>
          <h2>{detailMetric.label}</h2>
          <span>{detailMetric.count}{detailMetric.unit}</span>
        </div>
        <RowList
          items={items.map((item, index) => metricRow(item, detailMetric, index))}
          empty={<EmptyState title={t('workbench.home.metricDetailEmpty')} />}
        />
      </section>
    );
  } else {
    detailContent = (
      <DetailPanel
        activeKey={activeTab}
        onChange={(tab) => updateParams({ tab, detail: null })}
        tabs={[
          {
            key: 'agents', label: t('workbench.home.tabs.agents'), count: model.agentTasks.length,
            content: (
              <DataTable
                columns={[
                  { key: 'agent', title: t('workbench.home.agentColumns.agent'), width: 'var(--craft-workbench-column-agent)' },
                  { key: 'task', title: t('workbench.home.agentColumns.task'), width: '24%' },
                  { key: 'report', title: <span title={t('workbench.home.reportUnreadReason')}>{t('workbench.home.agentColumns.lastReport')}</span> },
                  { key: 'status', title: t('workbench.home.agentColumns.status'), width: 'var(--craft-workbench-column-status)' },
                  { key: 'age', title: <span title={t('workbench.home.timeUnreadReason')}>{t('workbench.home.agentColumns.age')}</span>, width: 'var(--craft-workbench-column-age)', align: 'right', hideOnSmall: true, numeric: true },
                ]}
                rows={agentRows}
                empty={<EmptyState title={t('workbench.home.noAgent')} />}
              />
            ),
          },
          {
            key: 'waits', label: t('workbench.home.tabs.waiting'),
            count: model.metrics[2].count ?? undefined,
            content: (
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
            key: 'lessons', label: t('workbench.home.tabs.lessons'),
            content: <EmptyState title={t('workbench.home.lessonsEmptyTitle')} description={t('workbench.home.lessonsEmptyDescription')} />,
          },
        ]}
      />
    );
  }

  const date = readAtUnixMs === undefined ? null : new Date(readAtUnixMs);
  const subtitle = date === null ? t('workbench.home.periodUnknown') : t('workbench.home.periodMe', {
    month: i18n.language.startsWith('zh') ? date.getMonth() + 1 : date.toLocaleString('en-US', { month: 'long' }),
    day: date.getDate(),
  });

  return (
    <div className="obs-workbench" data-testid="workbench-frame" data-state={resolvedState}>
      <div data-block="heading">
        <PageHeader title={t('workbench.home.headingMe')} subtitle={subtitle} />
        <PerspectiveLimitationNote />
        {resolvedState === 'unknown' ? (
          <div data-testid="workbench-ordering-unknown"><Notice tone="warning" title={t('workbench.home.unknownTitle')}>{t('workbench.home.unknownDescription')}</Notice></div>
        ) : null}
        {coverageText.length > 0 ? (
          <div data-testid="workbench-partial"><Notice tone={coverageWarning ? 'warning' : 'info'} title={coverageWarning ? t('workbench.home.partialTitle') : undefined}>{coverageText.join(' ')}</Notice></div>
        ) : null}
        {model.currentPerson === null ? (
          <Notice title={t('workbench.home.memberUnknownTitle')}>{t('workbench.home.memberUnknownDescription')}</Notice>
        ) : null}
      </div>
      <div data-block="freshness" className="obs-workbench-freshness"><FreshnessBadge freshness={freshness} /></div>
      <section className="obs-workbench-instrument" data-block="instrument" aria-label={t('workbench.home.headingMe')}>
        <div className="obs-workbench-verdict-top" data-health={model.verdict.health}>
          <div className="obs-workbench-verdict">
            <span className="obs-workbench-health-word" data-health={model.verdict.health} title={t(`display.healthStatus.${model.verdict.health}.meaning`)}>
              <StatusLamp lamp={healthLampState(model.verdict.health)} />{model.verdict.healthLabel}
            </span>
            <p className="obs-workbench-sentence">{model.verdict.sentence}</p>
            <div className="obs-workbench-byline">{model.verdict.byline}</div>
            {searchParams.get('basis') === '1' ? (
              <div className="obs-workbench-verdict-basis" data-testid="workbench-verdict-basis">
                {model.verdict.source === 'generated' ? t('workbench.home.verdictGeneratedBasis') : t('workbench.home.verdictUpdateBasis')}
              </div>
            ) : null}
          </div>
          <div className="obs-workbench-verdict-actions">
            <button
              type="button"
              className="obs-workbench-evidence-button"
              onClick={() => model.verdict.updateId
                ? navigate(`/updates/${encodeURIComponent(model.verdict.updateId!)}?${scopeQuery}`)
                : updateParams({ basis: searchParams.get('basis') === '1' ? null : '1' })}
            >
              {searchParams.get('basis') === '1' && !model.verdict.updateId
                ? t('workbench.home.closeBasis')
                : t('workbench.home.verdictBasis')}
              <DisplayIcon icon={ArrowRight} />
            </button>
          </div>
        </div>
        <KpiStrip items={model.metrics.map((metric) => ({
          key: metric.id,
          label: metric.label,
          value: metric.count === null ? '—' : String(metric.count),
          unit: metric.unit || undefined,
          change: metric.note === null ? undefined : { text: metric.note.text, tone: metric.note.tone, trend: metric.note.trend },
          emphasized: metric.emphasized,
          onOpen: metric.count === null ? undefined : () => updateParams({ detail: metric.id, tab: null }),
        }))} />
        <div className="obs-workbench-body">
          <Chain
            nodes={activeChain?.nodes ?? []}
            connectors={activeChain?.connectors}
            header={(
              <div className="obs-workbench-chain-head">
                <h2>{t('workbench.home.chainTitle')}</h2>
                {model.chains.length > 0 ? <span className="obs-tag">{t('workbench.home.chainCount', { count: model.chains.length })}</span> : null}
                {model.chains.length > 0 ? (
                  <div className="obs-workbench-chain-switch" role="group" aria-label={t('workbench.home.chainGroup')}>
                    {model.chainTabs.map((tab, index) => (
                      <button key={tab.id} type="button" aria-pressed={index === activeChainIndex} onClick={() => updateParams({ chain: String(index) })}>{tab.label}</button>
                    ))}
                  </div>
                ) : null}
              </div>
            )}
            empty={(
              <div className="obs-workbench-calm">
                <DisplayIcon icon={CheckCircle2} />
                <div><strong>{t('workbench.home.emptyChainTitle')}</strong><p>{t('workbench.home.emptyChainDescription')}</p></div>
              </div>
            )}
            present={(node) => chainNodeView(activeChain!, node)}
            onOpen={(node) => openChainNode(activeChain!, node)}
            footer={activeChain ? (
              <>
                <span>{activeChain.footer.impact.count === null
                  ? t('workbench.home.chainImpactUnknown')
                  : t('workbench.home.chainImpact', { count: activeChain.footer.impact.count })}</span>
                <span>{activeChain.footer.commitments.count === null
                  ? t('workbench.home.chainCommitmentsUnknown')
                  : t('workbench.home.chainCommitments', { count: activeChain.footer.commitments.count })}</span>
              </>
            ) : undefined}
          />
          <TodoRail
            title={t('workbench.home.railTitle')}
            items={model.pending.items.map(pendingView)}
            count={model.pending.count}
            coverageNote={model.pending.count === null
              ? t('workbench.home.railUnknownNote', { count: model.pending.items.length })
              : undefined}
            onFirstAgent={() => updateParams({ agent: searchParams.get('agent') === '1' ? null : '1' })}
            firstAccessory={searchParams.get('agent') === '1' && model.pending.items[0]
              ? <AgentHandback item={model.pending.items[0]} projectName={projectName} />
              : undefined}
            onViewAll={() => navigate(`/inbox?${scopeQuery}`)}
            empty={model.pending.count === null ? (
              <EmptyState title={t('workbench.home.railUnknownTitle')} description={t('workbench.home.railUnknownDescription')} />
            ) : (
              <EmptyState title={t('workbench.home.railEmptyTitle')} description={t('workbench.home.railEmptyDescription')} />
            )}
          />
        </div>
      </section>
      <div data-block="detail">{detailContent}</div>
    </div>
  );
}
