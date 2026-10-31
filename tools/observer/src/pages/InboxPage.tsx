import { Card, Typography } from 'antd';
import { useTranslation } from 'react-i18next';
import { ObservationEmpty } from '../components/states';

/** Inbox placeholder: the coordination inbox read wires up in a later slice. */
export function InboxPage() {
  const { t } = useTranslation();
  return (
    <Card data-testid="page-inbox">
      <Typography.Title level={5} style={{ marginTop: 0 }}>
        {t('nav.inbox')}
      </Typography.Title>
      <ObservationEmpty />
    </Card>
  );
}
