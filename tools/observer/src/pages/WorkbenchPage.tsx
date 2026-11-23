import { useQuery } from '@tanstack/react-query';
import { Spin } from 'antd';
import { useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import { Link } from 'react-router';
import { deriveOverview } from '../core/overview';
import { EmptyState, Notice, PageHeader } from '../components/display';
import { ObservationForbidden } from '../components/states';
import { FreshnessBadge } from '../components/FreshnessBadge';
import { DepartmentWorkbench } from '../components/workbench/DepartmentWorkbench';
import {
  buildDepartmentWorkbenchModel,
  portfolioClockOffsetUnixMs,
  portfolioReadMomentUnixMs,
} from '../components/workbench/department-model';
import { WorkbenchFrame, type WorkbenchFrameState } from '../components/workbench/WorkbenchFrame';
import {
  buildMyWorkModel,
  workbenchInputOptions,
  type WorkbenchPeriodUpdate,
} from '../components/workbench/model';
import { isAuthError } from '../lib/api';
import { readClockOffsetUnixMs, readMomentUnixMs, referenceNowUnixMs, useDisplayTimeZone } from '../lib/display-clock';
import { usePerspective } from '../lib/perspective/PerspectiveProvider';
import { overviewModelQueryOptions, portfolioQueryOptions, projectsQueryOptions } from '../lib/queries';
import { useRevocation } from '../lib/revocation';
import { useScope } from '../lib/scope';
import type { TeamFreshness, TeamOverviewModelResponse } from '../lib/types';

interface WorkbenchOverviewResponse extends TeamOverviewModelResponse {
  /** Reserved for AWR-PPD-012. No period-update read is issued by this page. */
  latestPeriodUpdate?: WorkbenchPeriodUpdate | null;
}

const FAILED_FRESHNESS: TeamFreshness = {
  report_received: { status: 'not_measured' },
  authoritative: { status: 'failed' },
  page_read: { status: 'failed' },
};

function StatePage({ children, testId, freshness, heading }: {
  children: React.ReactNode;
  testId: string;
  freshness?: TeamFreshness;
  heading?: string;
}) {
  const { t } = useTranslation();
  return (
    <div className="obs-workbench" data-testid={testId}>
      <PageHeader title={heading ?? t('workbench.home.headingMe')} />
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
  const isDepartment = selected === 'dept';
  const isMe = selected === 'me';

  const projects = useQuery({
    ...projectsQueryOptions(revoked),
    enabled: !revoked && !isDepartment,
  });
  const projectItems = projects.data?.projects;
  const onlyProject = projectItems?.length === 1 ? projectItems[0].key : null;
  const effectiveProject = projectKey ?? onlyProject;
  const invalidScope = Boolean(!isDepartment && projectKey && projectItems && !projectItems.some((project) => project.key === projectKey));
  const needsSelection = !isDepartment && !projectKey && (projectItems?.length ?? 0) > 1;
  const modelQuery = useQuery({
    ...overviewModelQueryOptions(effectiveProject, revoked),
    enabled: isMe && Boolean(effectiveProject) && !invalidScope && !needsSelection && !revoked,
  });
  // Department projects uses a single composed portfolio read (AWR-PPD-060) and never fans out per project.
  const portfolioQuery = useQuery(portfolioQueryOptions(revoked, isDepartment));

  const modelResponse = modelQuery.data as WorkbenchOverviewResponse | undefined;
  const modelInput = modelResponse?.input;
  const personId = modelInput?.options?.views?.me?.personId ?? null;

  const derived = useMemo(() => {
    if (!isMe || !modelInput?.options || !effectiveProject) return null;
    const input = {
      ...modelInput,
      options: workbenchInputOptions(modelInput.options, effectiveProject, personId),
    };
    return deriveOverview(input);
  }, [effectiveProject, isMe, modelInput, personId]);

  const projectTitles = useMemo(() => {
    const titles: Record<string, string> = {};
    for (const project of projectItems ?? []) {
      if (typeof project.title === 'string' && project.title.trim()) titles[project.key] = project.title;
    }
    return titles;
  }, [projectItems]);

  if (isDepartment) {
    const deptHeading = t('workbench.department.heading');
    if (revoked || isAuthError(portfolioQuery.error)) {
      return <ObservationForbidden />;
    }
    if (portfolioQuery.isPending) {
      return (
        <StatePage testId="workbench-loading" heading={deptHeading}>
          <Spin aria-label={t('workbench.department.loading')} />
        </StatePage>
      );
    }
    const portfolio = portfolioQuery.data;
    if (portfolioQuery.isError || !portfolio) {
      return (
        <StatePage testId="workbench-failed" heading={deptHeading} freshness={FAILED_FRESHNESS}>
          <Notice tone="warning" title={t('workbench.department.failedTitle')}>
            {t('workbench.department.failedDescription')}
          </Notice>
        </StatePage>
      );
    }
    const referenceUnixMs = referenceNowUnixMs(portfolioClockOffsetUnixMs(portfolio));
    const deptModel = buildDepartmentWorkbenchModel(portfolio, i18n.language, {
      readAtUnixMs: portfolioReadMomentUnixMs(portfolio),
      referenceUnixMs,
      timeZone: displayTimeZone,
    });
    const state: WorkbenchFrameState = portfolio.ordering === 'incomparable'
      || portfolio.freshness.authoritative.status === 'not_measured'
      ? 'unknown'
      : deptModel.coverage.totalProjects === 0 && deptModel.projects.length === 0
        ? 'empty'
        : !deptModel.coverage.complete ? 'partial' : 'data';
    return (
      <DepartmentWorkbench
        model={deptModel}
        freshness={portfolio.freshness}
        referenceUnixMs={referenceUnixMs}
        state={state}
      />
    );
  }

  if (revoked || invalidScope || isAuthError(projects.error) || isAuthError(modelQuery.error)) {
    return <ObservationForbidden />;
  }
  if (!isMe) {
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
    // The model counts every age it derives (the source line, the oldest pending wait) from
    // the read moment in the display clock's own time system: on a demo whose fixture clock
    // sits behind the real read time, the real read time would print a date in the future
    // and run durations a day long.
    readAtUnixMs: readMomentUnixMs(modelResponse),
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
