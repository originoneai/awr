import { useQuery, useQueryClient } from '@tanstack/react-query';
import { Button, Typography } from 'antd';
import type { TFunction } from 'i18next';
import { Fragment, useEffect, useMemo, useState, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { useNavigate, useParams, useSearchParams } from 'react-router';
import { FreshnessBadge } from '../components/FreshnessBadge';
import { DataTable, type DataTableRow } from '../components/display/DataTable';
import { Notice } from '../components/display/Notice';
import { RowList, type RowListItem } from '../components/display/RowList';
import { Section } from '../components/display/Section';
import { HealthTag, StatusLamp, healthLampState } from '../components/display/StatusTag';
import { ArrowLeftIcon, ArrowUpIcon, BuildingIcon } from '../components/icons/prototype-icons';
import { ObservationForbidden } from '../components/states';
import {
  buildFactRows,
  deriveUpdateClockOffset,
  findPreviousPeriodUpdate,
  formatPeriodLabel,
  formatPeriodRange,
  formatWrittenTime,
  hasUnknownWaitKind,
  isValidPeriodStatus,
  mergeConservativeFreshness,
  mergeLoadedUpdates,
  sanitizeAuthorName,
  sanitizeUpdateSummary,
  type FreshnessSource,
  type UpdateFactKey,
  type UpdateFactRow,
} from '../components/updates/update-page-model';
import { isAuthError } from '../lib/api';
import { referenceNowUnixMs, useDisplayTimeZone } from '../lib/display-clock';
import { workLabel } from '../core/observation';
import { findForbiddenVisibleText } from '../lib/human-language-guard';
import {
  overviewQueryOptions,
  periodUpdateQueryOptions,
  periodUpdatesQueryOptions,
  projectsQueryOptions,
} from '../lib/queries';
import { useRevocation } from '../lib/revocation';
import { useScope } from '../lib/scope';
import { taskPath } from '../lib/task-path';
import type {
  PeriodUpdateRecord,
  PeriodUpdateWaitKind,
  TeamPeriodUpdatesResponse,
} from '../lib/types';
import './UpdatePage.css';

interface ActiveFactDetail {
  factKey: UpdateFactKey;
  waitKind?: PeriodUpdateWaitKind;
}

function formatFactCount(row: UpdateFactRow, t: TFunction, language: string): string {
  if (row.unit === 'item') {
    if (!language.toLowerCase().startsWith('zh') && row.count === 1) {
      return t('updatePage.unitItemOne');
    }
    return t('updatePage.unitItem', { count: row.count });
  }
  return t('updatePage.unitWait', { count: row.count });
}

function errorCodeOf(error: unknown): string | null {
  if (!error || typeof error !== 'object') return null;
  const code = (error as { code?: unknown }).code;
  return typeof code === 'string' ? code : null;
}

function safeProjectTitle(
  project: { key: string; title?: string | null } | undefined,
  language: string,
  fallback: string,
): string {
  if (!project) return fallback;
  const raw = typeof project.title === 'string' ? project.title.trim() : '';
  if (!raw || raw === project.key) return fallback;
  const label = workLabel(raw, { language, recordedTitle: true, fallback });
  if (label.kind !== 'named' || findForbiddenVisibleText(label.text).length > 0) return fallback;
  return label.text;
}

/**
 * Prototype updatePage(): one periodic update for a project.
 * Left column: program-counted facts table (from PPD-011) with clickable counts and task links,
 * plus how the lead's sentence rolls up to department and executive views.
 * Right column: previous period with status transition, and all recorded updates for this project.
 * Strictly read-only: no business actions, no platform-written sentences.
 */
export function UpdatePage() {
  const { t, i18n } = useTranslation();
  const language = i18n.language;
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const { update: routeUpdateId } = useParams<{ update?: string }>();
  const [searchParams, setSearchParams] = useSearchParams();
  const { projectKey } = useScope();
  const { revoked } = useRevocation();
  const displayTimeZone = useDisplayTimeZone();

  const project = projectKey?.trim() ? projectKey.trim() : null;
  const [extraPages, setExtraPages] = useState<TeamPeriodUpdatesResponse[]>([]);
  const [loadingMore, setLoadingMore] = useState(false);
  const [loadMoreFailed, setLoadMoreFailed] = useState(false);
  const [activeDetail, setActiveDetail] = useState<ActiveFactDetail | null>(null);

  useEffect(() => {
    setExtraPages([]);
    setLoadingMore(false);
    setLoadMoreFailed(false);
    setActiveDetail(null);
  }, [project, revoked]);

  useEffect(() => {
    setActiveDetail(null);
  }, [routeUpdateId]);

  const projectsQuery = useQuery(projectsQueryOptions(revoked));
  const updatesListQuery = useQuery(periodUpdatesQueryOptions(project, null, revoked));

  const loadedPages = useMemo(
    () => (updatesListQuery.data ? [updatesListQuery.data, ...extraPages] : extraPages),
    [updatesListQuery.data, extraPages],
  );
  const allUpdates = useMemo(
    () => mergeLoadedUpdates(loadedPages.map((page) => page.result.data.items)),
    [loadedPages],
  );
  const lastLoadedPage = loadedPages.length > 0 ? loadedPages[loadedPages.length - 1] : undefined;
  const listTruncated = Boolean(lastLoadedPage?.result.data.truncated);
  const nextCursor = lastLoadedPage?.result.data.next_cursor ?? null;

  const activeUpdateId = routeUpdateId ?? allUpdates[0]?.update_id ?? null;

  const updateInspectQuery = useQuery(periodUpdateQueryOptions(project, activeUpdateId, revoked));
  const overviewQuery = useQuery(overviewQueryOptions(project, revoked));

  const workTitleById = useMemo(() => {
    const map = new Map<string, string>();
    for (const work of overviewQuery.data?.works ?? []) {
      if (work.key && typeof work.title === 'string' && work.title.trim()) {
        map.set(work.key, work.title.trim());
      }
    }
    return map;
  }, [overviewQuery.data]);

  const clockOffset = deriveUpdateClockOffset(updateInspectQuery.data, updatesListQuery.data);
  const nowUnixMs = referenceNowUnixMs(clockOffset);
  const referenceYear = useMemo(() => {
    const formatted = new Intl.DateTimeFormat('en-US', { year: 'numeric', timeZone: displayTimeZone })
      .format(new Date(nowUnixMs));
    const parsed = Number(formatted);
    return Number.isFinite(parsed) ? parsed : null;
  }, [nowUnixMs, displayTimeZone]);

  if (
    revoked
    || isAuthError(projectsQuery.error)
    || isAuthError(updatesListQuery.error)
    || isAuthError(updateInspectQuery.error)
    || isAuthError(overviewQuery.error)
  ) {
    return (
      <div className="update-page" data-testid="page-update">
        <ObservationForbidden />
      </div>
    );
  }

  const projectItems = projectsQuery.data?.projects ?? [];
  const projectIndex = project ? projectItems.findIndex((item) => item.key === project) : -1;
  const projectEntry = projectIndex >= 0 ? projectItems[projectIndex] : undefined;
  const projectTitle = projectEntry
    ? safeProjectTitle(projectEntry, language, t('shell.unnamedProject', { number: projectIndex + 1 }))
    : projectsQuery.isPending
      ? t('common.loading')
      : t('shell.unnamedProject', { number: 1 });

  const scopeQuery = project ? `?scope=${encodeURIComponent(project)}` : '';
  const openUpdate = (updateId: string) => {
    navigate(`/updates/${encodeURIComponent(updateId)}${scopeQuery}`);
  };
  const goBack = () => {
    if (project) navigate(`/projects/${encodeURIComponent(project)}`);
    else navigate('/projects');
  };

  // No project selected in URL scope: never guess the first project; let the viewer pick one.
  if (!project) {
    if (projectsQuery.isPending) {
      return (
        <div className="update-page" data-testid="page-update">
          <div className="update-state-box" data-testid="update-loading">{t('updatePage.loading')}</div>
        </div>
      );
    }
    if (projectsQuery.isError) {
      return (
        <div className="update-page" data-testid="page-update">
          <Notice
            tone="warning"
            title={t('updatePage.failedTitle')}
            extra={<Button size="small" onClick={() => void projectsQuery.refetch()}>{t('updatePage.retry')}</Button>}
            className="update-notice"
          >
            <span data-testid="update-failed">{t('updatePage.failedDescription')}</span>
          </Notice>
        </div>
      );
    }
    const projectRows: RowListItem[] = projectItems.map((item, idx) => ({
      key: item.key,
      title: safeProjectTitle(item, language, t('shell.unnamedProject', { number: idx + 1 })),
      onOpen: () => {
        const next = new URLSearchParams(searchParams);
        next.set('scope', item.key);
        setSearchParams(next);
      },
    }));
    return (
      <div className="update-page" data-testid="page-update">
        <button type="button" className="update-back" data-testid="update-back" onClick={() => navigate('/projects')}>
          <ArrowLeftIcon size={14} aria-hidden />
          {t('updatePage.back')}
        </button>
        <div className="update-empty-card" data-testid="update-choose-project">
          <Section title={t('updatePage.chooseProjectTitle')} className="update-section">
            <RowList
              items={projectRows}
              empty={<div className="update-empty">{t('updatePage.chooseProjectEmpty')}</div>}
            />
          </Section>
        </div>
      </div>
    );
  }

  // Conservative freshness across all active reads on this page.
  // Overview only resolves human task titles: its failure or staleness degrades freshness,
  // while its authoritative not_measured (e.g. demo overview) does not mask measured update reads.
  const freshnessSources: FreshnessSource[] = [];
  if (updatesListQuery.isError || loadMoreFailed) freshnessSources.push({ status: 'failed' });
  else if (updatesListQuery.data) {
    for (const page of loadedPages) freshnessSources.push({ status: 'ok', freshness: page.freshness });
  } else if (updatesListQuery.isPending) freshnessSources.push({ status: 'pending' });

  if (activeUpdateId) {
    if (updateInspectQuery.isError) freshnessSources.push({ status: 'failed' });
    else if (updateInspectQuery.data) freshnessSources.push({ status: 'ok', freshness: updateInspectQuery.data.freshness });
    else if (updateInspectQuery.isPending) freshnessSources.push({ status: 'pending' });
  }

  if (overviewQuery.isError || Boolean(overviewQuery.data && overviewQuery.data.coverage.failed > 0)) {
    freshnessSources.push({ status: 'failed' });
  } else if (overviewQuery.data) {
    const ov = overviewQuery.data.freshness;
    const overviewDegraded = ov.authoritative.status === 'failed'
      || ov.authoritative.status === 'stale'
      || ov.page_read.status === 'failed'
      || ov.page_read.status === 'stale';
    if (overviewDegraded) freshnessSources.push({ status: 'ok', freshness: ov });
  }

  const conservativeFreshness = mergeConservativeFreshness(freshnessSources);

  // Determine the displayed update record (inspect is authoritative; list record is fallback only when inspect failed).
  const inspectedData = updateInspectQuery.data?.result.data;
  const listRecord = activeUpdateId
    ? allUpdates.find((item) => item.update_id === activeUpdateId) ?? null
    : null;
  const activeRecord: PeriodUpdateRecord | null = inspectedData?.update ?? listRecord;

  const inspectNotFound = updateInspectQuery.isError && errorCodeOf(updateInspectQuery.error) === 'NotFound';
  const primaryLoading = !activeRecord
    && (updatesListQuery.isPending || (Boolean(activeUpdateId) && updateInspectQuery.isPending));

  const projectHasNoUpdates = !primaryLoading
    && Boolean(updatesListQuery.data)
    && !updatesListQuery.isError
    && allUpdates.length === 0
    && !listTruncated
    && (!activeUpdateId || inspectNotFound)
    && !inspectedData;

  const primaryFailed = !primaryLoading
    && !projectHasNoUpdates
    && !activeRecord
    && (updatesListQuery.isError || (Boolean(activeUpdateId) && updateInspectQuery.isError));

  const retryFailed = () => {
    if (updatesListQuery.isError) void updatesListQuery.refetch();
    if (updateInspectQuery.isError) void updateInspectQuery.refetch();
    if (overviewQuery.isError) void overviewQuery.refetch();
  };

  const loadEarlierUpdates = async () => {
    if (!project || !nextCursor || loadingMore || revoked) return;
    setLoadingMore(true);
    setLoadMoreFailed(false);
    try {
      const page = await queryClient.fetchQuery(periodUpdatesQueryOptions(project, nextCursor, revoked));
      setExtraPages((prev) => [...prev, page]);
    } catch (error) {
      if (!isAuthError(error)) setLoadMoreFailed(true);
    } finally {
      setLoadingMore(false);
    }
  };

  const backButton = (
    <button type="button" className="update-back" data-testid="update-back" onClick={goBack}>
      <ArrowLeftIcon size={14} aria-hidden />
      {t('updatePage.back')}
    </button>
  );

  // State: Loading
  if (primaryLoading) {
    return (
      <div className="update-page" data-testid="page-update">
        {backButton}
        <div className="update-state-box" data-testid="update-loading">
          {t('updatePage.loading')}
        </div>
      </div>
    );
  }

  // State: Empty project (no period updates recorded yet)
  if (projectHasNoUpdates) {
    return (
      <div className="update-page" data-testid="page-update">
        {backButton}
        <header className="update-hero" data-testid="update-hero">
          <div className="update-hero-main">
            <Typography.Title level={1} className="update-hero-title" data-testid="update-title">
              {t('updatePage.pageTitleNoPeriod', { project: projectTitle })}
            </Typography.Title>
          </div>
        </header>
        <div className="update-empty-card" data-testid="update-empty">
          <Section title={t('updatePage.allUpdatesTitle')} padded className="update-section">
            <p className="update-empty-title">{t('updatePage.emptyTitle')}</p>
            <p className="update-empty-body">{t('updatePage.emptyDescription')}</p>
          </Section>
        </div>
        <footer className="update-coverage-footer" data-testid="update-coverage">
          {conservativeFreshness ? <FreshnessBadge freshness={conservativeFreshness} /> : null}
          <div data-testid="update-coverage-summary">{t('updatePage.coverageComplete')}</div>
        </footer>
      </div>
    );
  }

  // State: Failed (nothing confirmed to render)
  if (primaryFailed || !activeRecord) {
    return (
      <div className="update-page" data-testid="page-update">
        {backButton}
        <Notice
          tone="warning"
          title={t('updatePage.failedTitle')}
          extra={<Button size="small" onClick={retryFailed}>{t('updatePage.retry')}</Button>}
          className="update-notice"
        >
          <span data-testid="update-failed">{t('updatePage.failedDescription')}</span>
        </Notice>
        {allUpdates.length > 0 ? (
          <div className="update-empty-card" data-testid="update-all-updates">
            <Section title={t('updatePage.allUpdatesTitle')} count={allUpdates.length} className="update-section">
              <div className="update-list-rows">
                <RowList
                  items={allUpdates.map((item) => {
                    const period = formatPeriodLabel(item, language, referenceYear) ?? '';
                    const author = sanitizeAuthorName(item.author.display_name) ?? t('updatePage.authorFallback');
                    const time = formatWrittenTime(item.recorded_at_unix_ms, {
                      language,
                      nowUnixMs,
                      timeZone: displayTimeZone,
                    });
                    return {
                      key: item.update_id,
                      leading: <StatusLamp lamp={healthLampState(isValidPeriodStatus(item.status) ? item.status : 'none')} />,
                      title: sanitizeUpdateSummary(item.summary, language, t('updatePage.summaryUnavailable')),
                      meta: t('updatePage.allUpdatesMeta', { period, author, time }),
                      onOpen: () => openUpdate(item.update_id),
                    };
                  })}
                />
              </div>
            </Section>
          </div>
        ) : null}
        <footer className="update-coverage-footer" data-testid="update-coverage">
          {conservativeFreshness ? <FreshnessBadge freshness={conservativeFreshness} /> : null}
        </footer>
      </div>
    );
  }

  // Data / Partial / Unknown state for a concrete update record
  const periodLabel = formatPeriodLabel(activeRecord, language, referenceYear);
  const dateRange = formatPeriodRange(activeRecord.starts_on, activeRecord.ends_on, language);
  const authorName = sanitizeAuthorName(activeRecord.author.display_name);
  const authorDisplay = authorName ?? t('updatePage.authorFallback');
  const writtenTime = formatWrittenTime(activeRecord.recorded_at_unix_ms, {
    language,
    nowUnixMs,
    timeZone: displayTimeZone,
  });
  const summaryText = sanitizeUpdateSummary(activeRecord.summary, language, t('updatePage.summaryUnavailable'));
  const healthStatus = isValidPeriodStatus(activeRecord.status) ? activeRecord.status : 'none';

  const factSnapshot = inspectedData?.fact_snapshot ?? null;
  const factCoverage = inspectedData?.fact_snapshot_coverage ?? null;
  const hiddenWorkCount = factCoverage && !factCoverage.complete ? factCoverage.hidden_work_count : 0;

  const factRows = factSnapshot
    ? buildFactRows(factSnapshot, workTitleById, language, t('updatePage.unnamedTask'))
    : [];

  const overviewIncomplete = overviewQuery.isError
    || Boolean(overviewQuery.data && (overviewQuery.data.coverage.failed > 0 || overviewQuery.data.coverage.truncated));
  const orderingIncomparable = updateInspectQuery.data?.ordering === 'incomparable'
    || loadedPages.some((page) => page.ordering === 'incomparable');

  const partialFailure = updatesListQuery.isError
    || updateInspectQuery.isError
    || loadMoreFailed
    || overviewIncomplete
    || hiddenWorkCount > 0;

  const unknownOutcome = orderingIncomparable
    || (Boolean(inspectedData) && factSnapshot === null)
    || hasUnknownWaitKind(factSnapshot)
    || conservativeFreshness?.authoritative.status === 'not_measured';

  const listSep = t('common.listSeparator');
  const waitKindSep = t('updatePage.waitKindSeparator');

  // Build facts table rows (exact 3 columns matching prototype: 数的是什么 / 多少 / 是哪些)
  const factTableRows: DataTableRow[] = factRows.map((row) => {
    const rowLabel = t(`updatePage.factLabels.${row.key}`);
    const countLabel = formatFactCount(row, t, language);
    let whichCell: ReactNode;

    if (row.count === 0) {
      whichCell = <span className="update-dim">{t('updatePage.factsEmpty')}</span>;
    } else if (row.waitKinds && row.waitKinds.length > 0) {
      whichCell = (
        <span className="update-fact-items">
          {row.waitKinds.map((group, idx) => (
            <Fragment key={group.kind}>
              {idx > 0 ? waitKindSep : null}
              <button
                type="button"
                className="update-fact-link"
                data-testid={`update-fact-kind-${row.key}-${group.kind}`}
                onClick={() => setActiveDetail({ factKey: row.key, waitKind: group.kind })}
              >
                {t(`updatePage.waitKinds.${group.kind}`, { count: group.count })}
              </button>
            </Fragment>
          ))}
        </span>
      );
    } else {
      whichCell = (
        <span className="update-fact-items">
          {row.items.map((item, idx) => (
            <Fragment key={item.key}>
              {idx > 0 ? listSep : null}
              <button
                type="button"
                className="update-fact-link"
                data-testid={`update-fact-task-${row.key}-${item.workId}`}
                onClick={() => navigate(taskPath(project, item.workId))}
              >
                {item.title}
              </button>
            </Fragment>
          ))}
        </span>
      );
    }

    return {
      key: row.key,
      testId: `update-fact-row-${row.key}`,
      cells: [
        <span className="update-fact-label">{rowLabel}</span>,
        <button
          type="button"
          className="update-fact-count"
          data-testid={`update-fact-count-${row.key}`}
          aria-label={t('updatePage.detailOpenAria', { fact: rowLabel, count: countLabel })}
          aria-expanded={activeDetail?.factKey === row.key && activeDetail.waitKind === undefined}
          onClick={() => setActiveDetail((prev) => (
            prev?.factKey === row.key && prev.waitKind === undefined ? null : { factKey: row.key }
          ))}
        >
          {countLabel}
        </button>,
        whichCell,
      ],
    };
  });

  // Active fact detail panel (opened when user clicks a count or wait-kind entry; row count === reported count)
  let detailPanel: ReactNode = null;
  if (activeDetail && factSnapshot) {
    const row = factRows.find((entry) => entry.key === activeDetail.factKey);
    if (row) {
      const waitGroup = activeDetail.waitKind
        ? row.waitKinds?.find((entry) => entry.kind === activeDetail.waitKind)
        : undefined;
      const detailCount = waitGroup ? waitGroup.count : row.count;
      const detailItems = waitGroup ? waitGroup.items : row.items;
      const factTitle = t(`updatePage.factLabels.${row.key}`);
      const detailTitle = waitGroup
        ? t('updatePage.detailTitleWithKind', {
          fact: factTitle,
          kind: t(`updatePage.waitKindLabels.${waitGroup.kind}`),
        })
        : factTitle;
      const detailRows: RowListItem[] = detailItems.map((item) => ({
        key: item.key,
        title: item.title,
        meta: item.waitKind ? t(`updatePage.waitKindLabels.${item.waitKind}`) : undefined,
        onOpen: () => navigate(taskPath(project, item.workId)),
      }));
      detailPanel = (
        <div
          data-testid="update-fact-detail"
          data-fact={row.key}
          data-wait-kind={waitGroup?.kind}
          data-count={detailCount}
        >
          <Section
            title={detailTitle}
            count={detailCount}
            className="update-section"
            extra={(
              <button
                type="button"
                className="update-link-btn"
                data-testid="update-fact-detail-close"
                onClick={() => setActiveDetail(null)}
              >
                {t('updatePage.detailClose')}
              </button>
            )}
          >
            <RowList
              items={detailRows}
              empty={<div className="update-empty">{t('updatePage.detailEmpty')}</div>}
            />
          </Section>
        </div>
      );
    }
  }

  // Left block 1 body (respects own query state: loading / failed / no snapshot / table)
  let factsBody: ReactNode;
  if (updateInspectQuery.isPending && !inspectedData) {
    factsBody = <div className="update-empty" data-testid="update-facts-loading">{t('updatePage.loading')}</div>;
  } else if (updateInspectQuery.isError && !inspectedData) {
    factsBody = <div className="update-empty" data-testid="update-facts-failed">{t('updatePage.factsReadFailed')}</div>;
  } else if (factSnapshot === null) {
    factsBody = <div className="update-empty" data-testid="update-facts-unknown">{t('updatePage.factsUnavailable')}</div>;
  } else {
    factsBody = (
      <DataTable
        columns={[
          { key: 'what', title: t('updatePage.factsColWhat'), width: '30%' },
          { key: 'count', title: t('updatePage.factsColCount'), width: 96 },
          { key: 'which', title: t('updatePage.factsColWhich') },
        ]}
        rows={factTableRows}
      />
    );
  }

  // Left block 2: how this sentence rolls up (static non-clickable info rows matching prototype)
  const rollupItems: RowListItem[] = [
    {
      key: 'dept',
      leading: (
        <span className="update-ki" aria-hidden="true">
          <BuildingIcon size={16} />
        </span>
      ),
      title: t('updatePage.rollupDeptTitle'),
      meta: t('updatePage.rollupDeptDesc'),
    },
    {
      key: 'top',
      leading: (
        <span className="update-ki" aria-hidden="true">
          <ArrowUpIcon size={16} />
        </span>
      ),
      title: t('updatePage.rollupTopTitle'),
      meta: t('updatePage.rollupTopDesc'),
    },
  ];

  // Right block 1: previous period
  const previousUpdate = !updatesListQuery.isPending && !updatesListQuery.isError
    ? findPreviousPeriodUpdate(allUpdates, activeRecord)
    : null;

  let previousBody: ReactNode;
  if (updatesListQuery.isPending && allUpdates.length === 0) {
    previousBody = <div className="update-empty" data-testid="update-prev-loading">{t('updatePage.loading')}</div>;
  } else if (updatesListQuery.isError && allUpdates.length === 0) {
    previousBody = <div className="update-empty" data-testid="update-prev-failed">{t('updatePage.previousFailed')}</div>;
  } else if (previousUpdate) {
    const prevPeriod = formatPeriodLabel(previousUpdate, language, referenceYear) ?? '';
    const prevHealth = isValidPeriodStatus(previousUpdate.status) ? previousUpdate.status : 'none';
    const prevHealthWord = t(`display.healthStatus.${prevHealth}.label`);
    const currentHealthWord = t(`display.healthStatus.${healthStatus}.label`);
    const transitionMeta = prevHealth !== healthStatus
      ? t('updatePage.previousChanged', {
        period: prevPeriod,
        prev: prevHealthWord,
        current: currentHealthWord,
      })
      : t('updatePage.previousUnchanged', {
        period: prevPeriod,
        prev: prevHealthWord,
      });
    previousBody = (
      <div className="update-list-rows">
        <RowList
          items={[
            {
              key: previousUpdate.update_id,
              leading: <StatusLamp lamp={healthLampState(prevHealth)} />,
              title: sanitizeUpdateSummary(previousUpdate.summary, language, t('updatePage.summaryUnavailable')),
              meta: transitionMeta,
              onOpen: () => openUpdate(previousUpdate.update_id),
            },
          ]}
        />
      </div>
    );
  } else if (listTruncated) {
    previousBody = <div className="update-empty" data-testid="update-prev-truncated">{t('updatePage.previousTruncated')}</div>;
  } else {
    previousBody = <div className="update-empty" data-testid="update-prev-first">{t('updatePage.previousFirst')}</div>;
  }

  // Right block 2: all updates
  const allUpdateRowItems: RowListItem[] = allUpdates.map((item) => {
    const itemPeriod = formatPeriodLabel(item, language, referenceYear) ?? '';
    const itemAuthor = sanitizeAuthorName(item.author.display_name) ?? t('updatePage.authorFallback');
    const itemTime = formatWrittenTime(item.recorded_at_unix_ms, {
      language,
      nowUnixMs,
      timeZone: displayTimeZone,
    });
    const itemHealth = isValidPeriodStatus(item.status) ? item.status : 'none';
    return {
      key: item.update_id,
      leading: <StatusLamp lamp={healthLampState(itemHealth)} />,
      title: sanitizeUpdateSummary(item.summary, language, t('updatePage.summaryUnavailable')),
      meta: t('updatePage.allUpdatesMeta', { period: itemPeriod, author: itemAuthor, time: itemTime }),
      selected: item.update_id === activeRecord.update_id,
      onOpen: () => openUpdate(item.update_id),
    };
  });

  let allUpdatesBody: ReactNode;
  if (updatesListQuery.isPending && allUpdates.length === 0) {
    allUpdatesBody = <div className="update-empty" data-testid="update-all-loading">{t('updatePage.loading')}</div>;
  } else if (updatesListQuery.isError && allUpdates.length === 0) {
    allUpdatesBody = <div className="update-empty" data-testid="update-all-failed">{t('updatePage.allUpdatesFailed')}</div>;
  } else {
    allUpdatesBody = (
      <>
        <div className="update-list-rows">
          <RowList
            items={allUpdateRowItems}
            empty={<div className="update-empty">{t('updatePage.emptyTitle')}</div>}
          />
        </div>
        {listTruncated ? (
          <div className="update-pnote update-pnote-row" data-testid="update-all-truncated">
            <span>{t('updatePage.allUpdatesTruncated')}</span>
            {nextCursor ? (
              <button
                type="button"
                className="update-link-btn"
                data-testid="update-load-more"
                disabled={loadingMore}
                onClick={() => void loadEarlierUpdates()}
              >
                {loadingMore ? t('updatePage.loadingMore') : t('updatePage.loadMore')}
              </button>
            ) : null}
          </div>
        ) : null}
      </>
    );
  }

  // Coverage disclosure lines (conservative: name every failure or gap)
  const coverageNotes: string[] = [];
  if (updateInspectQuery.isError) coverageNotes.push(t('updatePage.coveragePartialInspect'));
  if (updatesListQuery.isError || loadMoreFailed) coverageNotes.push(t('updatePage.coveragePartialList'));
  if (overviewIncomplete) coverageNotes.push(t('updatePage.coveragePartialTasks'));
  if (hiddenWorkCount > 0) coverageNotes.push(t('updatePage.coverageHiddenWorks', { count: hiddenWorkCount }));
  if (listTruncated) coverageNotes.push(t('updatePage.coverageTruncated'));
  if (orderingIncomparable) coverageNotes.push(t('updatePage.coverageOrderingIncomparable'));

  return (
    <div className="update-page" data-testid="page-update">
      {backButton}

      {partialFailure ? (
        <Notice
          tone="warning"
          title={t('updatePage.partialTitle')}
          extra={updatesListQuery.isError || updateInspectQuery.isError || overviewQuery.isError ? (
            <Button size="small" onClick={retryFailed}>{t('updatePage.retry')}</Button>
          ) : undefined}
          className="update-notice"
        >
          <span data-testid="update-partial">{t('updatePage.partialDescription')}</span>
        </Notice>
      ) : null}

      {!partialFailure && unknownOutcome ? (
        <Notice
          tone="info"
          title={t('updatePage.unknownTitle')}
          className="update-notice"
        >
          <span data-testid="update-unknown">{t('updatePage.unknownDescription')}</span>
        </Notice>
      ) : null}

      <header className="update-hero" data-testid="update-hero">
        <div className="update-hero-main">
          <Typography.Title level={1} className="update-hero-title" data-testid="update-title">
            {periodLabel
              ? t('updatePage.pageTitle', { project: projectTitle, period: periodLabel })
              : t('updatePage.pageTitleNoPeriod', { project: projectTitle })}
          </Typography.Title>
          <p className="update-hero-sum" data-testid="update-summary">
            {summaryText}
          </p>
          <div className="update-hero-meta" data-testid="update-meta">
            <HealthTag status={healthStatus} />
            {dateRange ? <span data-testid="update-range">{dateRange}</span> : null}
            <span className="update-hero-author" data-testid="update-author">
              {authorName ? (
                <span className="update-avatar" aria-hidden="true">{Array.from(authorName)[0]}</span>
              ) : null}
              {t('updatePage.authorWrittenAt', { author: authorDisplay, time: writtenTime })}
            </span>
            {activeRecord.current === false ? (
              <span className="obs-tag" data-testid="update-superseded-tag">
                {t('updatePage.supersededTag')}
              </span>
            ) : null}
          </div>
        </div>
      </header>

      <div className="update-cols2" data-testid="update-columns">
        <div className="update-stackv">
          <div data-testid="update-facts">
            <Section
              title={t('updatePage.factsTitle')}
              count={factSnapshot ? factRows.length : undefined}
              className="update-section"
            >
              {factsBody}
              {hiddenWorkCount > 0 ? (
                <div className="update-pnote" data-testid="update-facts-hidden">
                  {t('updatePage.factsHiddenNote', { count: hiddenWorkCount })}
                </div>
              ) : null}
              <div className="update-pnote" data-testid="update-facts-note">
                {t('updatePage.factsNote')}
              </div>
            </Section>
          </div>

          {detailPanel}

          <div data-testid="update-rollup">
            <Section title={t('updatePage.rollupTitle')} className="update-section">
              <RowList items={rollupItems} />
            </Section>
          </div>
        </div>

        <div className="update-stackv">
          <div data-testid="update-previous">
            <Section title={t('updatePage.previousTitle')} className="update-section">
              {previousBody}
            </Section>
          </div>

          <div data-testid="update-all">
            <Section
              title={t('updatePage.allUpdatesTitle')}
              count={!updatesListQuery.isPending && !updatesListQuery.isError ? allUpdates.length : undefined}
              className="update-section"
            >
              {allUpdatesBody}
            </Section>
          </div>
        </div>
      </div>

      <footer className="update-coverage-footer" data-testid="update-coverage">
        {conservativeFreshness ? <FreshnessBadge freshness={conservativeFreshness} /> : null}
        <div data-testid="update-coverage-summary">
          {coverageNotes.length > 0 ? coverageNotes.join(' · ') : t('updatePage.coverageComplete')}
        </div>
      </footer>
    </div>
  );
}
