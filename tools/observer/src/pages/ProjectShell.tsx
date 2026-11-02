import { useQuery } from '@tanstack/react-query';
import { Button, Card, Space, Tabs, Tag, Tooltip, Typography } from 'antd';
import { useTranslation } from 'react-i18next';
import { useParams, useSearchParams } from 'react-router';
import { ObservationEmpty, ObservationForbidden } from '../components/states';
import { ObservationValue } from '../components/ObservationValue';
import { TechnicalDetails } from '../components/TechnicalDetails';
import { isAuthError } from '../lib/api';
import { ProjectAcceptance } from './ProjectAcceptance';
import { ProjectKnowledge } from './ProjectKnowledge';
import { ProjectOverview } from './ProjectOverview';
import { WorkPlanPage } from './WorkPlanPage';
import { projectsQueryOptions } from '../lib/queries';
import { useRevocation } from '../lib/revocation';

const TAB_KEYS = ['overview', 'plan', 'deps', 'acceptance', 'knowledge'] as const;
type ProjectTab = (typeof TAB_KEYS)[number];

function asTab(value: string | null): ProjectTab {
  return (TAB_KEYS as readonly string[]).includes(value ?? '') ? (value as ProjectTab) : 'overview';
}

/** Project tab and scope state remain in the URL; unsourced tab bodies stay pending. */
export function ProjectShell() {
  const { t } = useTranslation();
  const { project = '' } = useParams();
  const [searchParams, setSearchParams] = useSearchParams();
  const { revoked } = useRevocation();
  const projects = useQuery(projectsQueryOptions(revoked));
  const meta = projects.data?.projects.find((p) => p.key === project);
  const activeTab = asTab(searchParams.get('tab'));

  if (revoked || isAuthError(projects.error)) return <ObservationForbidden />;

  return (
    <Space direction="vertical" size={16} style={{ width: '100%' }} data-testid="page-project">
      <Card data-testid="project-header">
        <Space align="start" style={{ width: '100%', justifyContent: 'space-between' }}>
          <div>
            <Space size={8} align="center">
              <Typography.Title level={4} style={{ margin: 0 }}>
                {meta?.title ?? project}
              </Typography.Title>
              {meta?.role ? <Tag><ObservationValue value={meta.role} field="role" /></Tag> : null}
            </Space>
            {/* The project key is an engineering identifier: it stays in the technical details
                instead of the default view, where the title and the role tag answer "what is
                this project and where do I stand". */}
            <TechnicalDetails items={[{ label: 'project_key', value: project, testId: 'technical-project-key' }]} />

          </div>
          <Tooltip title={t('shell.later')}>
            <Button disabled>{t('project.settings')}</Button>
          </Tooltip>
        </Space>
      </Card>
      <Card>
        <Tabs
          activeKey={activeTab}
          onChange={(key) => {
            const next = new URLSearchParams(searchParams);
            next.set('tab', key);
            next.delete('page');
            next.delete('work');
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
                    : <ObservationEmpty />,
          }))}
        />
      </Card>
    </Space>
  );
}
