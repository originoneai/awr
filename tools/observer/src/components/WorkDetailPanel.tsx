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
  Typography,
} from 'antd';
import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { LastReportText } from './LastReportText';
import { NotReportedYet, ObservationEmpty, ObservationForbidden, PartialFailure } from './states';
import { FieldLabel, ObservationValue } from './ObservationValue';
import { deriveWorkState, rawObservationText } from '../core/observation';
import { workStateTechnicalItems } from '../lib/work-state-technical';
import { RecentChanges } from './RecentChanges';
import { TechnicalDetails } from './TechnicalDetails';
import { pageTechnicalItems } from '../lib/activity-technical';
import { isAuthError } from '../lib/api';
import { activityQueryOptions, reviewQueryOptions, workSnapshotQueryOptions } from '../lib/queries';
import { useRevocation } from '../lib/revocation';
import type {
  ReviewData,
  TeamActivityResponse,
  TeamReadResponse,
  TeamWork,
  WorkContractValue,
  WorkSnapshotData,
} from '../lib/types';

type SnapshotQuery = UseQueryResult<TeamReadResponse<WorkSnapshotData>, unknown>;
type ReviewQuery = UseQueryResult<TeamReadResponse<ReviewData>, unknown>;
type ActivityQuery = UseQueryResult<TeamActivityResponse, unknown>;

function Measured({ value, testId, field }: { value: unknown; testId?: string; field?: string }) {
  return <ObservationValue value={value} testId={testId} field={field} />;
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
          <Typography.Text>{rawObservationText(value).text}</Typography.Text>
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
  const state = deriveWorkState({ language: i18n.language, readState: 'ready', snapshot: data });
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
            children: <Measured testId="plan-runtime-state" value={runtimeState} field="runtime_state" />,
          },
          {
            key: 'guidance',
            label: t('plan.detail.guidance'),
            children: <Typography.Text data-testid="plan-guidance">{state.next.text}</Typography.Text>,
          },
          {
            key: 'report',
            label: t('plan.detail.lastReport'),
            children: <LastReportText report={state.lastReport} testId={`last-report-${data?.work_id ?? ''}`} />,
          },
        ]}
      />
      <TechnicalDetails items={workStateTechnicalItems(state)} />
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
                <Flex vertical style={{ minWidth: 0 }}>
                  <Typography.Text code>{spec.path ?? t('common.notProvided')}</Typography.Text>
                  <Typography.Text data-testid="required-spec-text" ellipsis={{ tooltip: spec.text }}>
                    {spec.text ?? t('common.notProvided')}
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
          ? <pre className="workbench-progress">{rawObservationText(data.observation.progress).text}</pre>
          : data?.observation ? <NotReportedYet /> : <Measured value={null} />}
      </section>
    </Flex>
  );
}

function ReviewOutcomes({ query, roundId }: { query: ReviewQuery; roundId: string | null }) {
  const { t } = useTranslation();
  if (!roundId) {
    return <Empty data-testid="review-round-unavailable" description={t('plan.detail.reviewRoundUnavailable')} />;
  }
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
              <ObservationValue testId="review-decision-code" value={decision.decision} field="decision" />
              <ObservationValue value={decision.approval_basis} field="approval_basis" />
              <ObservationValue value={decision.independence_kind} field="independence_kind" />
              <Typography.Text type="secondary">
                <FieldLabel field="human_approval" />: <ObservationValue value={decision.human_approval} />
              </Typography.Text>
            </Space>
          </List.Item>
        )}
      />
      <Descriptions
        size="small"
        column={1}
        items={[
          { key: 'actor', label: <FieldLabel field="author_actor_id" />, children: <ObservationValue value={review.author_actor_id} /> },
          { key: 'person', label: <FieldLabel field="author_person_id" />, children: <ObservationValue value={review.author_person_id} /> },
          { key: 'artifact', label: <FieldLabel field="artifact_digest" />, children: <ObservationValue value={review.artifact_digest} /> },
          { key: 'contract', label: <FieldLabel field="contract_hash" />, children: <ObservationValue value={review.contract_hash} /> },
          { key: 'bundle', label: <FieldLabel field="bundle_hash" />, children: <ObservationValue value={review.bundle_hash} /> },
        ]}
      />
      <Typography.Paragraph type="secondary">
        {t('state.placeholder.title')} — {t('plan.detail.deliveryPending')}
      </Typography.Paragraph>
    </div>
  );
}

