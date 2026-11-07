import { useQueries, useQuery } from '@tanstack/react-query';
import { Card, Col, List, Pagination, Row, Segmented, Space, Table, Tag, Typography, theme } from 'antd';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useSearchParams } from 'react-router';
import { ArtifactReader } from '../components/ArtifactReader';
import { DeliverySteps, type DeliveryStep } from '../components/DeliverySteps';
import { FreshnessBadge } from '../components/FreshnessBadge';
import { RecordedTime } from '../components/RecordedTime';
import { ObservationEmpty, ObservationForbidden, PartialFailure } from '../components/states';
import { TechnicalDetails } from '../components/TechnicalDetails';
import { checkLabel } from '../core/observation';
import { ApiError, isAuthError } from '../lib/api';
import { formatObservationValue, notProvided } from '../i18n/craft';
import { craftSelectionStyle } from '../theme';
import { formatUnixMs } from '../lib/format';
import { asArray, asObject, asText, at } from '../lib/json';
import { outcomeQueryOptions } from '../lib/outcome';
import { overviewQueryOptions, workSnapshotQueryOptions } from '../lib/queries';
import { useRevocation } from '../lib/revocation';

const PAGE_SIZE = 60;

function validPage(value: string | null) {
  if (!value || !/^[1-9]\d*$/.test(value)) return 1;
  const page = Number(value);
  return Number.isSafeInteger(page) ? page : 1;
}

function errorCode(error: unknown) {
  return error instanceof ApiError ? error.code : 'UnknownError';
}

