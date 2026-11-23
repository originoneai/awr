import { useQuery } from '@tanstack/react-query';
import { Alert, Button, Space, Spin, Typography } from 'antd';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { documentLabel, readableNarrative, workLabel } from '../core/observation';
import { readsAsName } from '../core/observation/owner';
import { humanText } from '../i18n/human-language';
import { isAuthError } from '../lib/api';
import { formatUnixMs } from '../lib/format';
import { planningCandidatesQueryOptions, requiredSourcesQueryOptions } from '../lib/queries';
import { useRevocation } from '../lib/revocation';
import type { PlanningCandidatesData } from '../lib/types';
import { Section } from './display/Section';
import { ObservationForbidden } from './states';

function PageButtons({ previous, next, refresh }: { previous?: () => void; next?: () => void; refresh: () => void }) {
  const { t } = useTranslation();
  return (
    <Space wrap>
      {previous ? <Button size="small" onClick={previous}>{t('plan.observation.previous')}</Button> : null}
      {next ? <Button size="small" onClick={next}>{t('plan.observation.next')}</Button> : null}
      <Button size="small" aria-label={t('plan.observation.refresh')} onClick={refresh}>
        {t('plan.observation.refresh')}
      </Button>
    </Space>
  );
}

type ProposalStatusKey =
  | 'rejected'
  | 'applied'
  | 'previouslyApplied'
  | 'superseded'
  | 'applying'
  | 'approved'
  | 'awaitingReview'
  | 'unknown';

function proposalStatus(item: PlanningCandidatesData['items'][number]): ProposalStatusKey {
  if (item.state === 'rejected') return 'rejected';
  if (item.activation.value) return item.activation.value.matches_current_snapshot ? 'applied' : 'previouslyApplied';
  if (item.state === 'superseded') return 'superseded';
  if (item.publication.value) return 'applying';
  if (item.approval.value) return 'approved';
  return item.state === 'drafting' ? 'awaitingReview' : 'unknown';
}

function proposalTone(status: ProposalStatusKey): 'ok' | 'risk' | 'off' | 'info' | undefined {
  if (status === 'applied') return 'ok';
  if (status === 'rejected') return 'off';
  if (status === 'awaitingReview') return 'risk';
  if (status === 'approved' || status === 'applying') return 'info';
  return undefined;
}