function History({ query, project, workId }: { query: ActivityQuery; project: string; workId: string | null }) {
  const { t } = useTranslation();
  const data = query.data?.data;
  const errorCode = (query.error as { code?: string } | null)?.code;
  const viewAllHref = `/activity?project=${encodeURIComponent(project)}${workId ? `&work=${encodeURIComponent(workId)}` : ''}`;
  let content: ReactNode = <Spin />;
  if (errorCode === 'Unsupported') {
    content = <Empty data-testid="activity-unavailable" description={t('projectOverview.activity.unavailable')} />;
  } else if (query.error) {
    content = <PartialFailure errorCode={errorCode}><div /></PartialFailure>;
  } else if (data) {
    content = data.items.length === 0
      ? <ObservationEmpty />
      : <RecentChanges items={data.items} maxSteps={10} viewAllHref={viewAllHref} testId="plan-recent-changes" extraTechnical={pageTechnicalItems(data)} />;
  }
  return (
    <div data-testid="plan-history">
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
              label: <FieldLabel field="source_snapshot_id" />,
              children: <Measured testId="source-snapshot-id" value={result?.source_snapshot_id} />,
            },
            {
              key: 'revision',
              label: <FieldLabel field="project_revision" />,
              children: <Measured testId="source-project-revision" value={result?.project_revision} field="project_revision" />,
            },
            {
              key: 'epoch',
              label: <FieldLabel field="coordinator_epoch" />,
              children: <Measured testId="source-coordinator-epoch" value={result?.coordinator_epoch} />,
            },
            { key: 'contract', label: <FieldLabel field="contract_hash" />, children: <Measured value={data?.contract_hash} /> },
            { key: 'definition', label: <FieldLabel field="definition_state" />, children: <Measured value={data?.definition_state} field="definition_state" /> },
            { key: 'authority', label: <FieldLabel field="authority_version" />, children: <Measured value={result?.authority_version} field="authority_version" /> },
            { key: 'selection', label: <FieldLabel field="selection_basis" />, children: <Measured value={result?.selection_basis} field="selection_basis" /> },
          ]}
        />
        <Typography.Text type="secondary">{t('plan.detail.takeoverNote')}</Typography.Text>
      </Flex>
    </Card>
  );
}

export function WorkDetailPanel({
  project,
  work,
  reviewRoundId,
}: {
  project: string;
  work?: TeamWork;
  reviewRoundId: string | null;
}) {
  const { t } = useTranslation();
  const { revoked } = useRevocation();
  const workId = work?.key ?? null;
  const snapshot = useQuery(workSnapshotQueryOptions(
    work ? project : null,
    workId ?? '',
    work?.workstream_id ?? '',
    revoked,
  ));
  const review = useQuery(reviewQueryOptions(work ? project : null, workId, reviewRoundId, revoked));
  const activity = useQuery(activityQueryOptions(work ? project : null, revoked, workId ?? undefined));

  // Only the work-level read decides the whole panel's access: a Forbidden
  // from a review round of another work (stale/mistyped deep link) or from
  // the activity log must not blank requirements and source state. Those
  // failures stay inside their own tabs as partial failures with the code.
  if (revoked || isAuthError(snapshot.error)) {
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
              <Tag><ObservationValue value={work.status} field="state" /></Tag>
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
                children: <ReviewOutcomes query={review} roundId={reviewRoundId} />,
              },
              {
                key: 'history',
                label: t('plan.detail.history'),
                children: <History query={activity} project={project} workId={workId} />,
              },
            ]}
          />
        </Flex>
      </Card>
      <SourceState query={snapshot} />
    </Flex>
  );
}
