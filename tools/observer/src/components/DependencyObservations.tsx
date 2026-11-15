import { useQuery } from '@tanstack/react-query';
import { Alert, Button, Card, Col, Descriptions, Empty, Flex, List, Row, Space, Spin, Tag, Typography, theme } from 'antd';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Link } from 'react-router';
import { readableSentence } from '../core/observation';
import { isAuthError } from '../lib/api';
import { dependenciesQueryOptions } from '../lib/collaboration';
import { useRevocation } from '../lib/revocation';
import type { DependencyObservation, DependencyVersion } from '../lib/types';
import { RecordedTime } from './RecordedTime';
import { ObservationForbidden } from './states';

function sameDelivery(left: DependencyVersion | null, right: DependencyVersion | null) {
  return Boolean(left?.receipt_id && right?.receipt_id
    && left.receipt_id === right.receipt_id && left.contract_hash === right.contract_hash);
}

/** Version relationships are product facts. Exact identities remain in service records. */
export function DependencyDetails({ item, project }: { item: DependencyObservation; project: string }) {
  const { t, i18n } = useTranslation();
  function versionLabel(field: 'required_version' | 'available_version' | 'adopted_version' | 'consumed_version') {
    if (!item[field]) {
      if (item.missing[field] === 'permission_restricted') return t('collaboration.restricted');
      if (field === 'adopted_version') return t(item.missing[field] === 'adoption_unavailable' ? 'collaboration.adoptionUnavailable' : 'collaboration.notAdopted');
      if (field === 'consumed_version') return t('collaboration.notConsumed');
      return t('collaboration.unavailable');
    }
    if (field === 'required_version') return t(item.version_policy === 'fixed_delivery' ? 'collaboration.pinnedRequirement' : 'collaboration.currentRequirement');
    if (field === 'available_version') return t('collaboration.acceptedDelivery');
    if (field === 'adopted_version') return t(sameDelivery(item.adopted_version, item.available_version) ? 'collaboration.matchesAvailable' : 'collaboration.otherDelivery');
    return t(sameDelivery(item.consumed_version, item.adopted_version) ? 'collaboration.matchesAdopted' : 'collaboration.otherInput');
  }
  const reasons = {
    permission_restricted: 'restricted', completion_unavailable: 'awaitingDelivery',
    adoption_not_recorded: 'adoptionNeeded', adoption_invalid: 'adoptionCheck', adoption_unavailable: 'adoptionCheck',
    upstream_contract_changed: 'requirementsChanged', execution_inputs_changed: 'inputsChanged',
    dependency_binding_invalidated: 'dependencyChanged', dependency_policy_mismatch: 'agreementCheck',
  } as const;
  const reason = item.reason && Object.hasOwn(reasons, item.reason) ? reasons[item.reason as keyof typeof reasons] : null;
  const next = item.validity === 'valid' ? 'continueWork'
    : item.reason === 'permission_restricted' ? 'checkAccess'
      : item.reason === 'completion_unavailable' ? 'waitForDelivery'
        : item.reason === 'adoption_not_recorded' ? 'askAdoption' : 'askRecheck';
  const assurance = typeof item.review_assurance === 'object' && item.review_assurance
    ? item.review_assurance.cross_workstream.review_assurance : item.review_assurance;
  const review = assurance === 'team_independent' ? 'teamReview'
    : assurance === 'simulated_member_independent' ? 'collaboratorReview'
      : assurance === 'agent_reviewed_caller_asserted_reconciled' ? 'agentReview' : 'unknownReview';
  const provider = item.provider_title ? readableSentence(item.provider_title, i18n.language) : t('collaboration.restrictedWork');
  return <Card size="small" title={t('collaboration.dependencyDetail')} data-testid="dependency-detail">
    <Flex vertical gap={16}>
      <Descriptions size="small" column={1} items={[
        { key: 'provider', label: t('collaboration.provider'), children: item.provider_title
          ? <Link to={`/projects/${encodeURIComponent(project)}?tab=plan&work=${encodeURIComponent(item.provider_work_id)}`}>{provider}</Link> : provider },
        { key: 'consumer', label: t('collaboration.consumer'), children: <Link to={`/projects/${encodeURIComponent(project)}?tab=plan&work=${encodeURIComponent(item.work_id)}`}>{readableSentence(item.work_title, i18n.language)}</Link> },
        { key: 'policy', label: t('collaboration.policy'), children: t(item.version_policy === 'fixed_delivery' ? 'collaboration.fixedPolicy' : 'collaboration.currentPolicy') },
        { key: 'review', label: t('collaboration.review'), children: t(`collaboration.${review}`) },
        { key: 'required', label: t('collaboration.required'), children: versionLabel('required_version') },
        { key: 'available', label: t('collaboration.available'), children: versionLabel('available_version') },
        { key: 'adopted', label: t('collaboration.adopted'), children: versionLabel('adopted_version') },
        { key: 'consumed', label: t('collaboration.consumed'), children: versionLabel('consumed_version') },
      ]} />
      <Alert showIcon type={item.validity === 'valid' ? 'success' : 'warning'}
        message={t(item.validity === 'valid' ? 'collaboration.deliveryAvailable' : 'collaboration.needsAttention')}
        description={reason ? t(`collaboration.${reason}`) : undefined} />
      <div>
        <Typography.Text strong>{t('collaboration.next')}</Typography.Text>
        <Typography.Paragraph style={{ marginBottom: 0 }}>{t(`collaboration.${next}`)}</Typography.Paragraph>
      </div>
    </Flex>
  </Card>;
}

