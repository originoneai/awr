import { useQuery } from '@tanstack/react-query';
import { Tabs, Typography } from 'antd';
import { useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import { useNavigate, useParams, useSearchParams } from 'react-router';
import { DisplayIcon } from '../components/display/DisplayIcon';
import { HealthTag, StatusLamp } from '../components/display/StatusTag';
import { ArrowLeftIcon } from '../components/icons/prototype-icons';
import type { ProjectListEntry, ProjectListModel } from '../components/projects/project-list-model';
import { deriveProjectOverview } from '../components/projects/project-overview-model';
import { ObservationForbidden } from '../components/states';
import { isAuthError } from '../lib/api';
import { readClockOffsetUnixMs, referenceNowUnixMs, useDisplayTimeZone } from '../lib/display-clock';
import { projectLabel } from '../lib/project-label';
import { overviewModelQueryOptions, overviewQueryOptions, projectsQueryOptions } from '../lib/queries';
import { useRevocation } from '../lib/revocation';
import { PERSON_ROUTE_BASE } from './MembersPage';
import { ProjectAcceptance } from './ProjectAcceptance';
import { ProjectDependencies } from './ProjectDependencies';
import { ProjectKnowledge } from './ProjectKnowledge';
import { ProjectOverview } from './ProjectOverview';
import { WorkPlanPage } from './WorkPlanPage';
import './ProjectShell.css';

const TAB_KEYS = ['overview', 'plan', 'deps', 'acceptance', 'knowledge'] as const;
type ProjectTab = (typeof TAB_KEYS)[number];

function asTab(value: string | null): ProjectTab {
  return (TAB_KEYS as readonly string[]).includes(value ?? '') ? (value as ProjectTab) : 'overview';
}

/** Prototype project() header + five tabs; overview/model queries share the cache with child tabs. */
export function ProjectShell() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const { project = '' } = useParams();
  const [searchParams, setSearchParams] = useSearchParams();
  const { revoked } = useRevocation();
  const displayTimeZone = useDisplayTimeZone();

  const projects = useQuery(projectsQueryOptions(revoked));
  const overview = useQuery(overviewQueryOptions(project, revoked));
  const modelQuery = useQuery(overviewModelQueryOptions(project || null, revoked));

  const projectEntries = projects.data?.projects as ProjectListEntry[] | undefined;
  const metaIndex = projectEntries ? projectEntries.findIndex((item) => item.key === project) : -1;
  const meta = metaIndex >= 0 && projectEntries ? projectEntries[metaIndex] : undefined;
  const activeTab = asTab(searchParams.get('tab'));

  const modelData = modelQuery.data?.project === project && modelQuery.data.input && modelQuery.data.input.options
    ? (modelQuery.data as ProjectListModel)
    : undefined;
  const referenceUnixMs = referenceNowUnixMs(readClockOffsetUnixMs(modelData));
  const headerFacts = useMemo(() => deriveProjectOverview({
    projectKey: project,
    projectEntry: meta,
    overview: overview.data,
    model: modelData,
    timeZone: displayTimeZone,
    referenceUnixMs,
  }), [project, meta, overview.data, modelData, displayTimeZone, referenceUnixMs]);

  if (revoked || isAuthError(projects.error) || isAuthError(overview.error) || isAuthError(modelQuery.error)) {
    return <ObservationForbidden />;
  }

  const title = meta
    ? projectLabel(meta, metaIndex + 1)
    : projects.isPending
      ? t('common.loading')
      : t('shell.unnamedProject', { number: 1 });

  const leadName = headerFacts.lead.name;
  const leadHref = leadName
    ? `/${PERSON_ROUTE_BASE}/${encodeURIComponent(headerFacts.lead.id ?? leadName)}?scope=${encodeURIComponent(project)}`
    : null;
  const groupText = headerFacts.groupName ?? t('project.groupUnknown');
  const taskCountText = headerFacts.taskCount.count !== null
    ? t('project.taskCount', { count: headerFacts.taskCount.count })
    : t('project.taskCountUnknown');
  const workstreamCountText = headerFacts.workstreamCount !== null
    ? t('project.workstreamCount', { count: headerFacts.workstreamCount })
    : t('project.workstreamCountUnknown');

  const statusTag = headerFacts.health !== 'unknown'
    ? <HealthTag status={headerFacts.health} />
    : (
      <span className="obs-tag" data-testid="project-status-unknown" title={t('projectOverview.unknownStatusMeaning')}>
        <StatusLamp lamp="none" />
        {t('projectOverview.unknownStatus')}
      </span>
    );

  return (
    <div className="project-shell" data-testid="page-project">
      <button
        type="button"
        className="project-back"
        data-testid="project-back"
        onClick={() => navigate('/projects')}
      >
        <DisplayIcon icon={ArrowLeftIcon} />
        {t('project.backToAll')}
      </button>

      <header className="project-hero" data-testid="project-header">
        <div className="project-hero-main">
          <Typography.Title level={1} className="project-title" data-testid="project-title">
            {title}
          </Typography.Title>
          <div className="project-meta" data-testid="project-meta">
            {statusTag}
            {leadName ? (
              <>
                <span>{t('project.leadLabel')}</span>
                <button
                  type="button"
                  className="project-lead-link"
                  data-testid="project-lead-link"
                  onClick={() => { if (leadHref) navigate(leadHref); }}
                >
                  <span className="project-avatar" aria-hidden="true">{leadName.slice(0, 1)}</span>
                  {leadName}
                </button>
              </>
            ) : (
              <span data-testid="project-lead-unknown">{t('project.leadUnknown')}</span>
            )}
            <span data-testid="project-facts-meta">
              {`${groupText} · ${taskCountText} · ${workstreamCountText}`}
            </span>
          </div>
        </div>
      </header>

      <Tabs
        className="project-tabs"
        activeKey={activeTab}
        animated={false}
        onChange={(key) => {
          const next = new URLSearchParams(searchParams);
          next.set('tab', key);
          next.delete('page');
          next.delete('work');
          next.delete('detail');
          next.delete('stream');
          setSearchParams(next);
        }}
        items={TAB_KEYS.map((key) => ({
          key,
          label: t(`project.tabs.${key}`),
          children: key === 'overview'
            ? <ProjectOverview project={project} />
            : key === 'plan'
              ? <WorkPlanPage project={project} />
              : key === 'knowledge'
                ? <ProjectKnowledge project={project} />
                : key === 'acceptance'
                  ? <ProjectAcceptance project={project} />
                  : <ProjectDependencies project={project} />,
        }))}
      />
    </div>
  );
}
