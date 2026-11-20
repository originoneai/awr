import { useInfiniteQuery, useQuery } from '@tanstack/react-query';
import { Button } from 'antd';
import { useMemo, type CSSProperties, type KeyboardEvent, type ReactNode } from 'react';
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
  ageText,
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
import { overviewModelQueryOptions, projectsQueryOptions } from '../lib/queries';
import { useRevocation } from '../lib/revocation';
import { useScope } from '../lib/scope';
import type { TeamActivityItem, TeamProject } from '../lib/types';
import { craftTokens } from '../theme';

type ActivityFilter = ActivityCategory | 'all';

interface ResolvedTask {
  id: string;
  title: string;
  href: string;
}

/** Prototype activity(): .t colgroup widths 168px, 232px and 24%. */
const ACTIVITY_COLUMN_WIDTH = { time: 168, actor: 232, task: '24%' } as const;

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
  const geometry = craftTokens.perspective;
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
    <div
      className="activity-segment"
      data-testid={testId}
      role="group"
      aria-label={label}
      style={{
        display: 'inline-flex',
        gap: geometry.gap,
        minWidth: 0,
        maxWidth: '100%',
        padding: geometry.padding,
        overflowX: 'auto',
        borderRadius: geometry.containerRadius,
        background: 'var(--craft-color-raised)',
        whiteSpace: 'nowrap',
      }}
    >
      {options.map((option, index) => {
        const active = option.value === value;
        return (
          <button
            key={option.value}
            type="button"
            className="shell-perspective-option"
            aria-pressed={active}
            onClick={() => onChange(option.value)}
            onKeyDown={(event) => onKeyDown(event, index)}
            style={{
              appearance: 'none',
              flex: '0 0 auto',
              height: geometry.optionHeight,
              margin: 0,
              padding: `0 ${geometry.optionPaddingInline}px`,
              border: 0,
              borderRadius: geometry.optionRadius,
              background: active ? 'var(--craft-color-surface)' : 'transparent',
              boxShadow: active ? 'var(--craft-perspective-selected-shadow)' : 'none',
              color: active ? 'var(--craft-color-ink)' : 'var(--craft-color-secondary)',
              cursor: 'pointer',
              fontFamily: 'inherit',
              fontSize: geometry.optionFontSize,
              fontWeight: active ? geometry.selectedFontWeight : geometry.fontWeight,
              whiteSpace: 'nowrap',
              transition: `background-color ${craftTokens.motion.feedbackMs}ms ${craftTokens.motion.easing}, color ${craftTokens.motion.feedbackMs}ms ${craftTokens.motion.easing}`,
            }}
          >
            {option.label}
          </button>
        );
      })}
    </div>
  );
}

function StateMessage({ children, testId }: { children: ReactNode; testId: string }) {
  return (
    <div
      className="activity-state"
      data-testid={testId}
      style={{
        padding: 'var(--ant-padding-md)',
        color: 'var(--craft-color-muted)',
        fontSize: 'var(--craft-text-small)',
        lineHeight: 'var(--craft-leading-body)',
      }}
    >
      {children}
    </div>
  );
}

