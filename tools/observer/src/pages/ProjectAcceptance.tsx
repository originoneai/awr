import { useQueries, useQuery } from '@tanstack/react-query';
import { Card, Col, Descriptions, List, Pagination, Row, Space, Tag, Tooltip, Typography, theme } from 'antd';
import { isValidElement } from 'react';
import { useTranslation } from 'react-i18next';
import { useSearchParams } from 'react-router';
import { ArtifactReader } from '../components/ArtifactReader';
import { EvidenceChain } from '../components/EvidenceChain';
import { FreshnessBadge } from '../components/FreshnessBadge';
import { ObservationEmpty, ObservationForbidden, PartialFailure } from '../components/states';
import { FieldLabel, ObservationValue } from '../components/ObservationValue';
import { ApiError, isAuthError } from '../lib/api';
import { formatObservationValue, notProvided } from '../i18n/craft';
import { craftValueStyle } from '../theme';
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

/** Exact outcome evidence; no status is inferred from CI, merge or artifact readability. */
export function ProjectAcceptance({ project }: { project: string }) {
  const { t, i18n } = useTranslation();
  const { token } = theme.useToken();
  const { revoked } = useRevocation();
  const [searchParams, setSearchParams] = useSearchParams();
  const overview = useQuery(overviewQueryOptions(project, revoked));
  const works = overview.data?.works ?? [];
  const requestedWork = searchParams.get('work');
  const requestedIndex = requestedWork ? works.findIndex((work) => work.key === requestedWork) : -1;
  const lastPage = Math.max(1, Math.ceil(works.length / PAGE_SIZE));
  const requestedPage = validPage(searchParams.get('page'));
  const page = requestedIndex >= 0 ? Math.floor(requestedIndex / PAGE_SIZE) + 1 : Math.min(requestedPage, lastPage);
  const visibleWorks = works.slice((page - 1) * PAGE_SIZE, page * PAGE_SIZE);
  const selectedWork = visibleWorks.find((work) => work.key === requestedWork) ?? visibleWorks[0];
  const snapshots = useQueries({
    queries: visibleWorks.map((work) => workSnapshotQueryOptions(project, work.key, work.workstream_id, revoked)),
  });
  const selectedSnapshot = snapshots.find((query) => query.data?.result.data.work_id === selectedWork?.key);
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
  const timeValue = (input: unknown) => {
    const raw = asText(input);
    if (raw == null || !Number.isFinite(Number(raw))) return missing;
    return (
      <Tooltip title={raw}>
        <Typography.Text className="craft-value" style={craftValueStyle}>{formatUnixMs(Number(raw), i18n.language)}</Typography.Text>
      </Tooltip>
    );
  };
  const fields = (input: Array<[string, unknown]>) => input.map(([field, child]) => ({
    key: field,
    label: <FieldLabel field={field} />,
    children: isValidElement(child) ? child : <ObservationValue value={child} field={field} />,
  }));
  const readError = (query: { error: unknown }, testId: string) => (
    <Typography.Text type="warning" data-testid={testId}>
      {t('acceptance.readFailed', { code: errorCode(query.error) })}
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
            <List
              size="small"
              dataSource={visibleWorks}
              rowKey={(work) => work.key}
              renderItem={(work) => {
                const query = snapshots.find((item) => item.data?.result.data.work_id === work.key);
                const state = query?.data?.result.data.observation?.collaboration?.review?.state;
                return (
                  <List.Item
                    data-testid={`acceptance-work-${work.key}`}
                    onClick={() => selectWork(work.key)}
                    style={{
                      cursor: 'pointer',
                      background: selectedWork?.key === work.key ? token.colorFillSecondary : undefined,
                      border: selectedWork?.key === work.key ? `1px solid ${token.colorPrimary}` : undefined,
                      borderRadius: selectedWork?.key === work.key ? token.borderRadius : undefined,
                    }}
                  >
                    <Space direction="vertical" size={2}>
                      <Typography.Text strong style={{ overflowWrap: 'anywhere' }}>{work.title}</Typography.Text>
                      <Typography.Text type="secondary">{state ?? t('acceptance.notObserved')}</Typography.Text>
                    </Space>
                  </List.Item>
                );
              }}
            />
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
            <Card size="small" title={t('acceptance.version')} data-testid="acceptance-version">
              {selectedSnapshot?.isError ? <PartialFailure errorCode={errorCode(selectedSnapshot.error)}>{null}</PartialFailure> : (
                <Descriptions size="small" column={{ xs: 1, sm: 2 }} items={fields([
                  ['work_id', selectedWork.key],
                  ['contract_hash', snapshotData?.contract_hash],
                  ['candidate_digest', collaboration?.candidate?.digest],
                  ['candidate_current', collaboration?.candidate?.current],
                  ['selection_version', collaboration?.candidate?.selection_version],
                  ['source_revision', at(reviewObject, ['submission', 'source_revision', 'value'])],
                ])} />
              )}
            </Card>

            <Card size="small" title={t('acceptance.evidenceChain')} data-testid="acceptance-evidence-chain">
              <EvidenceChain
                candidateCurrent={collaboration?.candidate?.current}
                reviewState={reviewObject?.state}
                reviewCurrent={reviewIsCurrent}
                integrationState={integrationData?.state ?? collaboration?.integration?.state}
                completionState={completionData?.runtime_state ?? snapshotData?.observation?.runtime?.state}
              />
            </Card>

            <Card size="small" title={t('acceptance.review')}>
              {!selectedRound ? empty : review.isPending ? loading : review.isError ? readError(review, 'acceptance-review-error') : (
                <Space direction="vertical" size={12} style={{ width: '100%' }} data-testid={`acceptance-review-${selectedRound}`}>
                  <Space size={8} wrap>
                    <Tag>{value(reviewObject?.state, 'state')}</Tag>
                    {reviewIsCurrent && reviewObject?.state === 'approved' ? (
                      <Tag color="success" data-testid="acceptance-current-approval">{t('acceptance.currentApproval')}</Tag>
                    ) : null}
                    {!reviewIsCurrent ? <Tag color="warning" data-testid="acceptance-review-history">{t('acceptance.historical')}</Tag> : null}
                    {reviewCandidateCurrent == null ? <Tag data-testid="acceptance-review-currentness">{missing}</Tag> : null}
                    {at(reviewObject, ['evidence_provenance', 'author_self_report']) === true ? <Tag>{t('acceptance.selfReport')}</Tag> : null}
                    {at(reviewObject, ['evidence_provenance', 'execution_consistency', 'independent_verification']) === false ? <Tag>{t('acceptance.nonIndependent')}</Tag> : null}
                  </Space>
                  <Descriptions size="small" column={{ xs: 1, sm: 2 }} items={fields([
                    ['round_id', reviewObject?.round_id],
                    ['round_index', reviewObject?.round_index],
                    ['author_actor_id', reviewObject?.author_actor_id],
                    ['bundle_hash', reviewObject?.bundle_hash],
                    ['artifact_digest', reviewObject?.artifact_digest],
                  ])} />
                  {asArray(reviewObject?.decisions).map((decision, index) => (
                    <Card key={asText(at(decision, ['decision_id'])) ?? index} size="small" type="inner">
                      <Descriptions
                        size="small"
                        column={1}
                        items={[
                          ...fields([
                            ['decision', at(decision, ['decision'])],
                            ['reviewer_actor_id', at(decision, ['reviewer_actor_id'])],
                            ['independence_kind', at(decision, ['independence_kind'])],
                            ['approval_basis', at(decision, ['approval_basis'])],
                            ['team_independent_acceptance', at(decision, ['team_independent_acceptance'])],
                            ['reason', at(decision, ['reason'])],
                          ]),
                          { key: 'human_approval', label: <FieldLabel field="human_approval" />, children: <span data-testid="decision-human-approval">{value(at(decision, ['human_approval']))}</span> },
                        ]}
                      />
                    </Card>
                  ))}
                  {asArray(at(reviewObject, ['findings', 'findings'])).map((finding, index) => (
                    <Card key={asText(at(finding, ['finding_key'])) ?? index} size="small" type="inner">
                      <Typography.Text strong data-testid={`finding-original-${asText(at(finding, ['finding_key']))}`}>
                        {t('acceptance.originalWork', { work: workId, round: value(at(finding, ['round'])) })} · {value(at(finding, ['criterion']))}
                      </Typography.Text>
                      <Descriptions size="small" column={1} items={fields([
                        ['criterion', at(finding, ['criterion'])],
                        ['rule', at(finding, ['rule'])],
                        ['severity', at(finding, ['severity'])],
                        ['state', at(finding, ['state'])],
                        ['summary', at(finding, ['summary'])],
                      ])} />
                    </Card>
                  ))}
                </Space>
              )}
            </Card>

            <Row gutter={[16, 16]}>
              <Col xs={24} xl={10}>
                <Card size="small" title={t('acceptance.evidence')}>
                  {!evidenceId ? empty : evidence.isPending ? loading : evidence.isError ? readError(evidence, 'acceptance-evidence-error') : (
                    <div data-testid="acceptance-evidence">
                      <Descriptions size="small" column={1} items={fields([
                        ['evidence_id', at(evidence.data?.result.data, ['evidence', 'evidence_id'])],
                        ['execution_id', at(evidence.data?.result.data, ['evidence', 'execution_id'])],
                        ['artifact_id', at(evidence.data?.result.data, ['evidence', 'artifact_id'])],
                        ['artifact_digest', at(evidence.data?.result.data, ['evidence', 'artifact_digest'])],
                        ['trust_basis', at(evidence.data?.result.data, ['evidence', 'trust_basis'])],
                      ])} />
                    </div>
                  )}
                </Card>
              </Col>
              <Col xs={24} xl={8}>
                <Card size="small" title={t('acceptance.execution')}>
                  {!executionId ? empty : execution.isPending ? loading : execution.isError ? readError(execution, 'acceptance-execution-error') : (
                    <div data-testid="acceptance-execution">
                      <Descriptions size="small" column={1} items={fields([
                        ['execution_id', execution.data?.result.data.execution_id],
                        ['state', execution.data?.result.data.state],
                        ['artifact_verified', execution.data?.result.data.artifact_verified],
                        ['artifact_verification_basis', execution.data?.result.data.artifact_verification_basis],
                        ['effects_settled', execution.data?.result.data.effects_settled],
                        ['contract_matches_current', execution.data?.result.data.contract_matches_current],
                        ['epoch_matches_current', execution.data?.result.data.epoch_matches_current],
                        ['execution_authorized', execution.data?.result.data.execution_authorized],
                      ])} />
                    </div>
                  )}
                </Card>
              </Col>
              <Col xs={24} xl={6}>
                <Card size="small" title={t('acceptance.claim')}>
                  {!claimId ? empty : claim.isPending ? loading : claim.isError ? readError(claim, 'acceptance-claim-error') : (
                    <div data-testid="acceptance-claim">
                      <Descriptions size="small" column={1} items={fields([
                        ['claim_id', claim.data?.result.data.claim_id],
                        ['state', claim.data?.result.data.state],
                        ['lease_live', claim.data?.result.data.lease_live],
                        ['current_fence', claim.data?.result.data.current_fence],
                        ['epoch_matches_current', claim.data?.result.data.epoch_matches_current],
                        ['execution_authorized', claim.data?.result.data.execution_authorized],
                        ['expires_at', claim.data?.result.data.expires_at],
                      ])} />
                    </div>
                  )}
                </Card>
              </Col>
            </Row>

            {selectedCompletionId ? (
              <Card size="small" title={t('acceptance.completion')}>
                {completion.isPending ? loading : completion.isError ? readError(completion, 'acceptance-completion-error') : completionConflict ? (
                  <Typography.Text type="warning" data-testid="acceptance-completion-conflict">
                    {t('acceptance.conflict')}
                  </Typography.Text>
                ) : (
                  <div data-testid="acceptance-completion">
                    <Descriptions
                      size="small"
                      column={{ xs: 1, sm: 2 }}
                      items={[
                        ...fields([
                          ['receipt_id', completionReceipt],
                          ['selected_completion_id', completionSelected],
                          ['runtime_state', completionData?.runtime_state],
                          ['contract_hash', completionObject?.contract_hash],
                          ['evidence_id', completionObject?.evidence_id],
                          ['execution_id', completionObject?.execution_id],
                          ['human_approval', at(completionObject, ['approved_by', 'human_approval'])],
                          ['team_independent_acceptance', at(completionObject, ['approved_by', 'team_independent_acceptance'])],
                        ]),
                        { key: 'review_round_id', label: <FieldLabel field="review_round_id" />, children: <span data-testid="completion-review-round">{value(completionReviewRound)}</span> },
                        { key: 'review_decision_id', label: <FieldLabel field="review_decision_id" />, children: <span data-testid="completion-review-decision">{value(completionReviewDecision)}</span> },
                        { key: 'independence_kind', label: <FieldLabel field="independence_kind" />, children: <span data-testid="completion-independence">{value(completionIndependence, 'independence_kind')}</span> },
                      ]}
                    />
                  </div>
                )}
              </Card>
            ) : null}

            <Card size="small" title={t('acceptance.localGit')}>
              {neutral.isPending || submission.isPending ? loading : neutral.isError ? readError(neutral, 'acceptance-connectors-error') : (
                <div data-testid="acceptance-connectors">
                  {connectors.length === 0 ? empty : connectors.map((connector, index) => (
                    <Descriptions key={asText(at(connector, ['connector_id'])) ?? index} size="small" column={{ xs: 1, sm: 2 }} items={fields([
                      ['provider', at(connector, ['provider'])],
                      ['connector_id', at(connector, ['connector_id'])],
                      ['connector_version', at(connector, ['connector_version'])],
                      ['enabled', at(connector, ['enabled'])],
                      ['current_epoch', at(connector, ['current_epoch'])],
                      ['fact_source', at(connector, ['fact_source'])],
                      ['resource', at(connector, ['resource'])],
                      ['read_only', at(submission.data?.result.data, ['read_only'])],
                    ])} />
                  ))}
                </div>
              )}
            </Card>

            {integrationId ? (
              <Card size="small" title={t('acceptance.integration')}>
                {integration.isPending ? loading : integration.isError ? readError(integration, 'acceptance-integration-error') : (
                  <div data-testid="acceptance-integration">
                    <Descriptions
                      size="small"
                      column={{ xs: 1, sm: 2 }}
                      items={[
                        ...fields([
                          ['integration_id', integrationData?.integration_id],
                          ['target', at(integrationData, ['integration_request', 'binding', 'target', 'reference'])],
                          ['target_resource', at(integrationData, ['integration_request', 'binding', 'target', 'resource'])],
                          ['precondition', at(integrationData, ['integration_request', 'binding', 'target', 'precondition', 'revision', 'value'])],
                          ['result_revision', at(integrationData, ['confirmation', 'envelope', 'record', 'data', 'result_revision', 'value'])],
                        ]),
                        { key: 'outcome', label: <FieldLabel field="outcome" />, children: <span data-testid="acceptance-integration-outcome">{value(at(integrationData, ['confirmation', 'envelope', 'record', 'data', 'outcome']), 'outcome')}</span> },
                        { key: 'state', label: <FieldLabel field="state" />, children: <span data-testid="acceptance-integration-state">{value(integrationData?.state, 'state')}</span> },
                        { key: 'dispatch_state', label: <FieldLabel field="dispatch_state" />, children: <span data-testid="acceptance-dispatch-state">{value(at(integrationData, ['dispatch_receipt', 'data', 'state']), 'dispatch_state')}</span> },
                        ...fields([
                          ['current_binding', integrationData?.current_binding],
                          ['target_changed', integrationData?.target_changed],
                          ['dispatched_at', integrationData?.dispatched_at],
                        ]),
                      ]}
                    />
                    {at(integrationData, ['confirmation', 'envelope', 'record', 'data', 'outcome']) === 'rejected' ? (
                      <Tag color="error" data-testid="acceptance-integration-rejected">{t('acceptance.integrationRejected')}</Tag>
                    ) : null}
                  </div>
                )}
              </Card>
            ) : null}

            <Card size="small" title={t('acceptance.source')}>
              {!collaboration?.source_state ? empty : source.isPending ? loading : source.isError ? readError(source, 'acceptance-source-error') : (
                <div data-testid="acceptance-source">
                  <Descriptions size="small" column={{ xs: 1, sm: 2 }} items={fields([
                    ['source_observation', source.data?.result.data.source_observation],
                    ['confirmed_fingerprint', source.data?.result.data.confirmed_fingerprint],
                    ['source_fingerprint', source.data?.result.data.source_fingerprint],
                    ['source_synchronized', source.data?.result.data.source_synchronized],
                    ['cursor_current', source.data?.result.data.cursor_current],
                    ['projection_current', source.data?.result.data.projection_current],
                    ['pending_publication_id', source.data?.result.data.pending_publication_id],
                    ['metadata_revision', source.data?.result.data.metadata_revision],
                    ['history_truncated', source.data?.result.data.history_truncated],
                  ])} />
                </div>
              )}
            </Card>

            <Card size="small" title={t('acceptance.delivery')}>
              {delivery.isPending ? loading : delivery.isError ? readError(delivery, 'acceptance-delivery-error') : (
                <div data-testid="acceptance-delivery">
                  <Descriptions
                    size="small"
                    column={{ xs: 1, sm: 2 }}
                    items={[
                      { key: 'complete', label: <FieldLabel field="complete" />, children: <span data-testid="acceptance-delivery-complete">{completionConflict ? t('acceptance.conflict') : value(at(deliveryObject, ['awr_acceptance', 'complete']))}</span> },
                      ...fields([
                        ['runtime_state', at(deliveryObject, ['awr_acceptance', 'runtime_state'])],
                        ['selected_completion_id', deliverySelected],
                        ['github.submitted', at(deliveryObject, ['github', 'submitted'])],
                        ['github.approved', at(deliveryObject, ['github', 'approved'])],
                        ['github.merged', at(deliveryObject, ['github', 'merged'])],
                        ['pr', deliveryObject?.pr],
                        ['webhook_auto_sync', deliveryObject?.webhook_auto_sync],
                        ['cannot_skip_acceptance_via', deliveryObject?.cannot_skip_acceptance_via],
                      ]),
                    ]}
                  />
                </div>
              )}
            </Card>

            <Card size="small" title={t('acceptance.exports')}>
              {exports.isPending ? loading : exports.isError ? readError(exports, 'acceptance-exports-error') : (
                <div data-testid="acceptance-exports">
                  <Descriptions size="small" column={{ xs: 1, sm: 2 }} items={fields([
                    ['adoption_available', exportData?.adoption_available],
                    ['adoption_version_policies', exportData?.adoption_version_policies],
                    ['upstream_source_access', exportData?.upstream_source_access],
                    ['items_count', exportItems.length],
                    ['next_cursor', exportData?.next_cursor],
                  ])} />
                  {exportItems.length > 0 ? <Typography.Text type="warning">{t('acceptance.exportDetailsLater')}</Typography.Text> : empty}
                </div>
              )}
            </Card>

            <Card size="small" title={t('acceptance.artifacts')}>
              <div data-testid="acceptance-artifacts">
                {artifacts.length === 0 ? empty : artifacts.map((artifact, index) => {
                  const artifactId = asText(at(artifact, ['query', 'artifact_id']));
                  return (
                    <div key={artifactId ?? index} data-testid={`acceptance-artifact-${artifactId}`}>
                      <ArtifactReader project={project} artifact={artifact} revoked={revoked} />
                    </div>
                  );
                })}
              </div>
            </Card>

            <Card size="small" title={t('acceptance.times')}>
              <div data-testid="acceptance-times">
                <Descriptions size="small" column={{ xs: 1, sm: 2 }} items={fields([
                  [t('acceptance.externalObserved'), timeValue(observedAt)],
                  [t('acceptance.awrRecorded'), timeValue(recordedAt)],
                  [t('acceptance.pageRead'), timeValue(selectedSnapshot?.data?.freshness.page_read.at_unix_ms)],
                  [t('acceptance.sourceQuery'), timeValue(collaboration?.source_state?.query_started_at_unix_ms)],
                ])} />
              </div>
            </Card>
          </Space>
        </Col>
      </Row>
    </Space>
  );
}
