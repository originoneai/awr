import { useQuery, type UseQueryResult } from '@tanstack/react-query';
import {
  Alert,
  Card,
  Descriptions,
  Empty,
  Flex,
  List,
  Space,
  Spin,
  Tabs,
  Tag,
  Tooltip,
  Typography,
} from 'antd';
import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { NotReportedYet, ObservationEmpty, ObservationForbidden, PartialFailure } from './states';
import { isAuthError } from '../lib/api';
import { formatUnixMs } from '../lib/format';
import { activityQueryOptions, reviewQueryOptions, workSnapshotQueryOptions } from '../lib/queries';
import { useRevocation } from '../lib/revocation';
import type {
  ReviewData,
  TeamActivityItem,
  TeamActivityResponse,
  TeamReadResponse,
  TeamWork,
  WorkContractValue,
  WorkSnapshotData,
} from '../lib/types';

type SnapshotQuery = UseQueryResult<TeamReadResponse<WorkSnapshotData>, unknown>;
type ReviewQuery = UseQueryResult<TeamReadResponse<ReviewData>, unknown>;
type ActivityQuery = UseQueryResult<TeamActivityResponse, unknown>;

function rawText(value: unknown): string | null {
  if (value == null) return null;
  if (typeof value === 'string') return value;
  if (typeof value === 'number' || typeof value === 'boolean') return String(value);
  return JSON.stringify(value) ?? null;
}

function Measured({ value, testId }: { value: unknown; testId?: string }) {
  const { t } = useTranslation();
  return <span data-testid={testId}>{rawText(value) ?? t('freshness.state.not_measured')}</span>;
}

function ContractValues({ values }: { values?: WorkContractValue[] }) {
  const { t } = useTranslation();
  if (!values) return <Measured value={null} />;
  if (values.length === 0) {
    return <Typography.Text type="secondary">{t('workbench.detail.none')}</Typography.Text>;
  }
  return (
    <List
      size="small"
      dataSource={values}
      renderItem={(value) => (
        <List.Item>
          <Typography.Text>{rawText(value)}</Typography.Text>
        </List.Item>
      )}
    />
  );
}

function Requirements({ query }: { query: SnapshotQuery }) {
  const { t, i18n } = useTranslation();
  const data = query.data?.result.data;
  if (query.isPending) return <Spin />;
  if (query.isError) {
    return <PartialFailure errorCode={(query.error as { code?: string }).code}><div /></PartialFailure>;
  }
  const contract = data?.visible_contract ?? data?.published_contract;
  const guidance = data?.guidance?.action;
  const reportedAt = data?.observation?.reporting?.client_reported_at_unix_ms;
  const runtimeState = data?.runtime?.state ?? data?.observation?.runtime?.state;
  return (
    <Flex vertical gap={16}>
      <Descriptions
        size="small"
        column={1}
        items={[
          {
            key: 'runtime',
            label: t('plan.detail.runtimeState'),
            children: <Measured testId="plan-runtime-state" value={runtimeState} />,
          },
          {
            key: 'guidance',
            label: t('plan.detail.guidance'),
            children: (
              <Tooltip title={guidance?.note ?? undefined}>
                <Typography.Text data-testid="plan-guidance">
                  {guidance?.op ?? t('freshness.state.not_measured')}
                </Typography.Text>
              </Tooltip>
            ),
          },
          {
            key: 'report',
            label: t('plan.detail.lastReport'),
            children: (
              <span data-testid={`last-report-${data?.work_id ?? ''}`}>
                {reportedAt == null
                  ? t('freshness.state.not_measured')
                  : formatUnixMs(reportedAt, i18n.language)}
              </span>
            ),
          },
        ]}
      />
      <section>
        <Typography.Title level={5}>{t('workbench.detail.goalsAcceptance')}</Typography.Title>
        <Typography.Text strong>{t('workbench.detail.goals')}</Typography.Text>
        <ContractValues values={contract?.goals} />
        <Typography.Text strong>{t('workbench.detail.acceptance')}</Typography.Text>
        <ContractValues values={contract?.acceptance} />
      </section>
      <section>
        <Typography.Title level={5}>{t('plan.detail.requiredSpecs')}</Typography.Title>
        {data?.required_specs ? (
          <List
            size="small"
            dataSource={data.required_specs}
            renderItem={(spec) => (
              <List.Item>
                <Flex vertical>
                  <Typography.Text code>{spec.path ?? t('freshness.state.not_measured')}</Typography.Text>
                  <Typography.Text ellipsis={{ tooltip: spec.text }}>
                    {spec.text ?? t('freshness.state.not_measured')}
                  </Typography.Text>
                </Flex>
              </List.Item>
            )}
          />
        ) : <Measured value={null} />}
      </section>
      <section>
        <Typography.Title level={5}>{t('workbench.detail.currentOutcome')}</Typography.Title>
        {data?.observation?.progress
          ? <pre className="workbench-progress">{JSON.stringify(data.observation.progress, null, 2)}</pre>
          : data?.observation ? <NotReportedYet /> : <Measured value={null} />}
      </section>
    </Flex>
  );
}

