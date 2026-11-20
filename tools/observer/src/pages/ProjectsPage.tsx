import { useQueries, useQuery, type UseQueryResult } from '@tanstack/react-query';
import { Button } from 'antd';
import type { TFunction } from 'i18next';
import { Fragment, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { Link, useNavigate, useSearchParams } from 'react-router';
import { FreshnessBadge } from '../components/FreshnessBadge';
import { CompletionBar, CompletionLegend } from '../components/display/CompletionBar';
import { DataTable, type DataTableRow } from '../components/display/DataTable';
import { EmptyState } from '../components/display/EmptyState';
import { Notice } from '../components/display/Notice';
import { PageHeader } from '../components/display/PageHeader';
import { Section } from '../components/display/Section';
import { HealthTag, StatusLamp } from '../components/display/StatusTag';
import { ObservationForbidden } from '../components/states';
import {
  deriveProjectListItem,
  projectListFreshness,
  summarizeProjects,
  type ProjectListEntry,
  type ProjectListItem,
  type ProjectListModel,
  type ProjectSummaryCounts,
} from '../components/projects/project-list-model';
import { absoluteTimeText } from '../core/observation';
import { isAuthError } from '../lib/api';
import { readClockOffsetUnixMs, referenceNowUnixMs, useDisplayTimeZone } from '../lib/display-clock';
import { projectLabel } from '../lib/project-label';
import {
  normalizeProjectDetail,
  projectPath,
  projectsDetailPath,
  takeoverResultPath,
  type ProjectDetailFilter,
} from '../lib/project-path';
import { overviewModelQueryOptions, projectsQueryOptions } from '../lib/queries';
import { useRevocation } from '../lib/revocation';
import type { TeamFreshness, TeamOverviewModelResponse } from '../lib/types';
import './ProjectsPage.css';

const COLUMN_WIDTH = {
  project: 'var(--projects-column-project)',
  status: 'var(--projects-column-status)',
  completion: 'var(--projects-column-completion)',
  updated: 'var(--projects-column-updated)',
} as const;

const FAILED_FRESHNESS: TeamFreshness = {
  report_received: { status: 'failed' },
  authoritative: { status: 'failed' },
  page_read: { status: 'failed' },
};

type ModelQuery = UseQueryResult<TeamOverviewModelResponse>;

function matchingModel(entry: ProjectListEntry, query: ModelQuery | undefined): ProjectListModel | undefined {
  const data = query?.data;
  return data?.project === entry.key && data.input && data.input.options ? data : undefined;
}

function SummaryCount({ detail, count, label, prefix }: {
  detail: ProjectDetailFilter;
  count: number | null;
  label: string;
  prefix?: string;
}) {
  return (
    <span data-testid={`projects-summary-${detail}`}>
      {prefix}
      {count === null ? label : (
        <Link className="projects-lead-link" data-testid={`projects-summary-${detail}-link`} to={projectsDetailPath(detail)}>
          {label}
        </Link>
      )}
    </span>
  );
}

function ProjectStatus({ item }: { item: ProjectListItem }) {
  const { t } = useTranslation();
  if (item.health !== 'unknown') return <HealthTag status={item.health} />;
  return (
    <span className="obs-tag" title={t('projects.unknownStatusMeaning')}>
      <StatusLamp lamp="none" />
      {t('projects.unknownStatus')}
    </span>
  );
}

function destinationOf(item: ProjectListItem): string {
  return item.entry.takeover ? takeoverResultPath(item.entry.takeover.resultId) : projectPath(item.entry.key);
}

function ownerAndGroup(item: ProjectListItem, t: TFunction): string {
  if (item.entry.takeover) return t('display.healthStatus.info.label');
  const owner = item.leadName ?? t('projects.ownerUnknown');
  const group = item.groupName ?? t('projects.groupUnknown');
  if (!item.leadName && !item.groupName) return t('projects.ownerGroupUnknown');
  return `${owner} · ${group}`;
}

function factSummary(item: ProjectListItem, t: TFunction): string {
  const total = item.completion.all.count;
  return total === null ? t('projects.factSummaryUnknown') : t('projects.factSummary', { total });
}

function summaryOf(item: ProjectListItem, t: TFunction): string {
  const summary = item.summary;
  if (summary.kind === 'update') return summary.text.replace(/。$/, '');
  if (summary.kind === 'last-update') return t('projects.lastUpdate', { days: summary.days });
  if (summary.kind === 'facts') return factSummary(item, t);
  if (summary.kind === 'unknown') return t('projects.updateUnknown');
  return t('projects.takeoverSummary', {
    workstreams: summary.workstreams ?? '—',
    verified: summary.verified ?? '—',
    questions: summary.questions ?? '—',
  });
}

function updatedText(item: ProjectListItem, nowUnixMs: number | undefined, language: string, t: TFunction): string {
  const value = item.updatedAtUnixMs;
  if (value === null) return t('projects.notRecorded');
  if (nowUnixMs === undefined || !Number.isFinite(nowUnixMs) || value > nowUnixMs) {
    return absoluteTimeText(value, { language }).text;
  }
  const elapsed = nowUnixMs - value;
  if (item.entry.takeover && elapsed < 60 * 60 * 1000) return t('projects.justNow');
  const days = Math.floor(elapsed / 86_400_000);
  return days === 0 ? t('projects.today') : t('projects.daysAgo', { days });
}

function StateNotice({ testId, children, action }: { testId: string; children: ReactNode; action?: ReactNode }) {
  return (
    <div className="projects-notice" data-testid={testId}>
      <Notice tone="warning" extra={action}>{children}</Notice>
    </div>
  );
}

/** Prototype projects(): title, summary, and one read-only all-projects table. */
export function ProjectsPage() {
  const { t, i18n } = useTranslation();
  const navigate = useNavigate();
  const { revoked } = useRevocation();
  const [searchParams] = useSearchParams();
  const detail = normalizeProjectDetail(searchParams.get('detail'));
  const projects = useQuery(projectsQueryOptions(revoked));
  const entries: ProjectListEntry[] = revoked ? [] : (projects.data?.projects ?? []);
  const modelQueries = useQueries({
    queries: entries.map((entry) => {
      const options = overviewModelQueryOptions(entry.key, revoked);
      return { ...options, enabled: options.enabled && !entry.takeover };
    }),
  }) as ModelQuery[];
  const relevant = entries.flatMap((entry, index) => entry.takeover ? [] : [{ entry, query: modelQueries[index] }]);
  // Display clock and zone (AWR-PPO-079): each project's own model read carries its
  // derivation clock, walked forward with real time; the zone is the deployment's pinned
  // display zone. The takeover entry has no model read, so it keeps the shared display
  // zone and the list clock below.
  const displayTimeZone = useDisplayTimeZone();
  const items = entries.map((entry, index) => deriveProjectListItem(
    entry,
    matchingModel(entry, modelQueries[index]),
    displayTimeZone,
    referenceNowUnixMs(readClockOffsetUnixMs(matchingModel(entry, modelQueries[index]))),
  ));
  const summary = summarizeProjects(items);
  const visibleItems = detail === 'all' ? items
    : items.filter((item) => detail === 'risk' ? item.health === 'risk'
      : detail === 'no-update' ? item.health === 'none' : item.health === 'info');
  const models = relevant.flatMap(({ entry, query }) => {
    const model = matchingModel(entry, query);
    return model ? [model] : [];
  });
  // One clock for the updated column (the first ready model's display clock, matching the
  // list's shared comparison basis); the browser clock is only the fallback with no model.
  const commonNow = models.length > 0
    ? referenceNowUnixMs(readClockOffsetUnixMs(models[0]))
    : referenceNowUnixMs(null);
  const projectFailure = projects.isError && !projects.data;
  const factsFailure = relevant.length > 0 && relevant.every(({ query }) => query?.isError && !query.data);
  const initialLoading = projects.isPending;
  const factsLoading = relevant.some(({ query }) => query?.isPending);
  const partialRead = Boolean(projects.isError && projects.data)
    || relevant.some(({ query }) => query?.isError && !factsFailure);
  const hasUnknown = items.some((item) => item.health === 'unknown' || (!item.showTakeoverCompletion && item.completion.counts === null))
    || models.some((model) => model.freshness.authoritative.status === 'not_measured');
  const showUnknown = hasUnknown && !factsLoading && !partialRead && !factsFailure && !projectFailure && !initialLoading;
  const hasFailure = projectFailure || factsFailure || partialRead;
  const freshness = models.length > 0 ? projectListFreshness(models) : hasFailure ? FAILED_FRESHNESS : undefined;

  if (revoked || isAuthError(projects.error) || relevant.some(({ query }) => isAuthError(query?.error))) {
    return <ObservationForbidden />;
  }

  function retry() {
    if (projectFailure) return void projects.refetch();
    for (const { query } of relevant) if (query?.isError) void query.refetch();
  }

  const selectedCount = detail === 'no-update' ? summary.noUpdate.count : summary[detail].count;
  const sectionCount = selectedCount === null ? <span data-testid="projects-section-count">—</span> : (
    <span data-testid="projects-section-count">
      <Link className="projects-section-count-link" data-testid="projects-section-count-link" to={projectsDetailPath(detail)}>{selectedCount}</Link>
    </span>
  );
  const summaryLabel = (key: Exclude<keyof ProjectSummaryCounts, 'all'>) => t(`projects.summary.${key}`, { count: summary[key].count ?? '—' });
  const rows: DataTableRow[] = visibleItems.map((item) => {
    const projectIndex = entries.indexOf(item.entry);
    const title = projectLabel(item.entry, projectIndex + 1);
    const href = destinationOf(item);
    return {
      key: item.entry.key,
      highlighted: item.health === 'risk',
      onOpen: () => navigate(href),
      cells: [
        <div className="projects-project-cell">
          <Link className="projects-project-name projects-ellipsis" to={href} title={title}>{title}</Link>
          <div className="projects-project-meta projects-ellipsis" title={ownerAndGroup(item, t)}>{ownerAndGroup(item, t)}</div>
        </div>,
        <ProjectStatus item={item} />,
        <span className="projects-summary-text" data-reporter-text={item.summary.kind === 'update' || undefined}>
          {summaryOf(item, t)}
        </span>,
        item.showTakeoverCompletion ? (
          <Link className="projects-no-verified" to={href}>{t('projects.noVerifiedAfterTakeover')}</Link>
        ) : item.completion.counts ? <CompletionBar counts={item.completion.counts} />
          : <span className="projects-completion-unknown">{t('projects.completionUnknown')}</span>,
        <div className="projects-updated" data-late={item.late || undefined}>
          <div className="projects-update-time">{updatedText(item, commonNow, i18n.language, t)}</div>
          {item.late ? <div className="projects-update-note">{t('projects.needsUpdate')}</div> : null}
        </div>,
      ],
    };
  });

  const table = visibleItems.length > 0 ? (
    <div className="projects-table" data-testid="projects-table">
      <DataTable
        columns={[
          { key: 'project', title: t('projects.columns.project'), width: COLUMN_WIDTH.project },
          { key: 'status', title: t('projects.columns.status'), width: COLUMN_WIDTH.status },
          { key: 'sentence', title: t('projects.columns.sentence') },
          { key: 'completion', title: t('projects.columns.completion'), width: COLUMN_WIDTH.completion },
          { key: 'updated', title: t('projects.columns.updated'), width: COLUMN_WIDTH.updated, hideOnSmall: true, align: 'right' },
        ]}
        rows={rows}
      />
    </div>
  ) : (
    <div className="projects-table" data-testid="projects-table">
      <EmptyState title={t('projects.emptyDetail')} />
    </div>
  );

  let content: ReactNode;
  if (initialLoading) {
    content = <div className="projects-state" data-testid="projects-loading">{t('projects.loading')}</div>;
  } else if (projectFailure) {
    content = (
      <StateNotice testId="projects-read-error" action={<Button onClick={retry}>{t('projects.retry')}</Button>}>
        {t('projects.readFailed')}
      </StateNotice>
    );
  } else if (entries.length === 0) {
    content = <div className="projects-state" data-testid="projects-empty"><EmptyState title={t('projects.empty')} /></div>;
  } else {
    content = (
      <>
        {factsFailure ? <StateNotice testId="projects-read-error" action={<Button onClick={retry}>{t('projects.retry')}</Button>}>{t('projects.factsReadFailed')}</StateNotice> : null}
        {partialRead ? <StateNotice testId="projects-partial" action={<Button onClick={retry}>{t('projects.retry')}</Button>}>{t('projects.partial')}</StateNotice> : null}
        {showUnknown ? <StateNotice testId="projects-unknown">{t('projects.unknown')}</StateNotice> : null}
        {table}
      </>
    );
  }

  const readyCount = models.length;
  const coverage = !projects.data ? t('projects.loading')
    : readyCount === relevant.length ? t('projects.coverageAll', { projects: entries.length })
      : readyCount === 0 ? t('projects.coverageNone')
        : t('projects.coveragePartial', { projects: entries.length, ready: readyCount });

  return (
    <div className="projects-page" data-testid="page-projects">
      <PageHeader title={t('projects.title')} subtitle={t('projects.visibleCount', { count: summary.all.count ?? '—' })} />
      <p className="projects-lead" data-testid="projects-lead">
        <SummaryCount detail="all" count={summary.all.count} label={t('projects.summary.total', { count: summary.all.count ?? '—' })} />
        <span className="projects-lead-rest">
          {(['risk', 'noUpdate', 'takeover'] as const).map((key) => (
            <Fragment key={key}>
              <SummaryCount detail={key === 'noUpdate' ? 'no-update' : key} count={summary[key].count} label={summaryLabel(key)} prefix={t('projects.separator')} />
            </Fragment>
          ))}
        </span>
      </p>
      <Section
        className="projects-section"
        title={t('projects.allProjects')}
        count={projects.data ? sectionCount : undefined}
        extra={<CompletionLegend />}
      >
        {content}
        <div className="projects-footer">
          <div data-testid="projects-coverage">{coverage}</div>
          <div className="projects-freshness"><FreshnessBadge freshness={freshness} /></div>
        </div>
      </Section>
    </div>
  );
}