/** Prototype's master/detail outcome view; exact evidence stays in disclosure groups. */
export function ProjectAcceptance({ project }: { project: string }) {
  const { t, i18n } = useTranslation();
  const { token } = theme.useToken();
  const { revoked } = useRevocation();
  const [searchParams, setSearchParams] = useSearchParams();
  const [versionFilter, setVersionFilter] = useState<'all' | 'current' | 'history'>('current');
  const overview = useQuery(overviewQueryOptions(project, revoked));
  const works = overview.data?.works ?? [];
  const requestedWork = searchParams.get('work');
  const requestedIndex = requestedWork ? works.findIndex((work) => work.key === requestedWork) : -1;
  const lastPage = Math.max(1, Math.ceil(works.length / PAGE_SIZE));
  const requestedPage = validPage(searchParams.get('page'));
  const page = requestedIndex >= 0 ? Math.floor(requestedIndex / PAGE_SIZE) + 1 : Math.min(requestedPage, lastPage);
  const visibleWorks = works.slice((page - 1) * PAGE_SIZE, page * PAGE_SIZE);
  const snapshots = useQueries({
    queries: visibleWorks.map((work) => (
      workSnapshotQueryOptions(project, work.key, work.workstream_id, revoked)
    )),
  });
  const selectedWork = visibleWorks.find((work) => work.key === requestedWork) ?? visibleWorks[0];
  const selectedWorkIndex = selectedWork ? visibleWorks.indexOf(selectedWork) : -1;
  const selectedSnapshot = selectedWorkIndex >= 0 ? snapshots[selectedWorkIndex] : undefined;
  const snapshotData = selectedSnapshot?.data?.result.data;
  const collaboration = snapshotData?.observation?.collaboration;
  const currentRound = collaboration?.review?.id;
  const selectedRound = searchParams.get('round') ?? currentRound;
  const workId = selectedWork?.key ?? '';
  const enabled = Boolean(selectedWork);
  const review = useQuery(outcomeQueryOptions(project, 'review.inspect', {
    work_id: workId, review_round_id: selectedRound ?? '',
  }, revoked, enabled && Boolean(selectedRound)));
  const reviewObject = asObject(at(review.data?.result.data, ['review']));
  const evidenceId = asText(reviewObject?.evidence_id);
  const executionId = asText(reviewObject?.execution_id);
  const evidence = useQuery(outcomeQueryOptions(project, 'evidence.inspect', {
    work_id: workId, evidence_id: evidenceId ?? '',
  }, revoked, enabled && Boolean(evidenceId)));
  const execution = useQuery(outcomeQueryOptions(project, 'execution.inspect', {
    work_id: workId, execution_id: executionId ?? '',
  }, revoked, enabled && Boolean(executionId)));
  const claimId = asText(execution.data?.result.data.claim_id);
  const claim = useQuery(outcomeQueryOptions(project, 'claim.inspect', {
    work_id: workId, claim_id: claimId ?? '',
  }, revoked, enabled && Boolean(claimId)));
  const selectedCompletionId = snapshotData?.observation?.runtime?.selected_completion_id;
  const completion = useQuery(outcomeQueryOptions(project, 'completion.inspect', {
    work_id: workId,
  }, revoked, enabled && Boolean(selectedCompletionId)));
  const delivery = useQuery(outcomeQueryOptions(project, 'delivery.inspect', { work_id: workId }, revoked, enabled));
  const neutral = useQuery(outcomeQueryOptions(project, 'delivery.neutral.inspect', { work_id: workId }, revoked, enabled));
  const submission = useQuery(outcomeQueryOptions(project, 'delivery.submission.describe', { work_id: workId }, revoked, enabled));
  const integrationId = collaboration?.integration?.id;
  const integration = useQuery(outcomeQueryOptions(project, 'delivery.integration.inspect', {
    work_id: workId, request_id: integrationId ?? '',
  }, revoked, enabled && Boolean(integrationId)));
  const source = useQuery(outcomeQueryOptions(project, 'delivery.source.status', {
    work_id: workId,
  }, revoked, enabled && Boolean(collaboration?.source_state)));
  const exports = useQuery(outcomeQueryOptions(project, 'delivery.exports', {
    work_id: workId, limit: 20,
  }, revoked, enabled));

  const missing = notProvided(i18n.language);
  const value = (input: unknown, field?: string) => formatObservationValue(input, i18n.language, field);
  const readError = (query: { error: unknown }, testId: string) => (
    <Typography.Text type="warning" data-testid={testId}>
      {t('acceptance.readFailed')}
      <TechnicalDetails items={[{ label: 'Error', value: errorCode(query.error) }]} />
    </Typography.Text>
  );
  const loading = <Typography.Text type="secondary">{t('common.loading')}</Typography.Text>;
  const empty = <Typography.Text type="secondary">{t('acceptance.notObserved')}</Typography.Text>;

  if (revoked || isAuthError(overview.error)) return <ObservationForbidden />;
  if (overview.isPending) return <Card>{loading}</Card>;
  if (overview.isError) return <PartialFailure errorCode={errorCode(overview.error)}>{null}</PartialFailure>;
  if (works.length === 0) return <ObservationEmpty />;

  const reviewCandidateCurrent = reviewObject?.candidate_current ?? at(reviewObject, ['submission', 'candidate_current']);
  const reviewIsCurrent = reviewCandidateCurrent === true
    && at(reviewObject, ['evidence_provenance', 'review_contract_matches_current']) !== false;
  const completionData = completion.data?.result.data;
  const completionObject = asObject(completionData?.completion);
  const completionReceipt = asText(completionObject?.receipt_id);
  const completionSelected = asText(completionData?.selected_completion_id);
  const completionReviewRound = asText(at(completionObject, ['approved_by', 'review_round_id']));
  const completionReviewDecision = asText(at(completionObject, ['approved_by', 'review_decision_id']));
  const completionIndependence = asText(at(completionObject, ['approved_by', 'independence_kind']));
  const deliveryObject = asObject(at(delivery.data?.result.data, ['delivery']));
  const deliverySelected = asText(at(deliveryObject, ['awr_acceptance', 'selected_completion_id']));
  const completionConflict = Boolean(completionData && (
    completionReceipt !== completionSelected
    || completionSelected !== selectedCompletionId
    || (deliverySelected != null && deliverySelected !== completionSelected)
    || (completionObject?.evidence_id != null && completionObject.evidence_id !== evidenceId)
    || (completionObject?.execution_id != null && completionObject.execution_id !== executionId)
    || (completionObject?.contract_hash != null && completionObject.contract_hash !== snapshotData?.contract_hash)
    || (completionReviewRound != null && completionReviewRound !== selectedRound)
  ));
  const integrationData = integration.data?.result.data;
  const observedAt = at(integrationData, ['confirmation', 'envelope', 'record', 'data', 'provenance', 'observed_at_unix_ms']);
  const recordedAt = at(integrationData, ['confirmation', 'envelope', 'record', 'data', 'provenance', 'recorded_at_unix_ms']);
  const artifacts = asArray(at(reviewObject, ['submission', 'artifacts']));
  const connectors = asArray(at(neutral.data?.result.data, ['connectors']));
  const exportData = exports.data?.result.data;
  const exportItems = asArray(exportData?.items);
  const verification = asArray(collaboration?.verification);
  const reviewDecisions = asArray(reviewObject?.decisions);
  const reviewFindings = asArray(at(reviewObject, ['findings', 'findings']));
  const requiredDependencies = snapshotData?.visible_contract?.required_dependencies
    ?? snapshotData?.published_contract?.required_dependencies
    ?? asArray(at(snapshotData, ['adopted_dependencies']))
    ?? [];

  function selectWork(key: string) {
    const next = new URLSearchParams(searchParams);
    next.set('tab', 'acceptance');
    next.set('work', key);
    next.delete('round');
    const index = works.findIndex((work) => work.key === key);
    const targetPage = Math.floor(index / PAGE_SIZE) + 1;
    if (targetPage > 1) next.set('page', String(targetPage));
    else next.delete('page');
    setSearchParams(next);
  }

  function snapshotForWork(key: string) {
    return snapshots.find((query) => query.data?.result.data.work_id === key)?.data?.result.data;
  }

  function versionOf(data: typeof snapshotData) {
    const selection = data?.observation?.collaboration?.candidate?.selection_version;
    return selection == null ? null : `v${selection}`;
  }

  function isCurrentVersion(key: string) {
    return snapshotForWork(key)?.observation?.collaboration?.candidate?.current === true;
  }

  const filteredWorks = visibleWorks.filter((work) => (
    versionFilter === 'all'
    || (versionFilter === 'current' && isCurrentVersion(work.key))
    || (versionFilter === 'history' && !isCurrentVersion(work.key))
  ));
  const candidatePresent = collaboration?.candidate != null || collaboration?.review != null || reviewObject != null;
  const reviewState = asText(reviewObject?.state ?? collaboration?.review?.state);
  const reviewApproved = reviewState === 'approved' && reviewIsCurrent;
  const reviewIsHistorical = reviewCandidateCurrent === false
    || at(reviewObject, ['evidence_provenance', 'review_contract_matches_current']) === false;
  const integrationState = asText(integrationData?.state ?? collaboration?.integration?.state);
  const integrationOutcome = asText(at(integrationData, ['confirmation', 'envelope', 'record', 'data', 'outcome']));
  const integrationConfirmed = integrationState === 'confirmed' && integrationOutcome === 'applied';
  const integrationRejected = integrationState === 'rejected' || integrationOutcome === 'rejected';
  const deliveryComplete = at(deliveryObject, ['awr_acceptance', 'complete']) === true
    && at(deliveryObject, ['awr_acceptance', 'runtime_state']) === 'completed';
  const completionKnown = completionSelected != null && completionReceipt != null;
  const snapshotLoading = Boolean(selectedWork) && (selectedSnapshot == null || selectedSnapshot.isPending);
  const snapshotFailed = selectedSnapshot?.isError === true;
  const reviewLoading = Boolean(selectedRound) && review.isPending;
  const integrationLoading = Boolean(integrationId) && integration.isPending;
  const completionLoading = Boolean(selectedCompletionId) && completion.isPending;
  const acceptanceLoading = snapshotLoading || reviewLoading || integrationLoading || completionLoading || delivery.isPending;
  const acceptanceUnknown = snapshotFailed || review.isError || integration.isError || completion.isError || delivery.isError;
  const steps: DeliveryStep[] = [
    {
      key: 'candidate', title: t('acceptance.stepCandidate'),
      text: snapshotLoading ? t('common.loading') : snapshotFailed ? t('acceptance.statusUnknown') : candidatePresent ? t('acceptance.statusRecorded') : t('acceptance.statusNotGenerated'),
      tone: snapshotFailed ? 'warning' : snapshotLoading ? 'neutral' : candidatePresent ? 'positive' : 'neutral',
    },
    {
      key: 'verification', title: t('acceptance.stepVerification'),
      text: snapshotLoading
        ? t('common.loading')
        : snapshotFailed
          ? t('acceptance.statusUnknown')
          : verification.length === 0
            ? t('acceptance.statusNoRecord')
            : verification.every((entry) => at(entry, ['outcome']) === 'passed')
              ? t('acceptance.statusPassRecord')
              : t('acceptance.statusCheckRecord'),
      tone: snapshotFailed ? 'warning' : snapshotLoading || verification.length === 0 ? 'neutral' : verification.every((entry) => at(entry, ['outcome']) === 'passed') ? 'positive' : 'warning',
    },
    {
      key: 'review', title: t('acceptance.stepReview'),
      text: snapshotLoading || reviewLoading
        ? t('common.loading')
        : snapshotFailed || review.isError
          ? t('acceptance.statusUnknown')
          : reviewApproved
            ? t('acceptance.statusApproved')
            : reviewState === 'approved' && reviewCandidateCurrent == null
              ? t('acceptance.statusUnknown')
              : reviewState === 'open'
                ? t('acceptance.statusReviewPending')
                : reviewState
                  ? t('acceptance.statusNotApproved')
                  : t('acceptance.statusNotStarted'),
      tone: snapshotFailed || review.isError ? 'warning' : snapshotLoading || reviewLoading ? 'neutral' : reviewApproved ? 'positive' : reviewState === 'open' ? 'warning' : reviewState ? 'negative' : 'neutral',
      testId: reviewApproved && !review.isError ? 'acceptance-current-approval' : reviewIsHistorical ? 'acceptance-review-history' : undefined,
    },
    {
      key: 'integration', title: t('acceptance.stepIntegration'),
      text: integrationConfirmed
        ? t('acceptance.statusIntegrated')
        : integrationRejected
          ? t('acceptance.statusIntegrationRejected')
          : snapshotLoading || integrationLoading
            ? t('common.loading')
            : snapshotFailed || integration.isError
              ? t('acceptance.statusUnknown')
              : integrationId
                ? t('acceptance.statusIntegrationPending')
                : t('acceptance.statusIntegrationNotPrepared'),
      tone: integrationConfirmed ? 'positive' : integrationRejected ? 'negative' : snapshotFailed || integration.isError ? 'warning' : snapshotLoading || integrationLoading ? 'neutral' : integrationId ? 'warning' : 'neutral',
      testId: integrationRejected ? 'acceptance-integration-rejected' : undefined,
    },
    {
      key: 'acceptance', title: t('acceptance.stepAcceptance'),
      text: completionConflict
        ? t('acceptance.statusConflict')
        : acceptanceLoading
          ? t('common.loading')
          : acceptanceUnknown
            ? t('acceptance.statusUnknown')
            : deliveryComplete && completionKnown && integrationConfirmed
              ? t('acceptance.statusCompleted')
              : deliveryComplete
                ? t('acceptance.statusCompletionUnconfirmed')
                : completionKnown
                  ? t('acceptance.statusIntegrationPending')
                  : t('acceptance.noCompletionReceipt'),
      tone: completionConflict ? 'negative' : acceptanceUnknown ? 'warning' : acceptanceLoading ? 'neutral' : deliveryComplete && completionKnown && integrationConfirmed ? 'positive' : deliveryComplete || completionKnown ? 'warning' : 'neutral',
      testId: completionConflict ? 'acceptance-completion-conflict' : undefined,
    },
  ];

  const currentVersion = versionOf(snapshotData);
  const currentVersionText = currentVersion ?? t('acceptance.currentVersion');
  const verificationRows = verification.map((entry, index) => ({
    key: index,
    check: checkLabel(at(entry, ['check']), { language: i18n.language, position: index + 1 }).text,
    outcome: at(entry, ['outcome']) === 'passed' ? t('acceptance.statusPassed') : at(entry, ['outcome']) === 'failed' ? t('acceptance.statusFailed') : t('acceptance.statusUnknown'),
    version: currentVersionText,
    raw: entry,
  }));

  return (
    <Space direction="vertical" size={16} style={{ width: '100%' }} data-testid="acceptance-page">
      <Card size="small">
        <Space size={16} wrap>
          <Typography.Text strong>{t('acceptance.title')}</Typography.Text>
          <Typography.Text type="secondary" data-testid="acceptance-from-works">
            {t('acceptance.fromWorks', { count: snapshots.filter((query) => query.data).length, total: works.length })}
          </Typography.Text>
          {selectedSnapshot?.data ? <FreshnessBadge freshness={selectedSnapshot.data.freshness} /> : null}
        </Space>
      </Card>
      <Row gutter={[16, 16]}>
        <Col xs={24} lg={7}>
          <Card size="small" title={t('acceptance.sources')}>
            <Segmented
              block
              size="small"
              style={{ marginBottom: token.marginSM }}
              value={versionFilter}
              onChange={(next) => setVersionFilter(next as typeof versionFilter)}
              options={[
                { label: t('acceptance.currentVersion'), value: 'current' },
                { label: t('acceptance.allVersions'), value: 'all' },
                { label: t('acceptance.historyVersions'), value: 'history' },
              ]}
            />
            {filteredWorks.length === 0 ? <ObservationEmpty /> : (
              <List
                size="small"
                dataSource={filteredWorks}
                rowKey={(work) => work.key}
                renderItem={(work) => {
                  const data = snapshotForWork(work.key);
                  const observation = data?.observation;
                  const state = observation?.collaboration?.review?.state;
                  const version = versionOf(data);
                  const author = asText(observation?.responsibility?.owner_name)
                    ?? asText(observation?.collaboration?.review?.author_actor_id)
                    ?? asText(observation?.responsibility?.owner)
                    ?? missing;
                  const reportedAt = observation?.last_activity_at_unix_ms
                    ?? at(observation, ['progress', 'reported_at_unix_ms'])
                    ?? observation?.reporting?.client_reported_at_unix_ms;
                  const label = state === 'approved'
                    ? t('acceptance.statusApproved')
                    : state === 'rejected'
                      ? t('acceptance.statusFailed')
                    : state === 'open'
                      ? t('acceptance.statusReviewPending')
                      : observation?.collaboration?.candidate || observation?.collaboration?.review
                        ? t('acceptance.statusRecorded')
                        : t('acceptance.statusNotGenerated');
                  return (
                    <List.Item data-testid={`acceptance-work-${work.key}`} onClick={() => selectWork(work.key)} style={craftSelectionStyle(token, selectedWork?.key === work.key)}>
                      <Space direction="vertical" size={2} style={{ width: '100%' }}>
                        <Space style={{ display: 'flex', justifyContent: 'space-between', width: '100%' }}>
                          <Typography.Text strong style={{ overflowWrap: 'anywhere' }}>{work.title}{version ? ` ${version}` : ''}</Typography.Text>
                          <Tag color={state === 'approved' ? 'success' : state === 'rejected' ? 'error' : state === 'open' ? 'warning' : 'default'}>{label}</Tag>
                        </Space>
                        <Typography.Text type="secondary">
                          {author}
                          {reportedAt != null ? ` · ${formatUnixMs(Number(reportedAt), i18n.language)}` : ` · ${missing}`}
                        </Typography.Text>
                      </Space>
                    </List.Item>
                  );
                }}
              />
            )}
            <Pagination
              current={page}
              total={works.length}
              pageSize={PAGE_SIZE}
              showSizeChanger={false}
              hideOnSinglePage={false}
              onChange={(nextPage) => {
                const next = new URLSearchParams(searchParams);
                if (nextPage > 1) next.set('page', String(nextPage));
                else next.delete('page');
                next.delete('work');
                next.delete('round');
                setSearchParams(next);
              }}
            />
          </Card>
        </Col>
        <Col xs={24} lg={17}>
          <Space direction="vertical" size={16} style={{ width: '100%' }}>
            <Card size="small" title={selectedWork?.title ?? t('acceptance.title')}>
              <DeliverySteps steps={steps} testId="acceptance-evidence-chain" />
            </Card>

            {verificationRows.length > 0 ? (
              <Card size="small" title={t('acceptance.verificationTitle')} data-testid="acceptance-verification">
                <Table
                  size="small"
                  rowKey="key"
                  pagination={false}
                  dataSource={verificationRows}
                  columns={[
                    { title: t('acceptance.verificationContent'), dataIndex: 'check' },
                    { title: t('acceptance.verificationResult'), dataIndex: 'outcome', render: (text) => <Tag color={text === t('acceptance.statusPassed') ? 'success' : text === t('acceptance.statusFailed') ? 'error' : 'default'}>{text}</Tag> },
                    { title: t('acceptance.boundVersion'), dataIndex: 'version' },
                  ]}
                />
                <Typography.Text type="secondary">{t('acceptance.verificationNote')}</Typography.Text>
              </Card>
            ) : null}

            <Card size="small" title={t('acceptance.reviewChanges')} data-testid={`acceptance-review-${selectedRound ?? 'none'}`}>
              <Space direction="vertical" size={8} style={{ width: '100%' }}>
                <Space style={{ display: 'flex', justifyContent: 'space-between', width: '100%' }}>
                  <Typography.Text>{t('acceptance.currentVersion')}</Typography.Text>
                  <Typography.Text>{currentVersionText} · {steps[2].text}</Typography.Text>
                </Space>
                {reviewCandidateCurrent == null ? (
                  <Typography.Text type="secondary" data-testid="acceptance-review-currentness">{missing}</Typography.Text>
                ) : null}
                {reviewIsHistorical ? <Tag color="warning">{t('acceptance.historical')}</Tag> : null}
                <Typography.Text type="secondary">{t('acceptance.reviewChangeNote')}</Typography.Text>
                {review.isError ? readError(review, 'acceptance-review-explanation-error')
                  : selectedRound && review.isPending ? loading
                    : reviewObject ? (
                      <>
                        {reviewDecisions.map((decision, index) => (
                          <div key={asText(at(decision, ['decision_id'])) ?? index} data-testid="review-explanation" style={{ width: '100%' }}>
                            <Typography.Text strong>{t('acceptance.reviewReason')}</Typography.Text>
                            <Typography.Paragraph ellipsis={{ rows: 4, expandable: true, symbol: t('acceptance.readMore') }} style={{ marginBottom: 0, whiteSpace: 'pre-wrap', overflowWrap: 'anywhere' }}>
                              {asText(at(decision, ['reason']))?.trim() || missing}
                            </Typography.Paragraph>
                          </div>
                        ))}
                        {reviewFindings.map((finding, index) => (
                          <Space key={asText(at(finding, ['finding_key'])) ?? index} direction="vertical" size={4} style={{ width: '100%', overflowWrap: 'anywhere' }} data-testid="review-finding">
                            <Space wrap>
                              <Typography.Text strong>{t('acceptance.reviewFinding')}</Typography.Text>
                              <Tag color={at(finding, ['severity']) === 'blocking' && at(finding, ['state']) === 'open' ? 'error' : 'default'}>{value(at(finding, ['severity']), 'severity')}</Tag>
                              <Tag>{at(finding, ['state']) === 'open' ? t('acceptance.findingOpen') : value(at(finding, ['state']), 'state')}</Tag>
                            </Space>
                            <Typography.Text>{asText(at(finding, ['summary']))?.trim() || missing}</Typography.Text>
                            {asText(at(finding, ['criterion'])) ? <Typography.Text type="secondary">{t('acceptance.findingCriterion')}: {asText(at(finding, ['criterion']))}</Typography.Text> : null}
                            {(['given', 'when', 'then'] as const).map((part) => (
                              asText(at(finding, ['counterexample', part])) ? (
                                <Typography.Text key={part} type="secondary">
                                  {t(`acceptance.counterexample.${part}`)}: {asText(at(finding, ['counterexample', part]))}
                                </Typography.Text>
                              ) : null
                            ))}
                          </Space>
                        ))}
                        {reviewDecisions.length === 0 && reviewFindings.length === 0 ? (
                          <Typography.Text type="secondary">{t('acceptance.noReviewExplanation')}</Typography.Text>
                        ) : null}
                      </>
                    ) : null}
              </Space>
            </Card>

            {requiredDependencies.length > 0 ? (
              <Card size="small" title={t('acceptance.downstream')}>
                <List
                  size="small"
                  dataSource={requiredDependencies}
                  renderItem={(dependency) => {
                    const key = asText(dependency);
                    const related = works.find((work) => work.key === key);
                    return (
                      <List.Item>
                        <Typography.Text>{related?.title ?? t('acceptance.relatedWork')}</Typography.Text>
                        <TechnicalDetails items={[{ label: 'Dependency', value: dependency }]} />
                      </List.Item>
                    );
                  }}
                />
              </Card>
            ) : null}

            <TechnicalDetails items={[
              { label: 'Outcome and version', testId: 'acceptance-version', value: { workId, snapshot: snapshotData } },
              { label: 'Review', testId: 'acceptance-review', value: reviewObject },
              ...(evidenceId ? [{ label: 'Evidence', testId: 'acceptance-evidence', value: evidence.data?.result.data }] : []),
              ...(executionId ? [{ label: 'Execution', testId: 'acceptance-execution', value: execution.data?.result.data }] : []),
              ...(claimId ? [{ label: 'Claim', testId: 'acceptance-claim', value: claim.data?.result.data }] : []),
              ...(selectedCompletionId && !completionConflict ? [{ label: 'Completion', testId: 'acceptance-completion', value: completionData }] : []),
              { label: 'Connectors and submission', testId: 'acceptance-connectors', value: { neutral: neutral.data?.result.data, submission: submission.data?.result.data, connectors } },
              ...(integrationId ? [{ label: 'Integration', testId: 'acceptance-integration', value: integrationData }] : []),
              ...(collaboration?.source_state ? [{ label: 'Source', testId: 'acceptance-source', value: source.data?.result.data }] : []),
              { label: 'Delivery', testId: 'acceptance-delivery', value: deliveryObject },
              { label: 'Exports', testId: 'acceptance-exports', value: { ...exportData, items: exportItems } },
              { label: 'Artifacts', testId: 'acceptance-artifacts', value: artifacts },
              {
                label: 'Record times', testId: 'acceptance-times',
                copyValue: JSON.stringify({ observedAt, recordedAt, pageRead: selectedSnapshot?.data?.freshness.page_read.at_unix_ms, sourceQuery: collaboration?.source_state?.query_started_at_unix_ms }),
                value: (
                  <>
                    <span>{t('acceptance.externalObserved')}: <RecordedTime value={observedAt} /></span>{' · '}
                    <span>{t('acceptance.awrRecorded')}: <RecordedTime value={recordedAt} /></span>{' · '}
                    <span>{t('acceptance.pageRead')}: <RecordedTime value={selectedSnapshot?.data?.freshness.page_read.at_unix_ms} /></span>{' · '}
                    <span>{t('acceptance.sourceQuery')}: <RecordedTime value={collaboration?.source_state?.query_started_at_unix_ms} /></span>
                  </>
                ),
              },
            ]}>
              {asArray(reviewObject?.decisions).map((decision, index) => (
                <span key={asText(at(decision, ['decision_id'])) ?? index} data-testid="decision-human-approval">
                  {value(at(decision, ['human_approval']))}
                </span>
              ))}
              {asArray(at(reviewObject, ['findings', 'findings'])).map((finding, index) => (
                <Typography.Text key={asText(at(finding, ['finding_key'])) ?? index} data-testid={`finding-original-${asText(at(finding, ['finding_key']))}`}>
                  {t('acceptance.originalWork', { work: workId, round: value(at(finding, ['round'])) })} · {value(at(finding, ['criterion']))}
                </Typography.Text>
              ))}
              {at(reviewObject, ['evidence_provenance', 'author_self_report']) === true ? <span>{t('acceptance.selfReport')}</span> : null}
              {at(reviewObject, ['evidence_provenance', 'execution_consistency', 'independent_verification']) === false ? <span>{t('acceptance.nonIndependent')}</span> : null}
              {selectedCompletionId && !completionConflict ? (
                <>
                  <span data-testid="completion-review-round">{completionReviewRound ?? missing}</span>
                  <span data-testid="completion-review-decision">{completionReviewDecision ?? missing}</span>
                  <span data-testid="completion-independence">{value(completionIndependence, 'independence_kind')}</span>
                </>
              ) : null}
              {integrationId ? (
                <>
                  <span data-testid="acceptance-integration-outcome">{value(at(integrationData, ['confirmation', 'envelope', 'record', 'data', 'outcome']), 'outcome')}</span>
                  <span data-testid="acceptance-integration-state">{value(integrationData?.state, 'state')}</span>
                  <span data-testid="acceptance-dispatch-state">{value(at(integrationData, ['dispatch_receipt', 'data', 'state']), 'dispatch_state')}</span>
                </>
              ) : null}
              <span data-testid="acceptance-delivery-complete">{completionConflict ? t('acceptance.conflict') : value(at(deliveryObject, ['awr_acceptance', 'complete']))}</span>
              {artifacts.map((artifact, index) => {
                const artifactId = asText(at(artifact, ['query', 'artifact_id']));
                return (
                  <div key={artifactId ?? index} data-testid={`acceptance-artifact-${artifactId}`}>
                    <ArtifactReader project={project} artifact={artifact} revoked={revoked} />
                  </div>
                );
              })}
            </TechnicalDetails>

            {review.isError ? readError(review, 'acceptance-review-error') : null}
            {evidence.isError ? readError(evidence, 'acceptance-evidence-error') : null}
            {execution.isError ? readError(execution, 'acceptance-execution-error') : null}
            {claim.isError ? readError(claim, 'acceptance-claim-error') : null}
            {completion.isError ? readError(completion, 'acceptance-completion-error') : null}
            {integration.isError ? readError(integration, 'acceptance-integration-error') : null}
            {source.isError ? readError(source, 'acceptance-source-error') : null}
            {delivery.isError ? readError(delivery, 'acceptance-delivery-error') : null}
            {neutral.isError ? readError(neutral, 'acceptance-connectors-error') : null}
            {exports.isError ? readError(exports, 'acceptance-exports-error') : null}
            {selectedSnapshot?.isError ? readError(selectedSnapshot, 'acceptance-snapshot-error') : null}
            {selectedSnapshot?.isPending || reviewLoading || (Boolean(evidenceId) && evidence.isPending) || (Boolean(executionId) && execution.isPending) || (Boolean(claimId) && claim.isPending) || completionLoading || integrationLoading || delivery.isPending || neutral.isPending || submission.isPending || (Boolean(collaboration?.source_state) && source.isPending) || exports.isPending ? loading : null}
            {!acceptanceLoading && !acceptanceUnknown && (!selectedRound || !evidenceId || !executionId || !claimId || !selectedCompletionId) ? empty : null}
          </Space>
        </Col>
      </Row>
    </Space>
  );
}
