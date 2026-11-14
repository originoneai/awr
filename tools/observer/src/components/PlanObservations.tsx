import { useQuery } from '@tanstack/react-query';
import { Alert, Button, Card, Empty, Flex, List, Space, Spin, Typography } from 'antd';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { documentLabel, readableSentence } from '../core/observation';
import { isAuthError } from '../lib/api';
import { formatUnixMs } from '../lib/format';
import { planningCandidatesQueryOptions, requiredSourcesQueryOptions } from '../lib/queries';
import { useRevocation } from '../lib/revocation';
import type { PlanningCandidatesData } from '../lib/types';
import { ObservationForbidden } from './states';

function PageButtons({ previous, next, refresh }: { previous?: () => void; next?: () => void; refresh: () => void }) {
  const { t } = useTranslation();
  return <Space wrap>
    {previous ? <Button size="small" onClick={previous}>{t('plan.observation.previous')}</Button> : null}
    {next ? <Button size="small" onClick={next}>{t('plan.observation.next')}</Button> : null}
    <Button size="small" aria-label={t('plan.observation.refresh')} onClick={refresh}>{t('plan.observation.refresh')}</Button>
  </Space>;
}

function proposalStatus(item: PlanningCandidatesData['items'][number]) {
  if (item.activation.value) return item.activation.value.matches_current_snapshot ? 'applied' : 'previouslyApplied';
  if (item.state === 'superseded') return 'superseded';
  if (item.publication.value) return 'applying';
  if (item.approval.value) return 'approved';
  return item.state === 'drafting' ? 'awaitingReview' : 'unknown';
}

function ChangeDetails({ fields }: { fields: NonNullable<PlanningCandidatesData['items'][number]['current_diff']['items'][number]['fields']> }) {
  const { t, i18n } = useTranslation();
  const labels = { title: 'name', acceptance: 'acceptance', hard_rules: 'rules', verification_requirements: 'checks',
    goals: 'goals', required_dependencies: 'dependencies', dependency_acceptance: 'upstreamReview',
    execution_settlement: 'resultConfirmation', scope_paths: 'scope', completion_policy: 'review', definition_state: 'availability' } as const;
  function content(item: { value: unknown; reason?: string | null }, field: string, prose: boolean) {
    if (item.reason === 'permission_restricted') return t('plan.changes.restricted');
    if (item.value == null) return t('plan.changes.notSet');
    if (field === 'dependency_acceptance' && typeof item.value === 'object' && !Array.isArray(item.value)) {
      const descriptions = new Set<string>();
      for (const policy of Object.values(item.value)) {
        if (policy === 'agent_reviewed_caller_asserted_reconciled') descriptions.add(t('plan.changes.policy.agentReviewed'));
        else if (policy === 'simulated_member_independent') descriptions.add(t('plan.changes.policy.collaboratorReviewed'));
        else if (policy?.cross_workstream?.version_policy === 'fixed_delivery') descriptions.add(t('plan.changes.policy.fixedResults'));
        else if (policy?.cross_workstream?.version_policy === 'current_contract') descriptions.add(t('plan.changes.policy.currentResults'));
        else descriptions.add(t('plan.changes.policy.needsCheck'));
      }
      return [...descriptions].join(' · ') || t('plan.changes.notSet');
    }
    if (field === 'execution_settlement' && typeof item.value === 'object' && !Array.isArray(item.value)) {
      const mode = (item.value as Record<string, unknown>).mode;
      if (mode === 'independent_workspace_v1') return t('plan.changes.policy.confirmWhileAuthorized');
      if (mode === 'independent_workspace_v2') return t('plan.changes.policy.confirmAfterExpiry');
      return t('plan.changes.policy.needsCheck');
    }
    if (!prose) return t('plan.changes.policy.availableToAgent');
    const text = typeof item.value === 'string' ? item.value : Array.isArray(item.value)
      ? item.value.filter((v): v is string => typeof v === 'string').join(' · ') : '';
    return text ? readableSentence(text, i18n.language) : t('plan.changes.notSet');
  }
  return <>{fields.map((field, index) => {
    const label = Object.hasOwn(labels, field.field) ? labels[field.field as keyof typeof labels] : 'other';
    const prose = ['title', 'acceptance', 'hard_rules', 'verification_requirements'].includes(field.field);
    const describe = prose || ['dependency_acceptance', 'execution_settlement'].includes(field.field)
      || field.before.reason === 'permission_restricted' || field.after.reason === 'permission_restricted';
    return <div key={index}>
      <Typography.Text type="secondary">{t(`plan.changes.fields.${label}`)}</Typography.Text>
      {describe ? <Flex vertical gap={4}>
        <Typography.Text type="secondary">{t('plan.changes.before', { text: content(field.before, field.field, prose) })}</Typography.Text>
        <Typography.Text>{t('plan.changes.after', { text: content(field.after, field.field, prose) })}</Typography.Text>
      </Flex> : null}
    </div>;
  })}</>;
}

