import { useQuery } from '@tanstack/react-query';
import { Alert, Button, Descriptions, Empty, Flex, List, Space, Spin, Typography } from 'antd';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { readableSentence } from '../core/observation';
import { isAuthError } from '../lib/api';
import { handoffsQueryOptions } from '../lib/collaboration';
import { useRevocation } from '../lib/revocation';
import type { WorkSnapshotData } from '../lib/types';
import { RecordedTime } from './RecordedTime';
import { ObservationForbidden } from './states';

function record(value: unknown): Record<string, unknown> {
  return value && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : {};
}

const HANDOFF_STATUS = {
  proposed: 'proposed', inspected: 'inspected', accepted: 'accepted',
  rejected: 'rejected', cancelled: 'cancelled', expired: 'expired',
} as const;

export function WorkResponsibility({ project, workId, snapshot }: { project: string; workId: string; snapshot: WorkSnapshotData }) {
  const { t, i18n } = useTranslation();
  const { revoked } = useRevocation();
  const [cursors, setCursors] = useState<Array<string | null>>([null]);
  const handoffs = useQuery(handoffsQueryOptions(project, workId, cursors.at(-1) ?? null, revoked));
  const observation = snapshot.observation;
  const responsibility = observation?.responsibility;
  const claim = observation?.claim;
  const reporter = record(record(observation?.progress).origin).actor_name;
  const name = (value: unknown) => typeof value === 'string' && value.trim()
    ? readableSentence(value.trim(), i18n.language) : t('collaboration.nameMissing');
  const reservation = claim === null ? 'unreserved' : claim?.lease_live === true ? 'reserved'
    : claim?.lease_live === false ? 'reservationEnded' : 'unavailable';
  const owner = !responsibility ? t('collaboration.unavailable') : responsibility.owner === null
    ? t('collaboration.noOwner') : name(responsibility.owner_name);
  const executor = !responsibility ? t('collaboration.unavailable') : responsibility.current_executor === null
    ? t('collaboration.noExecutor') : name(responsibility.executor_person_name);
  const agent = !responsibility ? t('collaboration.unavailable') : responsibility.current_executor === null
    ? t('collaboration.noExecutor') : name(responsibility.executor_agent_name);
  const reportName = !observation || !('progress' in observation) ? t('collaboration.unavailable')
    : observation.progress === null ? t('collaboration.noReport') : name(reporter);
  const data = handoffs.data?.result.data;
  const refresh = () => cursors.length > 1 ? setCursors([null]) : void handoffs.refetch();
  if (revoked) return <ObservationForbidden />;

  return <Flex vertical gap={16} data-testid="work-responsibility">
    <Descriptions size="small" column={1} items={[
      { key: 'owner', label: t('collaboration.owner'), children: owner },
      { key: 'pending', label: t('collaboration.pending'), children: !responsibility ? t('collaboration.unavailable') : responsibility.pending === null
        ? t('collaboration.noAssignment') : name(responsibility?.pending_name) },
      { key: 'person', label: t('collaboration.executor'), children: executor },
      { key: 'agent', label: t('collaboration.agent'), children: agent },
      { key: 'reservation', label: t('collaboration.reservation'), children: observation && 'claim' in observation
        ? t(`collaboration.${reservation}`) : t('collaboration.unavailable') },
      { key: 'reporter', label: t('collaboration.reporter'), children: reportName },
    ]} />
    <div>
      <Typography.Title level={5}>{t('collaboration.handoffs')}</Typography.Title>
      {handoffs.isPending ? <Spin /> : null}
      {handoffs.error ? isAuthError(handoffs.error) ? <ObservationForbidden />
        : <Alert type="warning" showIcon message={t('collaboration.handoffsFailed')} description={t('collaboration.recover')} /> : null}
      {!handoffs.error && data ? <>
        {data.items.length === 0 ? <Empty description={t('collaboration.noHandoffs')} /> : <List dataSource={data.items}
          rowKey="handoff_id" renderItem={(item) => <List.Item>
            <Flex vertical gap={4}>
              <Typography.Text>{t('collaboration.handoffPeople', { from: name(item.from_display_name), to: name(item.to_display_name) })}</Typography.Text>
              <Typography.Text type="secondary">{t(item.kind === 'responsibility' ? 'collaboration.responsibilityHandoff' : 'collaboration.executionHandoff')}
                {' · '}{t(`collaboration.handoffStatus.${Object.hasOwn(HANDOFF_STATUS, item.effective_status) ? HANDOFF_STATUS[item.effective_status as keyof typeof HANDOFF_STATUS] : 'unknown'}`)}
              </Typography.Text>
              <RecordedTime value={item.updated_at_unix_ms} />
              {item.pending && item.expires_at_unix_ms !== null ? <Typography.Text type="secondary">
                {t('collaboration.expires')} <RecordedTime value={item.expires_at_unix_ms} />
              </Typography.Text> : null}
            </Flex>
          </List.Item>} />}
        <Typography.Text type="secondary">{t('collaboration.readAt')} <RecordedTime value={data.as_of.at_unix_ms} /></Typography.Text>
      </> : null}
    </div>
    <Space wrap>
      {cursors.length > 1 ? <Button size="small" onClick={() => setCursors(cursors.slice(0, -1))}>{t('plan.observation.previous')}</Button> : null}
      {!handoffs.error && data?.next_cursor ? <Button size="small" onClick={() => setCursors([...cursors, data.next_cursor])}>{t('plan.observation.next')}</Button> : null}
      <Button size="small" onClick={refresh}>{t('collaboration.refresh')}</Button>
    </Space>
  </Flex>;
}
