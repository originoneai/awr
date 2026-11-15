import { Card, Space, Typography } from 'antd';
import { useTranslation } from 'react-i18next';
import { Link } from 'react-router';
import { ProjectQuestions } from '../components/ProjectQuestions';
import { useScope } from '../lib/scope';

export function InboxPage() {
  const { t } = useTranslation();
  const { projectKey } = useScope();
  return <Space orientation="vertical" size={16} style={{ width: '100%' }} data-testid="page-inbox">
    <Card>
      <Typography.Title level={3} style={{ marginTop: 0 }}>{t('nav.inbox')}</Typography.Title>
      <Typography.Paragraph type="secondary" style={{ marginBottom: 0 }}>{t('inbox.description')}</Typography.Paragraph>
    </Card>
    {projectKey ? <ProjectQuestions key={projectKey} project={projectKey} /> : <Card>
      <Space orientation="vertical" size={12}>
        <Typography.Text>{t('inbox.chooseProject')}</Typography.Text>
        <Link to="/projects">{t('inbox.openProjects')}</Link>
      </Space>
    </Card>}
  </Space>;
}
