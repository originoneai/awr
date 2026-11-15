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
  Typography,
} from 'antd';
import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { LastReportText } from './LastReportText';
import { WorkProgress } from './WorkProgress';
import { ProgressReport } from './ProgressReport';
import { NotReportedYet, ObservationEmpty, ObservationForbidden, PartialFailure } from './states';
import { FieldLabel, ObservationValue } from './ObservationValue';
import { deriveWorkState, describeProgressReport, documentLabel, workLabel } from '../core/observation';
import { RecentChanges } from './RecentChanges';
import { WorkResponsibility } from './WorkResponsibility';
import { DependencyObservations } from './DependencyObservations';
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
          <ObservationValue value={value} />
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
  const report = describeProgressReport(data?.observation?.progress, { language: i18n.language, finished: state.lastReport.finished });
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
            children: <Typography.Text data-testid="plan-guidance" data-reporter-text={state.next.kind === 'reported' ? '' : undefined}>{state.next.text}</Typography.Text>,
          },
          {
            key: 'report',
            label: t('plan.detail.lastReport'),
            children: <LastReportText report={state.lastReport} testId={`last-report-${data?.work_id ?? ''}`} />,
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
            renderItem={(spec, index) => (
              <List.Item>
                <Flex vertical style={{ minWidth: 0 }}>
                  <Typography.Text strong data-testid="required-spec-name">
                    {documentLabel(spec.path, { language: i18n.language, position: index + 1 }).text}
                  </Typography.Text>
                  <Typography.Text type="secondary" data-testid="required-spec-text">
                    {t('plan.detail.materialReference')}
                  </Typography.Text>
                </Flex>
              </List.Item>
            )}
          />
        ) : <Measured value={null} />}
      </section>
      <section>
        <Typography.Title level={5}>{t('workbench.detail.currentOutcome')}</Typography.Title>
        {report.kind === 'reported'
          ? <ProgressReport report={report} />
          : data?.observation ? <NotReportedYet /> : <Measured value={null} />}
      </section>
    </Flex>
  );
}

function ReviewOutcomes({ query, roundId, currentRoundId }: { query: ReviewQuery; roundId: string | null; currentRoundId: string | null }) {
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
      {currentRoundId && roundId !== currentRoundId ? (
        <Typography.Paragraph type="secondary" data-testid="plan-historical-review">
          {t('plan.detail.historicalReviewSelected')}
        </Typography.Paragraph>
      ) : null}
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
      : <RecentChanges items={data.items} maxSteps={10} viewAllHref={viewAllHref} testId="plan-recent-changes" />;
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

/** The required project material and its current availability. */
function SourceState({ query }: { query: SnapshotQuery }) {
  const { t, i18n } = useTranslation();
  if (query.isPending) return <Spin />;
  if (query.isError) return null;
  const data = query.data?.result.data;
  return (
    <div data-testid="source-state-card">
      <Alert
        type={data?.context_complete === true ? 'success' : 'info'}
        showIcon
        message={t(data?.context_complete === true ? 'plan.detail.materialReady' : 'plan.detail.materialIncomplete')}
      />
      <List
        size="small"
        dataSource={data?.required_specs ?? []}
        renderItem={(spec, index) => <List.Item>{documentLabel(spec.path, { language: i18n.language, position: index + 1 }).text}</List.Item>}
      />
    </div>
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
  const { t, i18n } = useTranslation();
  const { revoked } = useRevocation();
  const workId = work?.key ?? null;
  const snapshot = useQuery(workSnapshotQueryOptions(
    work ? project : null,
    workId ?? '',
    work?.workstream_id ?? '',
    revoked,
  ));
  const currentRoundId = snapshot.isSuccess && snapshot.data.result.data.work_id === workId
    ? snapshot.data.result.data.observation?.collaboration?.review?.id ?? null
    : null;
  const selectedRoundId = reviewRoundId ?? currentRoundId;
  const review = useQuery(reviewQueryOptions(work ? project : null, workId, selectedRoundId, revoked));
  const activity = useQuery(activityQueryOptions(work ? project : null, revoked, workId ?? undefined));

  // Only the work-level read decides the whole panel's access: a Forbidden
  // from a review round of another work (stale/mistyped deep link) or from
  // the activity log must not blank requirements and source state. Those
  // failures stay inside their own tabs as product-language partial failures.
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
            <Typography.Title level={3} style={{ marginTop: 0 }}>{workLabel(work.title, { language: i18n.language }).text}</Typography.Title>
            <Space wrap>
              <WorkProgress work={work} query={snapshot} testId="plan-detail-status" />
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
                key: 'responsibility',
                label: t('collaboration.people'),
                children: snapshot.isPending ? <Spin /> : snapshot.isError || !snapshot.data
                  ? <PartialFailure><div /></PartialFailure>
                  : <WorkResponsibility key={work.key} project={project} workId={work.key} snapshot={snapshot.data.result.data} />,
              },
              {
                key: 'dependencies',
                label: t('collaboration.list'),
                children: <DependencyObservations key={work.key} project={project} workId={work.key} />,
              },
              {
                key: 'outcomes',
                label: t('plan.detail.outcomes'),
                children: <ReviewOutcomes query={review} roundId={selectedRoundId} currentRoundId={currentRoundId} />,
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
