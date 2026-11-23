import { useInfiniteQuery, useQuery } from '@tanstack/react-query';
import { Button } from 'antd';
import { useMemo, type KeyboardEvent, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { Link, useNavigate, useSearchParams } from 'react-router';

import { FreshnessBadge } from '../components/FreshnessBadge';
import { DataTable, type DataTableRow } from '../components/display/DataTable';
import { DetailPanel } from '../components/display/DetailPanel';
import { KeyValueTable } from '../components/display/KeyValueTable';
import { Notice } from '../components/display/Notice';
import { PageHeader } from '../components/display/PageHeader';
import { Section } from '../components/display/Section';
import { ObservationForbidden } from '../components/states';
import {
  absoluteTimeText,
  compactTimeText,
  describeUnknown,
  foldOperations,
  operationWord,
  statusWord,
  workLabel,
  type OperationEvent,
} from '../core/observation';
import { readsAsName } from '../core/observation/owner';
import {
  ACTIVITY_CATEGORIES,
  activityCategoryOf,
  type ActivityCategory,
} from '../i18n/activity-categories';
import { activityListQueryOptions, capabilitiesQueryOptions, type ActivityKind } from '../lib/activity';
import { isAuthError } from '../lib/api';
import { readClockOffsetUnixMs, referenceNowUnixMs, useDisplayTimeZone } from '../lib/display-clock';
import { overviewModelQueryOptions, projectsQueryOptions } from '../lib/queries';
import { useRevocation } from '../lib/revocation';
import { useScope } from '../lib/scope';
import { taskPath } from '../lib/task-path';
import type { TeamProject } from '../lib/types';
import { craftTokens } from '../theme';
import './ActivityPage.css';

type ActivityFilter = ActivityCategory | 'all';

interface ResolvedTask {
  title: string;
  href: string;
}

function kindOf(value: string | null): ActivityKind {
  return value === 'requests' ? 'requests' : 'development';
}

function categoryOf(value: string | null): ActivityFilter {
  return (ACTIVITY_CATEGORIES as readonly string[]).includes(value ?? '') ? value as ActivityCategory : 'all';
}

function limitOf(value: string | null) {
  if (!value || !/^[1-9]\d*$/.test(value)) return 50;
  const parsed = Number(value);
  return Number.isSafeInteger(parsed) ? Math.min(parsed, 100) : 50;
}

function projectName(project: TeamProject, language: string, fallback: string) {
  const title = typeof project.title === 'string' && project.title.trim() !== project.key ? project.title.trim() : null;
  if (!title) return fallback;
  const label = workLabel(title, { language, recordedTitle: true, fallback });
  return label.kind === 'named' ? label.text : fallback;
}

function ActivitySegment({
  label,
  value,
  options,
  onChange,
  testId,
}: {
  label: string;
  value: string;
  options: readonly { value: string; label: string }[];
  onChange: (value: string) => void;
  testId: string;
}) {
  const onKeyDown = (event: KeyboardEvent<HTMLButtonElement>, index: number) => {
    let nextIndex: number | null = null;
    if (event.key === 'ArrowRight' || event.key === 'ArrowDown') nextIndex = (index + 1) % options.length;
    if (event.key === 'ArrowLeft' || event.key === 'ArrowUp') nextIndex = (index - 1 + options.length) % options.length;
    if (event.key === 'Home') nextIndex = 0;
    if (event.key === 'End') nextIndex = options.length - 1;
    if (nextIndex === null) return;
    event.preventDefault();
    const buttons = event.currentTarget.parentElement?.querySelectorAll<HTMLButtonElement>('button');
    buttons?.[nextIndex]?.focus();
  };
  return (
    <div className="activity-segment" data-testid={testId} role="group" aria-label={label}>
      {options.map((option, index) => {
        const active = option.value === value;
        return (
          <button
            key={option.value}
            type="button"
            className="activity-segment-option shell-perspective-option"
            aria-pressed={active}
            onClick={() => onChange(option.value)}
            onKeyDown={(event) => onKeyDown(event, index)}
          >
            {option.label}
          </button>
        );
      })}
    </div>
  );
}

function StateMessage({ children, testId }: { children: ReactNode; testId: string }) {
  return <div className="activity-state" data-testid={testId}>{children}</div>;
}

/** Prototype activity(): heading, project/category filters, one ledger block and its collection note. */
export function ActivityPage() {
  const { t, i18n } = useTranslation();
  const language = i18n.language;
  const navigate = useNavigate();
  const { revoked } = useRevocation();
  const { projectKey } = useScope();
  const [searchParams, setSearchParams] = useSearchParams();
  const projects = useQuery(projectsQueryOptions(revoked));
  const project = projectKey ?? searchParams.get('project') ?? projects.data?.projects[0]?.key ?? '';
  const kind = kindOf(searchParams.get('kind'));
  const category = categoryOf(searchParams.get('category'));
  const limit = limitOf(searchParams.get('limit'));
  const capabilities = useQuery(capabilitiesQueryOptions(project, revoked));
  const identity = capabilities.data?.result.identity;
  const workFilter = searchParams.get('work') ?? undefined;
  const actorParam = searchParams.get('actor') ?? undefined;
  const projectAudit = identity?.can_read_project_audit === true;
  const actorFilter = actorParam && (projectAudit || actorParam === identity?.actor_id) ? actorParam : undefined;
  const waitingForActorIdentity = Boolean(actorParam) && (capabilities.isPending || capabilities.isError);
  const activityOptions = activityListQueryOptions(project, kind, {
    workId: workFilter, actorId: actorFilter, limit,
  }, revoked);
  const activity = useInfiniteQuery({
    ...activityOptions,
    enabled: activityOptions.enabled && !waitingForActorIdentity,
  });
  // This existing composed read supplies task titles and registered member names. No new bridge route.
  const context = useQuery({
    ...overviewModelQueryOptions(project || null, revoked),
    enabled: Boolean(project) && !revoked,
  });

  const folded = useMemo(() => foldOperations(
    (activity.data?.pages ?? []).flatMap((page) => page.data.items),
    { language, gapThresholdMs: 30 * 60 * 1000 },
  ), [activity.data, language]);
  const events = useMemo(
    () => folded.uniqueEvents.filter((item) => category === 'all' || activityCategoryOf(item.action) === category),
    [folded.uniqueEvents, category],
  );
  const firstPage = activity.data?.pages[0]?.data;
  const contextData = context.data?.project === project ? context.data : undefined;
  const taskById = useMemo(() => new Map(
    (contextData?.input.tasks.items ?? []).flatMap((task) => typeof task.id === 'string' && task.id ? [[task.id, task] as const] : []),
  ), [contextData]);
  const selectedEvent = folded.uniqueEvents.find((item) => item.id === searchParams.get('event'));
  // Display clock (AWR-PPO-079): the reference is this page's own overview read's
  // derivation clock walked forward with real time, and the zone is the deployment's
  // pinned display zone (canonical demo: UTC; otherwise the viewer's). The browser clock
  // itself is only the fallback while no read has answered.
  const nowUnixMs = referenceNowUnixMs(readClockOffsetUnixMs(contextData));
  const timeZone = useDisplayTimeZone();

  function changeProject(nextProject: string) {
    const next = new URLSearchParams(searchParams);
    next.set('project', nextProject);
    next.set('scope', nextProject);
    for (const key of ['category', 'event', 'work', 'actor']) next.delete(key);
    setSearchParams(next);
  }

  function changeCategory(nextCategory: string) {
    const next = new URLSearchParams(searchParams);
    if (nextCategory === 'all') next.delete('category');
    else next.set('category', nextCategory);
    next.delete('event');
    setSearchParams(next);
  }

  function actorText(event: OperationEvent): string {
    // AWR-PPD-055: the audit record carries its own identity disclosure. The person's
    // display name is the fact; a machine identifier in that field is never shown as a
    // name. The client is named only when the record discloses it (AWR-PPD-062: the name
    // the client registered for itself). The sentence form follows the recorded actor
    // kind: an Agent's run reads "client (name)", a person acting through a client reads
    // "name (via client)". Every real request carries a client id, so that id cannot
    // tell the two apart. Without a recorded kind the record cannot say who acted, so
    // only the person's name shows.
    const recordedName = typeof event.actor?.display_name === 'string' ? event.actor.display_name.trim() : '';
    const placeholder = /\b(?:undefined|null|NaN)\b/i;
    const name = recordedName && readsAsName(recordedName) && !placeholder.test(recordedName)
      ? recordedName : t('activity.unregisteredName');
    const recordedClient = typeof event.client?.display_name === 'string' ? event.client.display_name.trim() : '';
    if (!recordedClient || !readsAsName(recordedClient) || placeholder.test(recordedClient)) return name;
    if (event.actor?.kind === 'agent') return t('activity.actorByClient', { client: recordedClient, name });
    if (event.actor?.kind === 'human') return t('activity.actorViaClient', { client: recordedClient, name });
    return name;
  }

  function taskFor(workId: string | null): ResolvedTask | null {
    if (!workId) return null;
    const task = taskById.get(workId);
    if (!task) return null;
    const recorded = typeof task.title === 'string' && task.title.trim() && task.title.trim() !== workId ? task.title.trim() : null;
    const title = recorded
      ? workLabel(recorded, { language, recordedTitle: true, fallback: t('activity.unregisteredTask') }).text
      : t('activity.unregisteredTask');
    return { title, href: taskPath(project, workId) };
  }

  if (revoked || isAuthError(projects.error) || isAuthError(activity.error)
    || isAuthError(capabilities.error) || isAuthError(context.error)) {
    return <ObservationForbidden />;
  }

  const projectOptions = (projects.data?.projects ?? []).map((item) => ({
    value: item.key,
    label: projectName(item, language, t('activity.unregisteredProject')),
  }));
  const categoryOptions = [
    { value: 'all', label: t('activity.categories.all') },
    ...ACTIVITY_CATEGORIES.map((item) => ({ value: item as string, label: t(`activity.categories.${item}`) })),
  ];
  const scopeNote = firstPage?.scope === 'self' ? t('activity.scopeSelf')
    : firstPage?.scope === 'project' || !activity.data ? null : t('activity.scopeUnknown');
  const retentionCount = kind === 'requests' && firstPage?.request_retention === 'oldest_pruned_soft_capacity'
    && typeof firstPage.request_retention_capacity === 'number' ? firstPage.request_retention_capacity : null;
  const foldNotes = Array.from(new Map(
    folded.disclosures.filter((item) => item.code !== 'time-gap').map((item) => [item.code, item]),
  ).values());
  const taskCoverage = contextData?.coverage.tasks.status ?? contextData?.input.tasks.coverage;
  const memberCoverage = contextData?.coverage.members.status ?? contextData?.input.members.coverage;
  // The prototype's footnote is one sentence; read problems still get their quiet lines.
  // A completed, project-scoped read with complete context says nothing more.
  const coverageNote = context.isError ? t('activity.contextFailed')
    : !contextData ? null
      : taskCoverage === 'partial' || memberCoverage === 'partial' ? t('activity.coveragePartial')
        : taskCoverage !== 'complete' || memberCoverage !== 'complete' ? t('activity.coverageUnread')
          : null;
  const FRESHNESS_PLANES = ['report_received', 'authoritative', 'page_read'] as const;
  const freshnessNeedsNote = contextData != null && FRESHNESS_PLANES
    .some((plane) => ['stale', 'failed'].includes(contextData.freshness?.[plane]?.status ?? ''));
  const initialLoading = projects.isPending || waitingForActorIdentity || Boolean(project) && activity.isPending;
  const initialFailure = !initialLoading && (projects.isError || activity.isError && !activity.data || actorParam && capabilities.isError);

  const tableRows: DataTableRow[] = events.map((event, index) => {
    const task = taskFor(event.work_id);
    const validTime = Number.isFinite(event.created_at_unix_ms);
    const when = validTime
      ? compactTimeText(event.created_at_unix_ms, { language, nowUnixMs, timeZone })
      : t('activity.timeNotRecorded');
    const exact = validTime ? absoluteTimeText(event.created_at_unix_ms, { language, timeZone }).text : undefined;
    const actor = actorText(event);
    const action = operationWord(event.action, { language }).text;
    const actionText = event.result === 'unknown' ? `${action} · ${t('activity.unknownResult')}` : action;
    const workText = event.work_id ? task?.title ?? t('activity.unregisteredTask') : t('activity.wholeProject');
    return {
      key: event.id || `activity-row-${index}`,
      onOpen: task ? () => navigate(task.href) : undefined,
      cells: [
        <span className="activity-ellipsis activity-time" title={exact}>{when}</span>,
        <span className="activity-ellipsis activity-actor" title={actor}>{actor}</span>,
        <span className="activity-ellipsis activity-action" title={actionText}>{actionText}</span>,
        task ? (
          <Link className="activity-ellipsis activity-task-link" to={task.href} title={task.title}>{task.title}</Link>
        ) : <span className="activity-ellipsis activity-task-empty">{workText}</span>,
      ],
    };
  });

  function detail(event: OperationEvent) {
    const task = taskFor(event.work_id);
    const action = operationWord(event.action, { language }).text;
    const validTime = Number.isFinite(event.created_at_unix_ms);
    const when = validTime
      ? compactTimeText(event.created_at_unix_ms, { language, nowUnixMs, timeZone })
      : t('activity.timeNotRecorded');
    const exact = validTime ? absoluteTimeText(event.created_at_unix_ms, { language, timeZone }).text : undefined;
    const result = statusWord(event.result, { language, field: 'result' }).text;
    const unknown = describeUnknown({ result: event.result, unknown_means: firstPage?.unknown_means, language });
    return (
      <div data-testid="activity-event-detail">
        {unknown.isUnknown ? (
          <Notice tone="warning" title={t('activity.unknownResult')}>
            <span data-testid="activity-unknown-result">{unknown.disclosure}</span>
          </Notice>
        ) : null}
        <KeyValueTable items={[
          { key: 'actor', label: t('activity.detailActor'), value: <span data-testid="activity-event-actor">{actorText(event)}</span> },
          { key: 'action', label: t('activity.action'), value: <span data-testid="activity-event-action">{action}</span> },
          { key: 'work', label: t('activity.detailWork'), value: <span data-testid="activity-event-work">{event.work_id ? task?.title ?? t('activity.unregisteredTask') : t('activity.wholeProject')}</span> },
          { key: 'result', label: t('activity.detailResult'), value: <span data-testid="activity-event-result">{result}</span> },
          { key: 'time', label: t('activity.detailTime'), value: <span data-testid="activity-event-time" title={exact}>{when}</span> },
        ]} />
        {task ? <div className="activity-detail-outcome"><Link data-testid="activity-event-outcome-link" to={task.href}>{t('activity.openOutcome')} →</Link></div> : null}
      </div>
    );
  }

  const footer = (
    <div className="activity-pnote">
      <div data-testid="activity-collection-limits">{t('activity.collectionNote')}</div>
      {scopeNote || coverageNote ? (
        <div className="activity-scope-line">
          {scopeNote ? <span data-testid="activity-scope">{scopeNote}</span> : null}
          {scopeNote && coverageNote ? <span aria-hidden>·</span> : null}
          {coverageNote ? <span data-testid="activity-coverage">{coverageNote}</span> : null}
        </div>
      ) : null}
      {retentionCount != null ? <div className="activity-retention" data-testid="activity-retention">{t('activity.retention', { count: retentionCount })}</div> : null}
      {capabilities.isSuccess && actorParam && !actorFilter && !projectAudit ? <div className="activity-self-filter-note" data-testid="activity-self-filter-note">{t('activity.selfFilterNote')}</div> : null}
      {foldNotes.map((note) => <div className="activity-fold-note" key={note.code} data-testid={`activity-note-${note.code}`}>{note.text}</div>)}
      {activity.hasNextPage || freshnessNeedsNote ? (
        <div className="activity-footer-controls">
          {activity.hasNextPage ? (
            <Button
              className="activity-load-earlier"
              data-testid="activity-load-earlier"
              loading={activity.isFetchingNextPage}
              onClick={() => void activity.fetchNextPage()}
            >
              {t('activity.loadEarlier')}
            </Button>
          ) : null}
          {freshnessNeedsNote ? <div className="activity-freshness"><FreshnessBadge freshness={contextData?.freshness} /></div> : null}
        </div>
      ) : null}
    </div>
  );

  return (
    <div className="activity-page" data-testid="page-activity">
      <PageHeader title={t('activity.title')} subtitle={t('activity.subtitle')} />
      <div className="activity-filters">
        <ActivitySegment label={t('activity.project')} value={project} options={projectOptions} onChange={changeProject} testId="activity-project-filter" />
        <ActivitySegment label={t('activity.category')} value={category} options={categoryOptions} onChange={changeCategory} testId="activity-category-filter" />
      </div>
      {/* The prototype's block-head reads [h2][8px][count][auto]; the count sits beside the
          title, not inside it, so the measured title stays the h2's own box. */}
      <Section
        title={t('activity.sectionTitle')}
        extra={activity.data ? <span className="obs-section-count activity-section-count">{events.length}</span> : undefined}
        className="activity-section"
      >
        {initialLoading ? <StateMessage testId="activity-loading">{t('activity.readLoading')}</StateMessage>
          : initialFailure ? (
            <StateMessage testId="activity-read-error">
              <Notice
                tone="warning"
                title={t('activity.readFailed')}
                extra={<Button data-testid="activity-retry" onClick={() => void (actorParam && capabilities.isError ? capabilities.refetch() : projects.isError ? projects.refetch() : activity.refetch())}>{t('activity.retry')}</Button>}
              >{null}</Notice>
            </StateMessage>
          ) : (
            <>
              {activity.isError && activity.data ? <div className="activity-partial" data-testid="activity-partial"><Notice tone="warning">{t('activity.partialRead')}</Notice></div> : null}
              {events.length === 0 ? <StateMessage testId="activity-empty">{t('activity.empty')}</StateMessage>
                : <div className="activity-table"><DataTable columns={[
                  { key: 'time', title: t('activity.time'), width: craftTokens.activity.columnTime, numeric: true },
                  { key: 'actor', title: t('activity.actor'), width: craftTokens.activity.columnActor },
                  { key: 'action', title: t('activity.action') },
                  { key: 'work', title: t('activity.work'), width: craftTokens.activity.columnTask, hideOnSmall: true },
                ]} rows={tableRows} /></div>}
            </>
          )}
        {footer}
      </Section>
      {selectedEvent ? <DetailPanel tabs={[{ key: 'detail', label: t('activity.detail'), content: detail(selectedEvent) }]} /> : null}
    </div>
  );
}
