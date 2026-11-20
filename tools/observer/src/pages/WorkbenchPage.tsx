import { useQuery } from '@tanstack/react-query';
import { Spin } from 'antd';
import { useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import { Link } from 'react-router';
import { deriveOverview } from '../core/overview';
import type { OverviewMember } from '../core/overview';
import { EmptyState, Notice, PageHeader } from '../components/display';
import { ObservationForbidden } from '../components/states';
import { FreshnessBadge } from '../components/FreshnessBadge';
import { WorkbenchFrame, type WorkbenchFrameState } from '../components/workbench/WorkbenchFrame';
import {
  buildMyWorkModel,
  workbenchInputOptions,
  type WorkbenchPeriodUpdate,
} from '../components/workbench/model';
import { isAuthError } from '../lib/api';
import { usePerspective } from '../lib/perspective/PerspectiveProvider';
import {
  overviewModelQueryOptions,
  projectsQueryOptions,
  workNextQueryOptions,
} from '../lib/queries';
import { useRevocation } from '../lib/revocation';
import { useScope } from '../lib/scope';
import type { TeamOverviewModelResponse, TeamReadResponse, WorkNextData } from '../lib/types';

interface WorkbenchOverviewResponse extends TeamOverviewModelResponse {
  /** Reserved for AWR-PPD-012. No period-update read is issued by this page. */
  latestPeriodUpdate?: WorkbenchPeriodUpdate | null;
}

function memberIdentity(
  response: TeamReadResponse<WorkNextData> | undefined,
  members: readonly OverviewMember[],
): string | null {
  const identity = response?.result.data.identity as unknown as Record<string, unknown> | undefined;
  if (!identity) return null;
  const explicit = identity.person_id;
  const candidate = typeof explicit === 'string'
    ? explicit
    : typeof identity.actor_id === 'string' ? identity.actor_id : null;
  if (!candidate) return null;
  return members.some((member) => member.personId === candidate) ? candidate : null;
}

function StatePage({ children, testId, freshness }: {
  children: React.ReactNode;
  testId: string;
  freshness?: TeamOverviewModelResponse['freshness'];
}) {
  const { t } = useTranslation();
  return (
    <div className="obs-workbench" data-testid={testId}>
      <PageHeader title={t('workbench.home.headingMe')} />
      <FreshnessBadge freshness={freshness} />
      {children}
    </div>
  );
}

/** Read-only panorama home: scope + composed model + one shared derivation, never page-owned facts. */
export function WorkbenchPage() {
  const { t, i18n } = useTranslation();
  const { projectKey } = useScope();
  const { selected } = usePerspective();
  const { revoked } = useRevocation();
  const projects = useQuery(projectsQueryOptions(revoked));
  const projectItems = projects.data?.projects;
  const onlyProject = projectItems?.length === 1 ? projectItems[0].key : null;
  const effectiveProject = projectKey ?? onlyProject;
  const invalidScope = Boolean(projectKey && projectItems && !projectItems.some((project) => project.key === projectKey));
  const needsSelection = !projectKey && (projectItems?.length ?? 0) > 1;
  const modelQuery = useQuery({
    ...overviewModelQueryOptions(effectiveProject, revoked),
    enabled: Boolean(effectiveProject) && !invalidScope && !needsSelection && !revoked,
  });
  const modelResponse = modelQuery.data as WorkbenchOverviewResponse | undefined;
  const modelInput = modelResponse?.input;
  const suppliedPerson = modelInput?.options?.views?.me?.personId ?? null;
  const identityQuery = useQuery({
    ...workNextQueryOptions(effectiveProject, revoked),
    // The existing member read supplies identity only. All page facts still come from the model.
    enabled: Boolean(effectiveProject && modelInput && !suppliedPerson) && !revoked,
  });
  const personId = suppliedPerson ?? memberIdentity(
    identityQuery.data,
    modelInput?.members.coverage === 'unread' ? [] : modelInput?.members.items ?? [],
  );

  const derived = useMemo(() => {
    if (!modelInput?.options || !effectiveProject) return null;
    const input = {
      ...modelInput,
      options: workbenchInputOptions(modelInput.options, effectiveProject, personId),
    };
    return deriveOverview(input);
  }, [effectiveProject, modelInput, personId]);

  if (revoked || invalidScope || isAuthError(projects.error) || isAuthError(modelQuery.error) || isAuthError(identityQuery.error)) {
    return <ObservationForbidden />;
  }
  if (selected !== 'me') {
    return (
      <StatePage testId="workbench-not-open" freshness={modelResponse?.freshness}>
        <EmptyState title={t('workbench.home.notOpenTitle')} description={t('workbench.home.notOpenDescription')} />
      </StatePage>
    );
  }
  if (needsSelection) {
    return (
      <StatePage testId="workbench-select-project">
        <EmptyState title={t('workbench.home.selectProjectTitle')} description={t('workbench.home.selectProjectDescription')} />
        <Link className="obs-workbench-project-link" to="/projects">{t('workbench.home.chooseProject')}</Link>
      </StatePage>
    );
  }
  if (projectItems?.length === 0) {
    return (
      <StatePage testId="workbench-no-projects">
        <EmptyState title={t('workbench.home.noProjectsTitle')} description={t('workbench.home.noProjectsDescription')} />
      </StatePage>
    );
  }
  if (projects.isPending || (effectiveProject && modelQuery.isPending)) {
    return (
      <StatePage testId="workbench-loading" freshness={modelResponse?.freshness}>
        <Spin aria-label={t('workbench.home.loading')} />
      </StatePage>
    );
  }
  if (projects.isError || modelQuery.isError || !modelResponse || !derived) {
    return (
      <StatePage testId="workbench-failed" freshness={modelResponse?.freshness}>
        <Notice tone="warning" title={t('workbench.home.failedTitle')}>{t('workbench.home.failedDescription')}</Notice>
      </StatePage>
    );
  }

  const model = buildMyWorkModel(derived, i18n.language, {
    personId,
    projectId: effectiveProject,
    readAtUnixMs: modelResponse.read_at_unix_ms,
    latestPeriodUpdate: modelResponse.latestPeriodUpdate ?? null,
  });
  const state: WorkbenchFrameState = modelResponse.ordering === 'incomparable'
    ? 'unknown'
    : derived.tasks.length === 0
      ? 'empty'
      : model.coverage.incompleteFacts ? 'partial' : 'data';
  const projectName = projectItems?.find((project) => project.key === effectiveProject)?.title
    ?? t('workbench.home.periodUnknown');
  return (
    <WorkbenchFrame
      model={model}
      projectKey={effectiveProject!}
      projectName={projectName}
      freshness={modelResponse.freshness}
      readAtUnixMs={modelResponse.read_at_unix_ms}
      state={state}
    />
  );
}
