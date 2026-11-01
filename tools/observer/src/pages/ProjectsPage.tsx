import { useQuery } from '@tanstack/react-query';
import { Card, List, Space, Tag, Typography } from 'antd';
import { useTranslation } from 'react-i18next';
import { Link } from 'react-router';
import { ObservationEmpty, ObservationForbidden } from '../components/states';
import { ObservationValue } from '../components/ObservationValue';
import { isAuthError } from '../lib/api';
import { projectsQueryOptions } from '../lib/queries';
import { useRevocation } from '../lib/revocation';

/** Project list placeholder: the only real data so far is the projects read. */
export function ProjectsPage() {
  const { t } = useTranslation();
  const { revoked } = useRevocation();
  const projects = useQuery(projectsQueryOptions(revoked));
  const items = projects.data?.projects ?? [];

  if (revoked || isAuthError(projects.error)) return <ObservationForbidden />;

  return (
    <Card data-testid="page-projects">
      <Typography.Title level={5} style={{ marginTop: 0 }}>
        {t('nav.projects')}
      </Typography.Title>
      {items.length === 0 ? (
        <ObservationEmpty />
      ) : (
        <List
          dataSource={items}
          renderItem={(project) => (
            <List.Item>
              <Space>
                <Link to={`/projects/${encodeURIComponent(project.key)}`}>{project.title}</Link>
                <Typography.Text type="secondary">{project.key}</Typography.Text>
                <Tag><ObservationValue value={project.role} /></Tag>
              </Space>
            </List.Item>
          )}
        />
      )}
    </Card>
  );
}
