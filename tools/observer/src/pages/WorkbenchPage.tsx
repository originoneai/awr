import { useQuery } from '@tanstack/react-query';
import { Spin } from 'antd';
import { useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import { Link } from 'react-router';
import { deriveOverview } from '../core/overview';
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
import { readClockOffsetUnixMs, referenceNowUnixMs, useDisplayTimeZone } from '../lib/display-clock';
import { usePerspective } from '../lib/perspective/PerspectiveProvider';
import { overviewModelQueryOptions, projectsQueryOptions } from '../lib/queries';
import { useRevocation } from '../lib/revocation';
import { useScope } from '../lib/scope';
import type { TeamOverviewModelResponse } from '../lib/types';

interface WorkbenchOverviewResponse extends TeamOverviewModelResponse {
  /** Reserved for AWR-PPD-012. No period-update read is issued by this page. */
  latestPeriodUpdate?: WorkbenchPeriodUpdate | null;
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
  // Display clock (AWR-PPO-079): the deployment's pinned zone, and the model read's own
  // derivation clock walked forward with real time; the browser clock is only the
  // fallback while no model has answered.
  const displayTimeZone = useDisplayTimeZone();
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
  const personId = modelInput?.options?.views?.me?.personId ?? null;

  const derived = useMemo(() => {
    if (!modelInput?.options || !effectiveProject) return null;
    const input = {
      ...modelInput,
      options: workbenchInputOptions(modelInput.options, effectiveProject, personId),
    };
    return deriveOverview(input);
  }, [effectiveProject, modelInput, personId]);

  const projectTitles = useMemo(() => {
    const titles: Record<string, string> = {};
    for (const project of projectItems ?? []) {
      if (typeof project.title === 'string' && project.title.trim()) titles[project.key] = project.title;
    }
    return titles;
  }, [projectItems]);

  if (revoked || invalidScope || isAuthError(projects.error) || isAuthError(modelQuery.error)) {
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

  // Display clock (AWR-PPO-079): the reference is this read's derivation clock
  // (input.options.nowUnixMs) walked forward from its own read time, so displayed ages
  // ride the read's clock instead of the wall clock; the reply's page_read time equals
  // its read time and never serves as "now".
  const referenceUnixMs = referenceNowUnixMs(readClockOffsetUnixMs(modelResponse));
  const model = buildMyWorkModel(derived, i18n.language, {
    personId,
    projectId: effectiveProject,
    readAtUnixMs: modelResponse.read_at_unix_ms,
    referenceUnixMs,
    timeZone: displayTimeZone,
    latestPeriodUpdate: modelResponse.latestPeriodUpdate ?? null,
    inputTasks: modelInput?.tasks?.items,
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
      projectTitles={projectTitles}
      freshness={modelResponse.freshness}
      readAtUnixMs={modelResponse.read_at_unix_ms}
      referenceUnixMs={referenceUnixMs}
      state={state}
    />
  );
}
