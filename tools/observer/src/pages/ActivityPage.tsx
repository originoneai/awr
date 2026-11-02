import { useInfiniteQuery, useQuery } from '@tanstack/react-query';
import { Alert, Badge, Button, Card, Col, Descriptions, Row, Select, Space, Table, Tabs, Typography, theme } from 'antd';
import { useMemo, type HTMLAttributes } from 'react';
import { useTranslation } from 'react-i18next';
import { Link, useSearchParams } from 'react-router';
import { TechnicalDetails } from '../components/TechnicalDetails';
import { ObservationEmpty, ObservationForbidden, PartialFailure } from '../components/states';
import {
  absoluteTimeText,
  actorAliases,
  ageText,
  describeUnknown,
  foldOperations,
  operationWord,
  statusWord,
  workLabel,
  type OperationEvent,
  type SemanticTone,
} from '../core/observation';
import { activityListQueryOptions, capabilitiesQueryOptions, type ActivityKind } from '../lib/activity';
import { eventTechnicalItems, pageTechnicalItems } from '../lib/activity-technical';
import { isAuthError } from '../lib/api';
import { projectsQueryOptions } from '../lib/queries';
import { useRevocation } from '../lib/revocation';
import { craftSelectionStyle } from '../theme';

type ActivityTab = ActivityKind | 'access';

/** The pause after which two records no longer read as one stretch of work (owner decision). */
const PAUSE_THRESHOLD_MS = 30 * 60 * 1000;
/** Stand-in value of the first entry of a filter list: no filter. */
const ALL = '__all__';

function tabOf(value: string | null): ActivityTab {
  return value === 'requests' || value === 'access' ? value : 'development';
}

function limitOf(value: string | null) {
  if (!value || !/^[1-9]\d*$/.test(value)) return 50;
  const parsed = Number(value);
  return Number.isSafeInteger(parsed) ? Math.min(parsed, 100) : 50;
}

function badgeStatus(tone: SemanticTone): 'success' | 'error' | 'warning' | 'default' {
  if (tone === 'positive') return 'success';
  if (tone === 'negative') return 'error';
  if (tone === 'warning') return 'warning';
  return 'default';
}

