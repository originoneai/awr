import { useQuery, useQueryClient } from '@tanstack/react-query';
import { Alert, Button, Card, Descriptions, Empty, Flex, Space, Tabs, Tag, Typography } from 'antd';
import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Link } from 'react-router';
import { ObservationForbidden } from '../components/states';
import { documentTitle } from '../core/observation';
import { ApiError, isAuthError } from '../lib/api';
import { formatUnixMs } from '../lib/format';
import { capabilitiesQueryOptions, ordinaryDecisionsQueryOptions } from '../lib/queries';
import { useRevocation } from '../lib/revocation';
import { useScope } from '../lib/scope';
import type { OrdinaryDecision } from '../lib/types';

/** Diagnostic-like statements stay in service history and never become interface text. */
function productText(value: string | null, fallback: string, language: string): string {
  const text = (value ?? '').trim();
  const lines = text.split(/\r\n?|\n/).filter((line) => line.trim());
  return lines.length > 0 && lines.every((line) => documentTitle(line, null, { language, position: 1 }).kind === 'named')
    ? text : fallback;
}

function QuestionCard({ item, project }: { item: OrdinaryDecision; project: string }) {
  const { t, i18n } = useTranslation();
  const text = (value: string | null, fallback: string) => productText(value, fallback, i18n.language);
  const person = (name: string | null) => text(name, t('inbox.nameMissing'));
  const changed = !item.baseline_matches_current && item.state !== 'cancelled';
  const status = changed ? 'changed' : item.state;
  const resolved = item.state !== 'open' && item.resolution !== null;
  return <Card data-testid="ordinary-question" title={text(item.question, t('inbox.questionMissing'))}
    styles={{ title: { whiteSpace: 'normal', overflowWrap: 'anywhere' }, header: { paddingTop: 16, paddingBottom: 16 } }}
    extra={<Tag color={status === 'replied' ? 'success' : status === 'cancelled' ? 'default' : 'warning'}>{t(`inbox.status.${status}`)}</Tag>}>
    <Space orientation="vertical" size={16} style={{ width: '100%' }}>
      <Link to={`/projects/${encodeURIComponent(project)}?tab=plan&work=${encodeURIComponent(item.work_id)}`}>
        {text(item.work_title, t('inbox.workMissing'))}
      </Link>
      {changed ? <Alert type="warning" showIcon title={t('inbox.changed')} /> : null}
      <section>
        <Typography.Text strong>{t('inbox.impact')}</Typography.Text>
        <Typography.Paragraph style={{ whiteSpace: 'pre-wrap', overflowWrap: 'anywhere', marginBottom: 0 }}>
          {text(item.impact, t('inbox.impactMissing'))}
        </Typography.Paragraph>
      </section>
      {resolved ? <section>
        <Typography.Text strong>{t(item.state === 'cancelled' ? 'inbox.withdrawal' : 'inbox.answer')}</Typography.Text>
        <Typography.Paragraph style={{ whiteSpace: 'pre-wrap', overflowWrap: 'anywhere', marginBottom: 0 }}>
          {text(item.reply, t('inbox.replyMissing'))}
        </Typography.Paragraph>
      </section> : null}
      <Descriptions size="small" column={{ xs: 1, sm: 2 }} colon={false} items={[
        { key: 'requester', label: t('inbox.requestedBy'), children: person(item.requester.display_name) },
        { key: 'raised', label: t('inbox.raisedAt'), children: formatUnixMs(item.created_at_unix_ms, i18n.language) },
        ...(resolved && item.resolution ? [
          { key: 'responder', label: t(item.state === 'cancelled' ? 'inbox.withdrawnBy' : 'inbox.answeredBy'), children: person(item.resolution.display_name) },
          { key: 'responded', label: t('inbox.respondedAt'), children: formatUnixMs(item.resolution.at_unix_ms, i18n.language) },
        ] : [{ key: 'responders', label: t('inbox.responders'), children: item.allowed_responders.map((entry) => person(entry.display_name)).join(i18n.language.startsWith('zh') ? '、' : ', ') }]),
      ]} />
    </Space>
  </Card>;
}

