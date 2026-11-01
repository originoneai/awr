import { Card, Typography } from 'antd';
import { useTranslation } from 'react-i18next';
import { ObservationEmpty } from '../components/states';

export type PlaceholderPageKey = 'knowledge' | 'members' | 'activity' | 'settings' | 'advanced' | 'agentConnect';

/** Lightweight placeholder for nav destinations whose slice has not shipped. */
export function PlaceholderPage({ pageKey }: { pageKey: PlaceholderPageKey }) {
  const { t } = useTranslation();
  return (
    <Card data-testid={`page-${pageKey}`}>
      <Typography.Title level={5} style={{ marginTop: 0 }}>
        {t(`nav.${pageKey}`)}
      </Typography.Title>
      <Typography.Paragraph type="secondary">
        {t('state.placeholder.title')} — {t('state.placeholder.description')}
      </Typography.Paragraph>
      <ObservationEmpty />
    </Card>
  );
}
