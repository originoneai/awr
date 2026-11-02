import { Card, Typography } from 'antd';
import { useTranslation } from 'react-i18next';
import { Link } from 'react-router';
import { NotOpenYet } from '../components/states';

/**
 * The inbox is not built yet. It must not read as an empty inbox ("no records observed" is what a person
 * reads as "nothing is waiting for me"), so it says it is not open and points to where the work that needs
 * the person is shown today: the "needs me" tab of the workbench.
 */
export function InboxPage() {
  const { t } = useTranslation();
  return (
    <Card data-testid="page-inbox">
      <Typography.Title level={5} style={{ marginTop: 0 }}>
        {t('nav.inbox')}
      </Typography.Title>
      <NotOpenYet />
      <Typography.Paragraph style={{ marginTop: 12, marginBottom: 0 }} data-testid="inbox-pointer">
        {t('inbox.pointerBefore')}
        <Link to="/workbench?tab=needs-me">{t('workbench.tabs.needsMe')}</Link>
        {t('inbox.pointerAfter')}
      </Typography.Paragraph>
    </Card>
  );
}