export function DependencyObservations({ project, workId = null }: { project: string; workId?: string | null }) {
  const { t, i18n } = useTranslation();
  const { revoked } = useRevocation();
  const { token } = theme.useToken();
  const [cursors, setCursors] = useState<Array<string | null>>([null]);
  const [selected, setSelected] = useState<string | null>(null);
  const query = useQuery(dependenciesQueryOptions(project, workId, cursors.at(-1) ?? null, revoked));
  const data = query.data?.result.data;
  const keyOf = (item: DependencyObservation) => JSON.stringify([item.work_id, item.provider_work_id]);
  const current = data?.items.find((item) => keyOf(item) === selected) ?? data?.items[0];
  const refresh = () => cursors.length > 1 ? setCursors([null]) : void query.refetch();
  if (revoked || isAuthError(query.error)) return <ObservationForbidden />;

  return <Flex vertical gap={16} data-testid="dependency-observations">
    {query.isPending ? <Spin aria-label={t('common.loading')} /> : null}
    {query.error ? <Alert type="warning" showIcon message={t('collaboration.dependenciesFailed')} description={t('collaboration.recover')} /> : null}
    {!query.error && data ? <>
      <Typography.Text type="secondary">{t('collaboration.visibleDependencies', { count: data.total })}</Typography.Text>
      {data.items.length === 0 ? <Empty description={t('collaboration.noDependencies')} /> : <Row gutter={[16, 16]}>
        <Col xs={24} xl={12}>
          <List dataSource={data.items} rowKey={keyOf} renderItem={(item) => <List.Item>
            <Flex vertical gap={4} style={{ width: '100%', minWidth: 0 }}>
              <Button type={current === item ? 'default' : 'text'} block aria-pressed={current === item}
                aria-label={`${readableSentence(item.work_title, i18n.language)} · ${t('collaboration.dependsOn', {
                  name: item.provider_title ? readableSentence(item.provider_title, i18n.language) : t('collaboration.restrictedWork'),
                })}`} onClick={() => setSelected(keyOf(item))}
                style={{ height: 'auto', minHeight: token.controlHeight, whiteSpace: 'normal', textAlign: 'start', justifyContent: 'start' }}>
                {readableSentence(item.work_title, i18n.language)}
              </Button>
              <Typography.Text type="secondary">{t('collaboration.dependsOn', {
                name: item.provider_title ? readableSentence(item.provider_title, i18n.language) : t('collaboration.restrictedWork'),
              })}</Typography.Text>
              <Tag style={{ alignSelf: 'start' }} color={item.validity === 'valid' ? 'success' : 'warning'}>
                {t(item.validity === 'valid' ? 'collaboration.deliveryAvailable' : 'collaboration.needsAttention')}
              </Tag>
            </Flex>
          </List.Item>} />
        </Col>
        <Col xs={24} xl={12}>{current ? <DependencyDetails item={current} project={project} /> : null}</Col>
      </Row>}
      <Typography.Text type="secondary">{t('collaboration.readAt')} <RecordedTime value={data.as_of.at_unix_ms} /></Typography.Text>
    </> : null}
    <Space wrap>
      {cursors.length > 1 ? <Button onClick={() => setCursors(cursors.slice(0, -1))}>{t('plan.observation.previous')}</Button> : null}
      {!query.error && data?.next_cursor ? <Button onClick={() => setCursors([...cursors, data.next_cursor])}>{t('plan.observation.next')}</Button> : null}
      <Button aria-label={t('collaboration.refresh')} loading={query.isFetching} onClick={refresh}>{t('collaboration.refresh')}</Button>
    </Space>
  </Flex>;
}