/** Only business facts are rendered; exact revisions, selectors and digests stay in service history. */
export function PlanningChanges({ project, stream }: { project: string; stream: string | null }) {
  const { t, i18n } = useTranslation();
  const { revoked } = useRevocation();
  const [cursors, setCursors] = useState<Array<string | null>>([null]);
  const query = useQuery(planningCandidatesQueryOptions(project, stream, cursors.at(-1) ?? null, revoked));
  const data = query.data?.result.data;
  const refresh = () => cursors.length > 1 ? setCursors([null]) : void query.refetch();
  if (revoked || isAuthError(query.error)) return <ObservationForbidden />;

  return <Card title={t('plan.changes.title')} data-testid="plan-changes" size="small">
    <Flex vertical gap={12}>
      {query.isPending ? <Spin /> : null}
      {query.error ? <Alert type="warning" showIcon message={t('plan.changes.failed')} description={t('plan.observation.recover')} /> : null}
      {!query.error && data?.items.length === 0 ? <Empty description={t('plan.changes.empty')} /> : null}
      {data ? <>
        {query.error ? <Typography.Text type="secondary">{t('plan.observation.lastConfirmed')}</Typography.Text> : null}
        <List dataSource={data.items} rowKey="candidate_id" renderItem={(item) => <List.Item>
          <Flex vertical gap={8} style={{ width: '100%' }}>
            <Flex justify="space-between" gap={8} wrap>
              <Typography.Text strong>{t('plan.changes.revision', { count: item.draft_revision })}</Typography.Text>
              <Typography.Text>{t(`plan.changes.status.${proposalStatus(item)}`)}</Typography.Text>
            </Flex>
            <Typography.Text type="secondary">{readableSentence(item.author.display_name ?? t('plan.changes.authorUnknown'), i18n.language)} · {formatUnixMs(item.updated_at_unix_ms, i18n.language)}</Typography.Text>
            {!item.baseline.matches_current && !item.activation.value ? <Alert type="info" showIcon message={t('plan.changes.planChanged')} /> : null}
            <Typography.Text type="secondary">{t('plan.changes.summary', item.current_diff.counts)}</Typography.Text>
            {item.current_diff.items.map((change) => <div key={change.work_id}>
              <Typography.Text>{readableSentence(change.title, i18n.language)}</Typography.Text>
              {' · '}<Typography.Text type="secondary">{t(`plan.changes.kind.${change.kind === 'added' || change.kind === 'removed' || change.kind === 'changed' || change.kind === 'unchanged' ? change.kind : 'unknown'}`)}</Typography.Text>
              <ChangeDetails fields={change.fields ?? []} />
            </div>)}
          </Flex>
        </List.Item>} />
        <Typography.Text type="secondary">{t('plan.observation.updated', { time: formatUnixMs(data.as_of.at_unix_ms, i18n.language) })}</Typography.Text>
      </> : null}
      <PageButtons refresh={refresh}
        previous={cursors.length > 1 ? () => setCursors(cursors.slice(0, -1)) : undefined}
        next={!query.error && data?.next_cursor ? () => setCursors([...cursors, data.next_cursor]) : undefined} />
    </Flex>
  </Card>;
}

export function SourceReadiness({ project, stream, workId }: { project: string; stream: string | null; workId: string | null }) {
  const { t, i18n } = useTranslation();
  const { revoked } = useRevocation();
  const [cursors, setCursors] = useState<Array<string | null>>([null]);
  const query = useQuery(requiredSourcesQueryOptions(project, stream, workId, cursors.at(-1) ?? null, revoked));
  const data = query.data?.result.data;
  const refresh = () => cursors.length > 1 ? setCursors([null]) : void query.refetch();
  if (revoked || isAuthError(query.error)) return <ObservationForbidden />;

  return <Card title={t('plan.materials.title')} data-testid="plan-materials" size="small">
    <Flex vertical gap={12}>
      <Typography.Text type="secondary">{t(workId ? 'plan.materials.workScope' : 'plan.materials.projectScope')}</Typography.Text>
      {query.isPending ? <Spin /> : null}
      {query.error ? <Alert type="warning" showIcon message={t('plan.materials.failed')} description={t('plan.observation.recover')} /> : null}
      {data ? <>
        {query.error ? <Typography.Text type="secondary">{t('plan.observation.lastConfirmed')}</Typography.Text> : null}
        {data.collection.required_total === 0 ? <Empty description={t('plan.materials.empty')} /> : <>
          {!query.error ? <Alert type={data.collection.complete ? 'success' : 'warning'} showIcon
            message={t(data.collection.complete ? 'plan.materials.ready' : 'plan.materials.incomplete')} /> : null}
          <List dataSource={data.items} rowKey="source_position" renderItem={(source) => {
            const status = source.status === 'consistent' || source.status === 'changed' || source.status === 'missing' || source.status === 'forbidden' ? source.status : 'unverified';
            return <List.Item><Flex vertical gap={4}>
              <Typography.Text>{documentLabel(source.path, { language: i18n.language, position: source.source_position }).text}</Typography.Text>
              <Typography.Text type="secondary">{t(`plan.materials.status.${status}`)}</Typography.Text>
            </Flex></List.Item>;
          }} />
        </>}
        <Typography.Text type="secondary">{t('plan.observation.updated', { time: formatUnixMs(data.collection.checked_at_unix_ms, i18n.language) })}</Typography.Text>
        <Typography.Text type="secondary">{t('plan.materials.boundary')}</Typography.Text>
      </> : null}
      <PageButtons refresh={refresh}
        previous={cursors.length > 1 ? () => setCursors(cursors.slice(0, -1)) : undefined}
        next={!query.error && data?.next_cursor ? () => setCursors([...cursors, data.next_cursor]) : undefined} />
    </Flex>
  </Card>;
}