function ReviewOutcomes({ query }: { query: ReviewQuery }) {
  const { t } = useTranslation();
  if (query.isPending) return <Spin />;
  if (query.isError) {
    return <PartialFailure errorCode={(query.error as { code?: string }).code}><div /></PartialFailure>;
  }
  const review = query.data?.result.data.review;
  if (!review) {
    return <Empty data-testid="review-absent" description={t('plan.detail.reviewAbsent')} />;
  }
  return (
    <div data-testid="plan-review">
      <List
        size="small"
        dataSource={review.decisions}
        rowKey="decision_id"
        renderItem={(decision) => (
          <List.Item>
            <Space wrap data-testid={`review-decision-${decision.decision_id}`}>
              <Typography.Text code data-testid="review-decision-code">{decision.decision}</Typography.Text>
              <Typography.Text>{decision.approval_basis}</Typography.Text>
              <Typography.Text>{decision.independence_kind}</Typography.Text>
              <Typography.Text type="secondary">
                human_approval: {String(decision.human_approval)}
              </Typography.Text>
            </Space>
          </List.Item>
        )}
      />
      <Descriptions
        size="small"
        column={1}
        items={[
          { key: 'actor', label: 'author_actor_id', children: review.author_actor_id },
          { key: 'person', label: 'author_person_id', children: review.author_person_id },
          { key: 'artifact', label: 'artifact_digest', children: review.artifact_digest },
          { key: 'contract', label: 'contract_hash', children: review.contract_hash },
          { key: 'bundle', label: 'bundle_hash', children: review.bundle_hash },
        ]}
      />
      <Typography.Paragraph type="secondary">
        {t('state.placeholder.title')} — {t('plan.detail.deliveryPending')}
      </Typography.Paragraph>
    </div>
  );
}

function ActivityItems({ items }: { items: TeamActivityItem[] }) {
  const { t, i18n } = useTranslation();
  const latest = [...items]
    .sort((left, right) => right.created_at_unix_ms - left.created_at_unix_ms)
    .slice(0, 20);
  return (
    <List
      size="small"
      dataSource={latest}
      rowKey="id"
      renderItem={(item) => (
        <List.Item>
          <div data-testid={`activity-item-${item.id}`}>
            <Space wrap size={[12, 4]}>
              <Typography.Text code data-testid="activity-action">{item.action}</Typography.Text>
              <Typography.Text data-testid="activity-actor">{item.actor_id}</Typography.Text>
              <Typography.Text data-testid="activity-result">
                {rawText(item.result) ?? t('freshness.state.not_measured')}
              </Typography.Text>
              <Typography.Text type="secondary" data-testid="activity-time">
                {formatUnixMs(item.created_at_unix_ms, i18n.language)}
              </Typography.Text>
            </Space>
          </div>
        </List.Item>
      )}
    />
  );
}