function ProjectQuestions({ project }: { project: string }) {
  const { t, i18n } = useTranslation();
  const { revoked } = useRevocation();
  const queryClient = useQueryClient();
  const [view, setView] = useState<'open' | 'all'>('open');
  const [paging, setPaging] = useState<{ cursor: string | null; previous: Array<string | null> }>({ cursor: null, previous: [] });
  const questions = useQuery(ordinaryDecisionsQueryOptions(project, view, paging.cursor, revoked));
  const expired = questions.error instanceof ApiError && questions.error.code === 'CursorExpired';
  const data = expired ? undefined : questions.data?.result.data;
  useEffect(() => {
    if (!expired || !paging.cursor) return;
    const queryKey = ['team', 'decisions.list', project];
    void queryClient.cancelQueries({ queryKey });
    queryClient.removeQueries({ queryKey });
    setPaging({ cursor: null, previous: [] });
  }, [expired, paging.cursor, project, queryClient]);
  const refresh = () => {
    setPaging({ cursor: null, previous: [] });
    if (!paging.cursor) void questions.refetch();
  };
  if (revoked || isAuthError(questions.error)) return <ObservationForbidden />;
  return <Space orientation="vertical" size={16} style={{ width: '100%' }} data-testid="project-questions">
    <Flex justify="space-between" align="center" gap={12} wrap>
      <Tabs activeKey={view} style={{ minWidth: 240 }} items={[
        { key: 'open', label: t('inbox.pending') }, { key: 'all', label: t('inbox.all') },
      ]} onChange={(key) => { setView(key as 'open' | 'all'); setPaging({ cursor: null, previous: [] }); }} />
      <Button aria-label={t('inbox.refresh')} loading={questions.isFetching} onClick={refresh}>{t('inbox.refresh')}</Button>
    </Flex>
    {questions.isError ? <Alert type="warning" showIcon title={t('inbox.failed')} action={<Button onClick={refresh}>{t('inbox.retry')}</Button>} /> : null}
    {questions.isPending ? <Typography.Text type="secondary">{t('common.loading')}</Typography.Text> : null}
    {data?.items.length === 0 ? <Empty description={t(view === 'open' ? 'inbox.emptyPending' : 'inbox.emptyAll')} /> : null}
    {data?.items.map((item) => <QuestionCard key={item.decision_id} project={project} item={item} />)}
    {data ? <Flex justify="space-between" align="center" gap={12} wrap>
      <Typography.Text type="secondary">{t('inbox.updated', { time: formatUnixMs(data.as_of.at_unix_ms, i18n.language) })}</Typography.Text>
      <Space wrap>
        <Button disabled={paging.previous.length === 0} onClick={() => setPaging({
          cursor: paging.previous.at(-1) ?? null, previous: paging.previous.slice(0, -1),
        })}>{t('inbox.previous')}</Button>
        <Button disabled={!data.next_cursor} onClick={() => setPaging({
          cursor: data.next_cursor, previous: [...paging.previous, paging.cursor],
        })}>{t('inbox.next')}</Button>
      </Space>
    </Flex> : null}
  </Space>;
}

function QuestionAvailability({ project }: { project: string }) {
  const { t } = useTranslation();
  const { revoked } = useRevocation();
  const capabilities = useQuery(capabilitiesQueryOptions(project, revoked));
  if (revoked || isAuthError(capabilities.error)) return <ObservationForbidden />;
  if (capabilities.isPending) return <Typography.Text type="secondary">{t('common.loading')}</Typography.Text>;
  if (capabilities.isError) return <Space orientation="vertical" size={12}>
    <Alert type="warning" title={t('inbox.failed')} />
    <Button onClick={() => void capabilities.refetch()}>{t('inbox.retry')}</Button>
  </Space>;
  if (capabilities.data?.result.queries?.includes('decisions.list')) return <ProjectQuestions project={project} />;
  return <Space orientation="vertical" size={12}>
    <Typography.Text strong>{t('inbox.unavailable')}</Typography.Text>
    <Typography.Text type="secondary">{t('inbox.unavailableHelp')}</Typography.Text>
    <Link to={`/workbench?scope=${encodeURIComponent(project)}&tab=needs-me`}>{t('inbox.openWorkbench')}</Link>
  </Space>;
}

export function InboxPage() {
  const { t } = useTranslation();
  const { projectKey } = useScope();
  return <Space orientation="vertical" size={16} style={{ width: '100%' }} data-testid="page-inbox">
    <Card>
      <Typography.Title level={3} style={{ marginTop: 0 }}>{t('nav.inbox')}</Typography.Title>
      <Typography.Paragraph type="secondary" style={{ marginBottom: 0 }}>{t('inbox.description')}</Typography.Paragraph>
    </Card>
    {projectKey ? <QuestionAvailability key={projectKey} project={projectKey} /> : <Card>
      <Space orientation="vertical" size={12}>
        <Typography.Text>{t('inbox.chooseProject')}</Typography.Text>
        <Link to="/projects">{t('inbox.openProjects')}</Link>
      </Space>
    </Card>}
  </Space>;
}
