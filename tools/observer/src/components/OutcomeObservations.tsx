import { useInfiniteQuery, useQuery } from '@tanstack/react-query';
import { Alert, Button, Card, Empty, Flex, List, Space, Tag, Typography } from 'antd';
import { useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Link } from 'react-router';
import { actorAliases, operationWord, statusWord } from '../core/observation';
import { formatObservationValue } from '../i18n/craft';
import { activityListQueryOptions } from '../lib/activity';
import { isAuthError } from '../lib/api';
import { reviewRoundsQueryOptions } from '../lib/outcome';
import { RecordedTime } from './RecordedTime';
import { ObservationForbidden } from './states';

/** Select recorded reviews without changing the current outcome's acceptance basis. */
export function OutcomeReviewHistory({ project, workId, stream, currentRound, selectedRound, onSelect, revoked }: {
  project: string;
  workId: string;
  stream: string;
  currentRound?: string;
  selectedRound?: string;
  onSelect: (round: string) => void;
  revoked: boolean;
}) {
  const { t, i18n } = useTranslation();
  const [cursors, setCursors] = useState<Array<string | null>>([null]);
  const history = useQuery(reviewRoundsQueryOptions(project, workId, stream, cursors.at(-1) ?? null, revoked));
  const data = history.data?.result.data;
  const refresh = () => cursors.length > 1 ? setCursors([null]) : void history.refetch();
  if (revoked || isAuthError(history.error)) return <ObservationForbidden />;

  return <Card size="small" title={t('outcomeReviews.title')} data-testid="outcome-review-history">
    <Flex vertical gap={12}>
      {history.isPending ? <Typography.Text type="secondary">{t('common.loading')}</Typography.Text> : null}
      {history.isError ? <Alert type="warning" showIcon message={t('outcomeReviews.failed')} description={t('outcomeReviews.recover')} /> : null}
      {!history.isError && data?.items.length === 0 ? <Empty description={t('outcomeReviews.empty')} /> : null}
      {!history.isError && data ? <>
        <List dataSource={data.items} rowKey="round_id" renderItem={(round) => <List.Item>
          <Flex vertical gap={4} style={{ width: '100%' }}>
            <Flex justify="space-between" align="center" gap={8} wrap>
              <Button type={selectedRound === round.round_id ? 'default' : 'text'} size="small" disabled={!round.readable}
                aria-pressed={selectedRound === round.round_id}
                onClick={() => onSelect(round.round_id)}>
                {t('outcomeReviews.round', { count: round.round_index })}
                {currentRound === round.round_id ? ` · ${t('outcomeReviews.latest')}` : ''}
              </Button>
              {round.readable ? <Tag color={round.state === 'approved' ? 'success' : round.state === 'rejected' ? 'error' : 'default'}>
                {t(`outcomeReviews.states.${round.state ?? 'unknown'}`)}
              </Tag> : <Typography.Text type="secondary">{t('outcomeReviews.restricted')}</Typography.Text>}
            </Flex>
            {round.readable ? <>
              <Typography.Text type="secondary">
                {t(`outcomeReviews.roles.${round.reviewer_role ?? 'unknown'}`)}
                {' · '}<RecordedTime value={round.decided_at_unix_ms} />
              </Typography.Text>
              {round.contract_matches_current === false ? <Typography.Text type="secondary">{t('outcomeReviews.requirementsChanged')}</Typography.Text> : null}
              {round.reason_first_line ? <Typography.Paragraph style={{ margin: 0, overflowWrap: 'anywhere' }}>
                {formatObservationValue(round.reason_first_line, i18n.language)}
              </Typography.Paragraph> : null}
            </> : null}
          </Flex>
        </List.Item>} />
        <Typography.Text type="secondary">{t('outcomeReviews.readAt')} <RecordedTime value={data.as_of.at_unix_ms} /></Typography.Text>
      </> : null}
      <Space wrap>
        {currentRound && currentRound !== selectedRound ? <Button size="small" onClick={() => onSelect(currentRound)}>{t('outcomeReviews.returnLatest')}</Button> : null}
        {cursors.length > 1 ? <Button size="small" onClick={() => setCursors(cursors.slice(0, -1))}>{t('outcomeReviews.previous')}</Button> : null}
        {!history.isError && data?.next_cursor ? <Button size="small" onClick={() => setCursors([...cursors, data.next_cursor])}>{t('outcomeReviews.next')}</Button> : null}
        <Button size="small" onClick={refresh}>{t('outcomeReviews.refresh')}</Button>
      </Space>
    </Flex>
  </Card>;
}

/** Activity is filtered by work and permission on the service, with product-only labels. */
export function OutcomeActivity({ project, workId, revoked }: { project: string; workId: string; revoked: boolean }) {
  const { t, i18n } = useTranslation();
  const activity = useInfiniteQuery(activityListQueryOptions(project, 'development', { workId, limit: 20 }, revoked));
  const first = activity.data?.pages[0]?.data;
  const items = first?.items ?? [];
  const aliases = useMemo(() => actorAliases(items.map(item => item.actor_id), { language: i18n.language }), [items, i18n.language]);
  if (revoked || isAuthError(activity.error)) return <ObservationForbidden />;

  return <Card size="small" title={t('outcomeActivity.title')} data-testid="outcome-activity">
    <Flex vertical gap={12}>
      {activity.isPending ? <Typography.Text type="secondary">{t('common.loading')}</Typography.Text> : null}
      {activity.isError ? <Typography.Text type="warning">{t('outcomeActivity.failed')}</Typography.Text> : null}
      {!activity.isError && first ? <>
        <Typography.Text type="secondary">{t(first.scope === 'project' ? 'activity.scopeProject' : first.scope === 'self' ? 'activity.scopeSelf' : 'activity.scopeUnknown')}</Typography.Text>
        {items.length === 0 ? <Empty description={t('outcomeActivity.empty')} /> : <List dataSource={items.slice(0, 8)} rowKey="id" renderItem={(item) => <List.Item>
          <Flex vertical gap={4}>
            <Space wrap>
              <Typography.Text>{aliases.labelOf(item.actor_id)}</Typography.Text>
              <Typography.Text>{operationWord(item.action, { language: i18n.language }).text}</Typography.Text>
              <Typography.Text type={item.result === 'unknown' ? 'warning' : 'secondary'}>{item.result === 'unknown' ? t('acceptance.statusUnknown') : statusWord(item.result, { language: i18n.language, field: 'result' }).text}</Typography.Text>
            </Space>
            <Typography.Text type="secondary"><RecordedTime value={item.created_at_unix_ms} /></Typography.Text>
          </Flex>
        </List.Item>} />}
      </> : null}
      <Link to={`/activity?project=${encodeURIComponent(project)}&work=${encodeURIComponent(workId)}`}>{t('outcomeActivity.viewAll')}</Link>
    </Flex>
  </Card>;
}
