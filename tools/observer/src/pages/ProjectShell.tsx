import { useQuery } from '@tanstack/react-query';
import { Button, Card, Space, Tabs, Tag, Typography } from 'antd';
import { useTranslation } from 'react-i18next';
import { useParams, useSearchParams } from 'react-router';
import { ObservationEmpty } from '../components/states';
import { projectsQueryOptions } from '../lib/queries';

const TAB_KEYS = ['overview', 'plan', 'deps', 'acceptance', 'knowledge'] as const;
type ProjectTab = (typeof TAB_KEYS)[number];

function asTab(value: string | null): ProjectTab {
  return (TAB_KEYS as readonly string[]).includes(value ?? '') ? (value as ProjectTab) : 'overview';
}

/**
 * Project workspace shell. The active tab is URL state (?tab=) so it survives
 * reload and sharing; every tab body is the shared pending presentation until
 * its slice lands — no fake data anywhere.
 */
export function ProjectShell() {
  const { t } = useTranslation();
  const { project = '' } = useParams();
  const [searchParams, setSearchParams] = useSearchParams();
  const projects = useQuery(projectsQueryOptions());
  const meta = projects.data?.projects.find((p) => p.key === project);
  const activeTab = asTab(searchParams.get('tab'));

  return (
    <Space direction="vertical" size={16} style={{ width: '100%' }} data-testid="page-project">
      <Card data-testid="project-header">
        <Space align="start" style={{ width: '100%', justifyContent: 'space-between' }}>
          <div>
            <Space size={8} align="center">
              <Typography.Title level={4} style={{ margin: 0 }}>
                {meta?.title ?? project}
              </Typography.Title>
              {meta?.role ? <Tag color="blue">{meta.role}</Tag> : null}
            </Space>
            <div>
              <Typography.Text type="secondary">
                {t('project.metaKey')} {project}
                {meta?.role ? ` · ${t('project.metaRole')} ${meta.role}` : ''}
              </Typography.Text>
            </div>
          </div>
          <Button>{t('project.settings')}</Button>
        </Space>
      </Card>
      <Card>
        <Tabs
          activeKey={activeTab}
          onChange={(key) => setSearchParams({ tab: key })}
          items={TAB_KEYS.map((key) => ({
            key,
            label: t(`project.tabs.${key}`),
            children: <ObservationEmpty />,
          }))}
        />
      </Card>
    </Space>
  );
}