function History({ query }: { query: ActivityQuery }) {
  const { t } = useTranslation();
  const data = query.data?.data;
  const errorCode = (query.error as { code?: string } | null)?.code;
  let content: ReactNode = <Spin />;
  if (errorCode === 'Unsupported') {
    content = <Empty data-testid="activity-unavailable" description={t('projectOverview.activity.unavailable')} />;
  } else if (query.error) {
    content = <PartialFailure errorCode={errorCode}><div /></PartialFailure>;
  } else if (data) {
    content = data.items.length === 0 ? <ObservationEmpty /> : <ActivityItems items={data.items} />;
  }
  return (
    <div data-testid="plan-history">
      {data ? (
        <Typography.Paragraph type="secondary">
          development_source: {rawText(data.development_source) ?? t('freshness.state.not_measured')}
          <br />
          unknown_means: {rawText(data.unknown_means) ?? t('freshness.state.not_measured')}
        </Typography.Paragraph>
      ) : null}
      {content}
      {data?.next_cursor != null
        ? <Alert type="info" showIcon message={t('projectOverview.activity.earlier')} />
        : null}
    </div>
  );
}

function SourceState({ query }: { query: SnapshotQuery }) {
  const { t } = useTranslation();
  const result = query.data?.result;
  const data = result?.data;
  return (
    <Card size="small" data-testid="source-state-card">
      <Flex vertical gap={12}>
        <Typography.Title level={5} style={{ margin: 0 }}>{t('plan.detail.sourceState')}</Typography.Title>
        <Descriptions
          size="small"
          column={1}
          items={[
            {
              key: 'snapshot',
              label: 'source_snapshot_id',
              children: <Measured testId="source-snapshot-id" value={result?.source_snapshot_id} />,
            },
            {
              key: 'revision',
              label: 'project_revision',
              children: <Measured testId="source-project-revision" value={result?.project_revision} />,
            },
            {
              key: 'epoch',
              label: 'coordinator_epoch',
              children: <Measured testId="source-coordinator-epoch" value={result?.coordinator_epoch} />,
            },
            { key: 'contract', label: 'contract_hash', children: <Measured value={data?.contract_hash} /> },
            { key: 'definition', label: 'definition_state', children: <Measured value={data?.definition_state} /> },
            { key: 'authority', label: 'authority_version', children: <Measured value={result?.authority_version} /> },
            { key: 'selection', label: 'selection_basis', children: <Measured value={result?.selection_basis} /> },
          ]}
        />
        <Typography.Text type="secondary">{t('plan.detail.takeoverNote')}</Typography.Text>
      </Flex>
    </Card>
  );
}

export function WorkDetailPanel({ project, work }: { project: string; work?: TeamWork }) {
  const { t } = useTranslation();
  const { revoked } = useRevocation();
  const workId = work?.key ?? null;
  const snapshot = useQuery(workSnapshotQueryOptions(
    work ? project : null,
    workId ?? '',
    work?.workstream_id ?? '',
    revoked,
  ));
  const review = useQuery(reviewQueryOptions(work ? project : null, workId, revoked));
  const activity = useQuery(activityQueryOptions(work ? project : null, revoked, workId ?? undefined));

  if (revoked || isAuthError(snapshot.error) || isAuthError(review.error) || isAuthError(activity.error)) {
    return <ObservationForbidden />;
  }
  if (!work) {
    return (
      <Card data-testid="plan-detail">
        <Typography.Text type="secondary">{t('plan.selectWork')}</Typography.Text>
      </Card>
    );
  }
  return (
    <Flex vertical gap={16} style={{ minWidth: 0 }}>
      <Card data-testid="plan-detail">
        <Flex vertical gap={12}>
          <div>
            <Typography.Title level={3} style={{ marginTop: 0 }}>{work.title}</Typography.Title>
            <Space wrap>
              <Tag>{work.status === 'unknown' || work.status == null
                ? t('freshness.state.not_measured')
                : work.status}</Tag>
              <Typography.Text type="secondary">{work.key}</Typography.Text>
            </Space>
          </div>
          <Tabs
            defaultActiveKey="requirements"
            items={[
              {
                key: 'requirements',
                label: t('plan.detail.requirements'),
                children: <Requirements query={snapshot} />,
              },
              {
                key: 'outcomes',
                label: t('plan.detail.outcomes'),
                children: <ReviewOutcomes query={review} />,
              },
              {
                key: 'history',
                label: t('plan.detail.history'),
                children: <History query={activity} />,
              },
            ]}
          />
        </Flex>
      </Card>
      <SourceState query={snapshot} />
    </Flex>
  );
}