function ellipsisStyle(): CSSProperties {
  return {
    display: 'block',
    minWidth: 0,
    overflow: 'hidden',
    textOverflow: 'ellipsis',
    whiteSpace: 'nowrap',
  };
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
  const members = useMemo(() => contextData?.input.members.items ?? [], [contextData]);
  const selectedEvent = folded.uniqueEvents.find((item) => item.id === searchParams.get('event'));
  const nowUnixMs = Date.now();
  const timeZone = Intl.DateTimeFormat().resolvedOptions().timeZone;

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

  function actorName(event: OperationEvent): string {
    const identityRecord = identity as unknown as Record<string, unknown> | undefined;
    const selfPersonId = event.actor_id === identity?.actor_id && typeof identityRecord?.person_id === 'string'
      ? identityRecord.person_id : null;
    const member = members.find((candidate) => candidate.personId === event.actor_id || candidate.personId === selfPersonId);
    const name = typeof member?.displayName === 'string' ? member.displayName.trim() : '';
    return name && name !== member?.personId && readsAsName(name) ? name : t('activity.unregisteredName');
  }

  function actorText(event: OperationEvent): string {
    const name = actorName(event);
    return event.client_id ? `${name} · ${t('activity.viaOwnAgent')}` : name;
  }

  function taskFor(workId: string | null): ResolvedTask | null {
    if (!workId) return null;
    const task = taskById.get(workId);
    if (!task) return null;
    const recorded = typeof task.title === 'string' && task.title.trim() && task.title.trim() !== workId ? task.title.trim() : null;
    const title = recorded
      ? workLabel(recorded, { language, recordedTitle: true, fallback: t('activity.unregisteredTask') }).text
      : t('activity.unregisteredTask');
    return {
      id: workId,
      title,
      href: `/projects/${encodeURIComponent(project)}?tab=acceptance&work=${encodeURIComponent(workId)}`,
    };
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
  const scopeNote = firstPage?.scope === 'project' ? t('activity.scopeProject')
    : firstPage?.scope === 'self' ? t('activity.scopeSelf') : t('activity.scopeUnknown');
  const retentionCount = kind === 'requests' && firstPage?.request_retention === 'oldest_pruned_soft_capacity'
    && typeof firstPage.request_retention_capacity === 'number' ? firstPage.request_retention_capacity : null;
  const foldNotes = Array.from(new Map(
    folded.disclosures.filter((item) => item.code !== 'time-gap').map((item) => [item.code, item]),
  ).values());
  const taskCoverage = contextData?.coverage.tasks.status ?? contextData?.input.tasks.coverage;
  const memberCoverage = contextData?.coverage.members.status ?? contextData?.input.members.coverage;
  const coverageText = context.isError ? t('activity.contextFailed')
    : context.isPending || taskCoverage === 'unread' || memberCoverage === 'unread' ? t('activity.coverageUnread')
      : taskCoverage === 'partial' || memberCoverage === 'partial' ? t('activity.coveragePartial')
        : t('activity.coverageComplete');
  const initialLoading = projects.isPending || waitingForActorIdentity || Boolean(project) && activity.isPending;
  const initialFailure = !initialLoading && (projects.isError || activity.isError && !activity.data || actorParam && capabilities.isError);

  const tableRows: DataTableRow[] = events.map((event, index) => {
    const task = taskFor(event.work_id);
    const validTime = Number.isFinite(event.created_at_unix_ms);
    const when = validTime ? ageText(event.created_at_unix_ms, { language, nowUnixMs, timeZone }).text : t('activity.timeNotRecorded');
    const exact = validTime ? absoluteTimeText(event.created_at_unix_ms, { language }).text : undefined;
    const actor = actorText(event);
    const action = operationWord(event.action, { language }).text;
    const actionText = event.result === 'unknown' ? `${action} · ${t('activity.unknownResult')}` : action;
    const workText = event.work_id ? task?.title ?? t('activity.unregisteredTask') : t('activity.wholeProject');
    return {
      key: event.id || `activity-row-${index}`,
      onOpen: task ? () => navigate(task.href) : undefined,
      cells: [
        <span style={{ color: 'var(--craft-color-muted)', fontVariantNumeric: 'tabular-nums' }} title={exact}>{when}</span>,
        <span style={{ ...ellipsisStyle(), fontWeight: 500 }} title={actor}>{actor}</span>,
        <span style={ellipsisStyle()} title={actionText}>{actionText}</span>,
        task ? (
          <Link to={task.href} style={ellipsisStyle()} title={task.title}>{task.title}</Link>
        ) : <span style={{ ...ellipsisStyle(), color: 'var(--craft-color-muted)' }}>{workText}</span>,
      ],
    };
  });

  function detail(event: OperationEvent) {
    const task = taskFor(event.work_id);
    const action = operationWord(event.action, { language }).text;
    const validTime = Number.isFinite(event.created_at_unix_ms);
    const when = validTime ? ageText(event.created_at_unix_ms, { language, nowUnixMs, timeZone }).text : t('activity.timeNotRecorded');
    const exact = validTime ? absoluteTimeText(event.created_at_unix_ms, { language }).text : undefined;
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
          { key: 'actor', label: t('activity.detailActor'), value: <span data-testid="activity-event-actor">{actorText(event as TeamActivityItem)}</span> },
          { key: 'action', label: t('activity.action'), value: <span data-testid="activity-event-action">{action}</span> },
          { key: 'work', label: t('activity.detailWork'), value: <span data-testid="activity-event-work">{event.work_id ? task?.title ?? t('activity.unregisteredTask') : t('activity.wholeProject')}</span> },
          { key: 'result', label: t('activity.detailResult'), value: <span data-testid="activity-event-result">{result}</span> },
          { key: 'time', label: t('activity.detailTime'), value: <span data-testid="activity-event-time" title={exact}>{when}</span> },
        ]} />
        {task ? <div style={{ padding: '0 var(--ant-padding-md) var(--ant-padding-sm)' }}><Link data-testid="activity-event-outcome-link" to={task.href}>{t('activity.openOutcome')} →</Link></div> : null}
      </div>
    );
  }

  const footer = (
    <div
      className="activity-pnote"
      style={{
        padding: 'calc(var(--ant-padding-xs) + var(--ant-padding-xxs)) var(--ant-padding-md)',
        borderTop: '1px solid var(--craft-color-hairline)',
        color: 'var(--craft-color-muted)',
        fontSize: 'var(--ant-font-size-sm)',
        lineHeight: 'var(--craft-leading-tight)',
        overflowWrap: 'anywhere',
      }}
    >
      <div data-testid="activity-collection-limits">{t('activity.collectionNote')}</div>
      <div data-testid="activity-scope" style={{ marginTop: 'var(--ant-padding-xs)' }}>{scopeNote}</div>
      {retentionCount != null ? <div data-testid="activity-retention" style={{ marginTop: 'var(--ant-padding-xs)' }}>{t('activity.retention', { count: retentionCount })}</div> : null}
      <div data-testid="activity-coverage" style={{ marginTop: 'var(--ant-padding-xs)' }}>{coverageText}</div>
      {capabilities.isSuccess && actorParam && !actorFilter && !projectAudit ? <div data-testid="activity-self-filter-note" style={{ marginTop: 'var(--ant-padding-xs)' }}>{t('activity.selfFilterNote')}</div> : null}
      {foldNotes.map((note) => <div key={note.code} data-testid={`activity-note-${note.code}`} style={{ marginTop: 'var(--ant-padding-xs)' }}>{note.text}</div>)}
      {activity.hasNextPage ? (
        <Button
          data-testid="activity-load-earlier"
          loading={activity.isFetchingNextPage}
          onClick={() => void activity.fetchNextPage()}
          style={{ alignSelf: 'flex-start', marginTop: 'var(--ant-padding-sm)' }}
        >
          {t('activity.loadEarlier')}
        </Button>
      ) : null}
      <div style={{ marginTop: 'var(--ant-padding-sm)' }}><FreshnessBadge freshness={contextData?.freshness} /></div>
    </div>
  );

  return (
    <div style={{ width: '100%', minWidth: 0 }} data-testid="page-activity">
      <PageHeader title={t('activity.title')} subtitle={t('activity.subtitle')} />
      <div
        className="activity-filters"
        style={{
          display: 'flex',
          alignItems: 'center',
          gap: 'calc(var(--ant-padding-xs) + var(--ant-padding-xxs))',
          flexWrap: 'wrap',
          minWidth: 0,
          marginBottom: 'var(--ant-padding-sm)',
        }}
      >
        <ActivitySegment label={t('activity.project')} value={project} options={projectOptions} onChange={changeProject} testId="activity-project-filter" />
        <ActivitySegment label={t('activity.category')} value={category} options={categoryOptions} onChange={changeCategory} testId="activity-category-filter" />
      </div>
      <Section title={t('activity.sectionTitle')} count={activity.data ? events.length : undefined} className="activity-section">
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
              {activity.isError && activity.data ? <div style={{ padding: 'var(--ant-padding-sm) var(--ant-padding-md) 0' }} data-testid="activity-partial"><Notice tone="warning">{t('activity.partialRead')}</Notice></div> : null}
              {events.length === 0 ? <StateMessage testId="activity-empty">{t('activity.empty')}</StateMessage>
                : <div className="activity-table"><DataTable columns={[
                  { key: 'time', title: t('activity.time'), width: ACTIVITY_COLUMN_WIDTH.time },
                  { key: 'actor', title: t('activity.actor'), width: ACTIVITY_COLUMN_WIDTH.actor },
                  { key: 'action', title: t('activity.action') },
                  { key: 'work', title: t('activity.work'), width: ACTIVITY_COLUMN_WIDTH.task, hideOnSmall: true },
                ]} rows={tableRows} /></div>}
            </>
          )}
        {footer}
      </Section>
      {selectedEvent ? <DetailPanel tabs={[{ key: 'detail', label: t('activity.detail'), content: detail(selectedEvent) }]} /> : null}
    </div>
  );
}