/** Bounded object activity, not the deferred full ops-audit/export surface. */
export function ActivityPage() {
  const { t, i18n } = useTranslation();
  const { token } = theme.useToken();
  const { revoked } = useRevocation();
  const [searchParams, setSearchParams] = useSearchParams();
  const projects = useQuery(projectsQueryOptions(revoked));
  const project = searchParams.get('project') ?? projects.data?.projects[0]?.key ?? '';
  const tab = tabOf(searchParams.get('kind'));
  const kind: ActivityKind = tab === 'requests' ? 'requests' : 'development';
  const limit = limitOf(searchParams.get('limit'));
  const capabilities = useQuery(capabilitiesQueryOptions(project, revoked));
  const identity = capabilities.data?.result.data.identity;
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
  const language = i18n.language;
  const folded = useMemo(() => foldOperations(
    (activity.data?.pages ?? []).flatMap((page) => page.data.items),
    { language, gapThresholdMs: PAUSE_THRESHOLD_MS },
  ), [activity.data, language]);
  const rows = tab === 'access' ? folded.uniqueEvents.filter((item) => item.action === 'access.changed') : folded.uniqueEvents;
  const firstPage = activity.data?.pages[0]?.data;
  const aliases = useMemo(() => actorAliases(
    folded.uniqueEvents.map((item) => item.actor_id),
    { language, selfActorId: identity?.actor_id },
  ), [folded, language, identity?.actor_id]);
  const workOptions = useMemo(() => {
    let unnamed = 0;
    return Array.from(new Set(folded.uniqueEvents.map((item) => item.work_id).filter((id): id is string => Boolean(id))))
      .map((value) => {
        const label = workLabel(value, { language });
        if (label.kind === 'unnamed') unnamed += 1;
        return { value, label: label.kind === 'unnamed' ? `${label.text} ${unnamed}` : label.text };
      });
  }, [folded, language]);
  const actorOptions = useMemo(() => {
    const entries = projectAudit
      ? aliases.entries
      : aliases.entries.filter((entry) => entry.actorId === identity?.actor_id);
    return entries.map((entry) => ({ value: entry.actorId, label: entry.label }));
  }, [aliases, projectAudit, identity?.actor_id]);

  function updateFilter(key: 'work' | 'actor', value?: string) {
    const next = new URLSearchParams(searchParams);
    if (value) next.set(key, value);
    else next.delete(key);
    next.delete('event');
    setSearchParams(next);
  }

  const selectedEvent = rows.find((item) => item.id === searchParams.get('event'));
  function selectEvent(id: string | undefined) {
    if (!id) return;
    const next = new URLSearchParams(searchParams);
    next.set('event', id);
    setSearchParams(next);
  }

  if (revoked || isAuthError(projects.error) || isAuthError(activity.error) || isAuthError(capabilities.error)) {
    return <ObservationForbidden />;
  }
  if (actorParam && capabilities.isError) {
    return (
      <Card>
        <div data-testid="activity-identity-error">
          <Alert
            type="error"
            showIcon
            message={t('activity.identityReadFailed')}
            action={<Button data-testid="activity-identity-retry" onClick={() => void capabilities.refetch()}>{t('activity.identityRetry')}</Button>}
          />
        </div>
      </Card>
    );
  }
  if (projects.isPending || (project && activity.isPending)) return <Card>{t('common.loading')}</Card>;
  if (projects.isError) return <PartialFailure errorCode="ActivityProjectsError">{null}</PartialFailure>;
  if (activity.isError) return <PartialFailure errorCode="ActivityReadError">{null}</PartialFailure>;

  const nowUnixMs = Date.now();
  const timeZone = Intl.DateTimeFormat().resolvedOptions().timeZone;
  const workLink = (workId: string) => `/projects/${encodeURIComponent(project)}?tab=acceptance&work=${encodeURIComponent(workId)}`;
  const scopeNote = firstPage?.scope === 'project' ? t('activity.scopeProject')
    : firstPage?.scope === 'self' ? t('activity.scopeSelf')
      : t('activity.scopeUnknown');
  const noChatOrToolIo = firstPage?.chat_text_collected === false && firstPage?.tool_io_collected === false;
  const retentionCount = firstPage?.request_retention === 'oldest_pruned_soft_capacity' && typeof firstPage?.request_retention_capacity === 'number'
    ? firstPage.request_retention_capacity : null;
  const foldNotes = Array.from(new Map(
    folded.disclosures.filter((item) => item.code !== 'time-gap').map((item) => [item.code, item]),
  ).values());
  const pageFacts = pageTechnicalItems(firstPage);

  function renderResult(value: unknown) {
    const word = statusWord(value, { language, field: 'result' });
    const tone: SemanticTone = value === 'unknown' ? 'warning' : word.tone;
    return <Badge status={badgeStatus(tone)} text={word.text} />;
  }

  function renderDetail(event: OperationEvent) {
    const action = operationWord(event.action, { language });
    const when = ageText(event.created_at_unix_ms, { language, nowUnixMs, timeZone });
    const exact = absoluteTimeText(event.created_at_unix_ms, { language });
    const work = workLabel(event.work_id, { language });
    const unknown = describeUnknown({ result: event.result, unknown_means: firstPage?.unknown_means, language });
    return (
      <Space direction="vertical" size={12} style={{ width: '100%' }} data-testid="activity-event-detail">
        <Typography.Title level={5} style={{ margin: 0 }} data-testid="activity-event-action">{action.text}</Typography.Title>
        <Descriptions
          size="small"
          column={1}
          items={[
            { key: 'actor', label: t('activity.detailActor'), children: <span data-testid="activity-event-actor">{aliases.labelOf(event.actor_id)}</span> },
            {
              key: 'work',
              label: t('activity.detailWork'),
              children: <span data-testid="activity-event-work">{work.text}</span>,
            },
            { key: 'result', label: t('activity.detailResult'), children: <span data-testid="activity-event-result">{renderResult(event.result)}</span> },
            { key: 'time', label: t('activity.detailTime'), children: <span data-testid="activity-event-time" title={exact.text}>{when.text}</span> },
          ]}
        />
        {unknown.isUnknown ? (
          <Alert
            type="warning"
            showIcon
            message={t('activity.unknownResult')}
            description={<span data-testid="activity-unknown-result">{unknown.disclosure}</span>}
          />
        ) : null}
        {event.work_id ? (
          <Link data-testid="activity-event-outcome-link" to={workLink(event.work_id)}>{t('activity.openOutcome')} →</Link>
        ) : null}
        <TechnicalDetails items={eventTechnicalItems(event)}>
          <Typography.Text type="secondary" data-testid="activity-event-not-request">{t('activity.auditRowNote')}</Typography.Text>
        </TechnicalDetails>
      </Space>
    );
  }

  return (
    <Space direction="vertical" size={16} style={{ width: '100%' }} data-testid="page-activity">
      <div>
        <Typography.Title level={4} style={{ margin: 0 }}>{t('activity.title')}</Typography.Title>
        <Typography.Text type="secondary">{t('activity.subtitle')}</Typography.Text>
      </div>
      <Card size="small">
        <Tabs
          activeKey={tab}
          onChange={(key) => {
            const next = new URLSearchParams(searchParams);
            next.set('kind', key);
            next.delete('event');
            setSearchParams(next);
          }}
          items={[
            { key: 'development', label: t('activity.development') },
            { key: 'requests', label: t('activity.requests') },
            { key: 'access', label: t('activity.access') },
          ]}
        />
        <Space size={12} wrap style={{ marginBottom: 12 }}>
          <Select
            aria-label={t('activity.workFilter')}
            data-testid="activity-work-filter"
            value={workFilter ?? ALL}
            style={{ minWidth: 190, maxWidth: 320 }}
            onChange={(value) => updateFilter('work', value === ALL ? undefined : value)}
            options={[{ value: ALL, label: t('activity.workFilter') }, ...workOptions]}
          />
          <Select
            aria-label={t('activity.actorFilter')}
            data-testid="activity-actor-filter"
            value={actorFilter ?? ALL}
            style={{ minWidth: 190, maxWidth: 320 }}
            onChange={(value) => updateFilter('actor', value === ALL ? undefined : value)}
            options={[{ value: ALL, label: t('activity.actorFilter') }, ...actorOptions]}
          />
          {capabilities.isSuccess && actorParam && !actorFilter && !projectAudit ? (
            <Typography.Text type="warning" data-testid="activity-self-filter-note">{t('activity.selfFilterNote')}</Typography.Text>
          ) : null}
        </Space>
        {tab === 'access' ? (
          <Typography.Paragraph type="secondary" data-testid="activity-access-note">{t('activity.accessNote')}</Typography.Paragraph>
        ) : null}
        {rows.length === 0 ? <ObservationEmpty /> : (
          <Row gutter={[16, 16]}>
            <Col xs={24} xl={16}>
              <Table<OperationEvent>
                size="small"
                rowKey={(record, index) => record.id ?? `row-${index}`}
                pagination={false}
                scroll={{ x: 640 }}
                dataSource={rows}
                onRow={(record) => ({
                  'data-testid': 'activity-row',
                  onClick: () => selectEvent(record.id),
                  style: craftSelectionStyle(token, Boolean(record.id) && record.id === selectedEvent?.id),
                } as HTMLAttributes<HTMLElement>)}
                columns={[
                  {
                    title: t('activity.time'), dataIndex: 'created_at_unix_ms', width: 150,
                    render: (value) => ageText(value, { language, nowUnixMs, timeZone }).text,
                  },
                  {
                    title: t('activity.actor'), dataIndex: 'actor_id', width: 120,
                    render: (value: string) => projectAudit || value === identity?.actor_id ? (
                      <Button
                        type="link"
                        style={{ padding: 0 }}
                        data-testid={`activity-actor-${value}`}
                        onClick={(event) => { event.stopPropagation(); updateFilter('actor', value); }}
                      >
                        {aliases.labelOf(value)}
                      </Button>
                    ) : <Typography.Text>{aliases.labelOf(value)}</Typography.Text>,
                  },
                  { title: t('activity.action'), dataIndex: 'action', width: 170, render: (value) => operationWord(value, { language }).text },
                  {
                    title: t('activity.work'), dataIndex: 'work_id', width: 170,
                    render: (value: string | null | undefined, record) => {
                      const label = workLabel(value, { language });
                      return value ? (
                        <Link data-testid={`activity-work-link-${value}`} to={workLink(value)} onClick={(event) => event.stopPropagation()}>
                          {label.text}
                        </Link>
                      ) : <span data-testid={`activity-work-empty-${record.id}`}>{label.text}</span>;
                    },
                  },
                  { title: t('activity.result'), dataIndex: 'result', width: 110, render: (value) => renderResult(value) },
                ]}
              />
            </Col>
            <Col xs={24} xl={8}>
              <div style={{ position: 'sticky', top: 16 }}>
                <Card size="small" title={t('activity.detail')}>
                  {selectedEvent ? renderDetail(selectedEvent) : <Typography.Text type="secondary">{t('activity.selectEvent')}</Typography.Text>}
                </Card>
              </div>
            </Col>
          </Row>
        )}
        <Space direction="vertical" size={4} style={{ marginTop: 12, width: '100%' }}>
          <Typography.Text type="secondary" data-testid="activity-count">{t('activity.shown', { count: rows.length })}</Typography.Text>
          {activity.hasNextPage ? (
            <Button
              data-testid="activity-load-earlier"
              loading={activity.isFetchingNextPage}
              onClick={() => activity.fetchNextPage()}
              style={{ alignSelf: 'flex-start' }}
            >
              {t('activity.loadEarlier')}
            </Button>
          ) : null}
        </Space>
      </Card>
      <Card size="small">
        <Space direction="vertical" size={4} style={{ width: '100%' }}>
          <Typography.Text type="secondary" data-testid="activity-scope">{scopeNote}</Typography.Text>
          {noChatOrToolIo ? <Typography.Text type="secondary" data-testid="activity-collection-limits">{t('activity.noChatOrToolIo')}</Typography.Text> : null}
          {tab === 'requests' && retentionCount != null ? (
            <Typography.Text type="secondary" data-testid="activity-retention">{t('activity.retention', { count: retentionCount })}</Typography.Text>
          ) : null}
          <Typography.Text type="secondary" data-testid="activity-operators-note">{t('activity.operatorsNote')}</Typography.Text>
          {foldNotes.map((note) => (
            <Typography.Text key={note.code} type="secondary" data-testid={`activity-note-${note.code}`}>{note.text}</Typography.Text>
          ))}
          <TechnicalDetails items={pageFacts} />
        </Space>
      </Card>
    </Space>
  );
}