function ChangeDetails({ fields }: { fields: NonNullable<PlanningCandidatesData['items'][number]['current_diff']['items'][number]['fields']> }) {
  const { t, i18n } = useTranslation();
  const labels = {
    title: 'name',
    acceptance: 'acceptance',
    hard_rules: 'rules',
    verification_requirements: 'checks',
    goals: 'goals',
    required_dependencies: 'dependencies',
    dependency_acceptance: 'upstreamReview',
    execution_settlement: 'resultConfirmation',
    scope_paths: 'scope',
    completion_policy: 'review',
    definition_state: 'availability',
  } as const;

  function content(item: { value: unknown; reason?: string | null }, field: string, prose: boolean) {
    if (item.reason === 'permission_restricted') return t('plan.changes.restricted');
    if (item.value == null) return t('plan.changes.notSet');
    if (field === 'dependency_acceptance' && typeof item.value === 'object' && !Array.isArray(item.value)) {
      const descriptions = new Set<string>();
      for (const policy of Object.values(item.value)) {
        if (policy === 'agent_reviewed_caller_asserted_reconciled') descriptions.add(t('plan.changes.policy.agentReviewed'));
        else if (policy === 'simulated_member_independent') descriptions.add(t('plan.changes.policy.collaboratorReviewed'));
        else if (policy?.cross_workstream) {
          const requirement = policy.cross_workstream;
          const review = requirement.review_assurance === 'team_independent'
            ? t('plan.changes.policy.memberReviewed')
            : requirement.review_assurance === 'simulated_member_independent'
              ? t('plan.changes.policy.collaboratorReviewed')
              : t('plan.changes.policy.needsCheck');
          const version = requirement.version_policy === 'fixed_delivery'
            ? t('plan.changes.policy.fixedResults')
            : requirement.version_policy === 'current_contract'
              ? t('plan.changes.policy.currentResults')
              : t('plan.changes.policy.needsCheck');
          descriptions.add(`${review} · ${version}`);
        } else {
          descriptions.add(t('plan.changes.policy.needsCheck'));
        }
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
    const values = Array.isArray(item.value) ? item.value : [item.value];
    return values
      .filter((value): value is string => typeof value === 'string' && value.trim() !== '')
      .map((value) => readableNarrative(value, i18n.language) ?? humanText('contentUnavailable', i18n.language))
      .join(' · ') || t('plan.changes.notSet');
  }

  return (
    <>
      {fields.map((field, index) => {
        const label = Object.hasOwn(labels, field.field) ? labels[field.field as keyof typeof labels] : 'other';
        const prose = ['title', 'acceptance', 'hard_rules', 'verification_requirements'].includes(field.field);
        const describe = prose
          || ['dependency_acceptance', 'execution_settlement'].includes(field.field)
          || field.before.reason === 'permission_restricted'
          || field.after.reason === 'permission_restricted';
        const previousWorkspace = (field.before.value as { workspace_id?: unknown } | null)?.workspace_id;
        const nextWorkspace = (field.after.value as { workspace_id?: unknown } | null)?.workspace_id;
        const workspaceChanged = field.field === 'execution_settlement'
          && typeof previousWorkspace === 'string'
          && typeof nextWorkspace === 'string'
          && previousWorkspace !== nextWorkspace;
        return (
          <div key={index} className="work-plan-obs-diff-field">
            <span className="work-plan-obs-meta">{t(`plan.changes.fields.${label}`)}</span>
            {describe ? (
              <>
                <span className="work-plan-obs-meta">{t('plan.changes.before', { text: content(field.before, field.field, prose) })}</span>
                <span>{t('plan.changes.after', { text: content(field.after, field.field, prose) })}</span>
              </>
            ) : null}
            {workspaceChanged ? <span>{t('plan.changes.policy.workspaceChanged')}</span> : null}
          </div>
        );
      })}
    </>
  );
}

/** Only business facts are rendered; exact revisions, selectors and digests stay in service history. */
export function PlanningChanges({ project, stream }: { project: string; stream: string | null }) {
  const { t, i18n } = useTranslation();
  const { revoked } = useRevocation();
  const [cursors, setCursors] = useState<Array<string | null>>([null]);
  const query = useQuery(planningCandidatesQueryOptions(project, stream, cursors.at(-1) ?? null, revoked));
  const data = query.data?.result.data;
  const refresh = () => (cursors.length > 1 ? setCursors([null]) : void query.refetch());
  if (revoked || isAuthError(query.error)) return <ObservationForbidden />;

  return (
    <div className="work-plan-obs-card" data-testid="plan-changes">
      <Section
        title={t('plan.changes.title')}
        count={data ? data.total : undefined}
        className="work-plan-section"
      >
        <div className="work-plan-obs-body">
          {query.isPending ? <Spin /> : null}
          {query.error ? (
            <Alert type="warning" showIcon title={t('plan.changes.failed')} description={t('plan.observation.recover')} />
          ) : null}
          {!query.error && data?.items.length === 0 ? (
            <div className="work-plan-obs-meta">{t('plan.changes.empty')}</div>
          ) : null}
          {data ? (
            <>
              {query.error ? <span className="work-plan-obs-meta">{t('plan.observation.lastConfirmed')}</span> : null}
              {data.items.length > 0 ? (
                <ul className="work-plan-obs-list">
                  {data.items.map((item) => {
                    const status = proposalStatus(item);
                    const author = item.author.display_name && readsAsName(item.author.display_name)
                      ? item.author.display_name
                      : item.author.display_name
                        ? humanText('nameUnavailable', i18n.language)
                        : t('plan.changes.authorUnknown');
                    return (
                      <li key={item.candidate_id} className="work-plan-obs-item">
                        <div className="work-plan-obs-item-head">
                          <span className="work-plan-obs-item-title">
                            {t('plan.changes.revision', { count: item.draft_revision })}
                          </span>
                          <span className="obs-tag" data-tone={proposalTone(status)}>
                            {t(`plan.changes.status.${status}`)}
                          </span>
                        </div>
                        <span className="work-plan-obs-meta">
                          {`${author} · ${formatUnixMs(item.updated_at_unix_ms, i18n.language)}`}
                        </span>
                        {!item.baseline.matches_current && !item.activation.value && item.state !== 'rejected' ? (
                          <Alert type="info" showIcon title={t('plan.changes.planChanged')} />
                        ) : null}
                        {item.state === 'rejected' ? (
                          <div className="work-plan-obs-returned" data-testid="plan-returned-explanation">
                            {item.rejection?.value ? (
                              <>
                                <span className="work-plan-obs-meta">
                                  {t('plan.changes.returnedBy', {
                                    name: item.rejection.value.display_name && readsAsName(item.rejection.value.display_name)
                                      ? item.rejection.value.display_name
                                      : item.rejection.value.display_name
                                        ? humanText('nameUnavailable', i18n.language)
                                        : t('plan.changes.reviewerUnknown'),
                                    time: formatUnixMs(item.rejection.value.at_unix_ms, i18n.language),
                                  })}
                                </span>
                                <Typography.Text>
                                  {readableNarrative(item.rejection.value.reason, i18n.language)
                                    ?? t('plan.changes.returnedReasonUnavailable')}
                                </Typography.Text>
                              </>
                            ) : (
                              <span className="work-plan-obs-meta">{t('plan.changes.restricted')}</span>
                            )}
                            <span className="work-plan-obs-meta">{t('plan.changes.returnedNext')}</span>
                          </div>
                        ) : null}
                        <span className="work-plan-obs-meta">
                          {t('plan.changes.summary', item.current_diff.counts)}
                        </span>
                        {item.current_diff.items.map((change) => (
                          <div key={change.work_id} className="work-plan-obs-diff-work">
                            <div>
                              <span>{workLabel(change.title, { language: i18n.language, recordedTitle: true }).text}</span>
                              {' · '}
                              <span className="work-plan-obs-meta">
                                {t(`plan.changes.kind.${change.kind === 'added' || change.kind === 'removed' || change.kind === 'changed' || change.kind === 'unchanged' ? change.kind : 'unknown'}`)}
                              </span>
                            </div>
                            <ChangeDetails fields={change.fields ?? []} />
                          </div>
                        ))}
                      </li>
                    );
                  })}
                </ul>
              ) : null}
              <div className="work-plan-obs-footer">
                <span className="work-plan-obs-meta">
                  {t('plan.observation.updated', { time: formatUnixMs(data.as_of.at_unix_ms, i18n.language) })}
                </span>
                <PageButtons
                  refresh={refresh}
                  previous={cursors.length > 1 ? () => setCursors(cursors.slice(0, -1)) : undefined}
                  next={!query.error && data?.next_cursor ? () => setCursors([...cursors, data.next_cursor]) : undefined}
                />
              </div>
            </>
          ) : (
            <div className="work-plan-obs-footer">
              <PageButtons
                refresh={refresh}
                previous={cursors.length > 1 ? () => setCursors(cursors.slice(0, -1)) : undefined}
              />
            </div>
          )}
        </div>
      </Section>
    </div>
  );
}

export function SourceReadiness({ project, stream, workId }: { project: string; stream: string | null; workId: string | null }) {
  const { t, i18n } = useTranslation();
  const { revoked } = useRevocation();
  const [cursors, setCursors] = useState<Array<string | null>>([null]);
  const query = useQuery(requiredSourcesQueryOptions(project, stream, workId, cursors.at(-1) ?? null, revoked));
  const data = query.data?.result.data;
  const refresh = () => (cursors.length > 1 ? setCursors([null]) : void query.refetch());
  if (revoked || isAuthError(query.error)) return <ObservationForbidden />;

  return (
    <div className="work-plan-obs-card" data-testid="plan-materials">
      <Section
        title={t('plan.materials.title')}
        count={data ? data.collection.required_total : undefined}
        className="work-plan-section"
      >
        <div className="work-plan-obs-body">
          <span className="work-plan-obs-meta">
            {t(workId ? 'plan.materials.workScope' : 'plan.materials.projectScope')}
          </span>
          {query.isPending ? <Spin /> : null}
          {query.error ? (
            <Alert type="warning" showIcon title={t('plan.materials.failed')} description={t('plan.observation.recover')} />
          ) : null}
          {data ? (
            <>
              {query.error ? <span className="work-plan-obs-meta">{t('plan.observation.lastConfirmed')}</span> : null}
              {data.collection.required_total === 0 ? (
                <div className="work-plan-obs-meta">{t('plan.materials.empty')}</div>
              ) : (
                <>
                  {!query.error ? (
                    <Alert
                      type={data.collection.complete ? 'success' : 'warning'}
                      showIcon
                      title={t(data.collection.complete ? 'plan.materials.ready' : 'plan.materials.incomplete')}
                    />
                  ) : null}
                  <ul className="work-plan-obs-list">
                    {data.items.map((source) => {
                      const status = source.status === 'consistent'
                        || source.status === 'changed'
                        || source.status === 'missing'
                        || source.status === 'forbidden'
                        ? source.status
                        : 'unverified';
                      return (
                        <li key={source.source_position} className="work-plan-obs-item">
                          <span>
                            {documentLabel(source.path, { language: i18n.language, position: source.source_position }).text}
                          </span>
                          <span className="work-plan-obs-meta">{t(`plan.materials.status.${status}`)}</span>
                        </li>
                      );
                    })}
                  </ul>
                </>
              )}
              <span className="work-plan-obs-meta">{t('plan.materials.boundary')}</span>
              <div className="work-plan-obs-footer">
                <span className="work-plan-obs-meta">
                  {t('plan.observation.updated', { time: formatUnixMs(data.collection.checked_at_unix_ms, i18n.language) })}
                </span>
                <PageButtons
                  refresh={refresh}
                  previous={cursors.length > 1 ? () => setCursors(cursors.slice(0, -1)) : undefined}
                  next={!query.error && data?.next_cursor ? () => setCursors([...cursors, data.next_cursor]) : undefined}
                />
              </div>
            </>
          ) : (
            <div className="work-plan-obs-footer">
              <PageButtons
                refresh={refresh}
                previous={cursors.length > 1 ? () => setCursors(cursors.slice(0, -1)) : undefined}
              />
            </div>
          )}
        </div>
      </Section>
    </div>
  );
}
