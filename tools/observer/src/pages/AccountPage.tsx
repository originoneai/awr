import { App, Button, Card, Input, Space, Typography } from 'antd';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { apiPost, ApiError } from '../lib/api';
import { useScope } from '../lib/scope';

/**
 * Account / connection entries — deliberately separate from every business
 * page. Entries only: sign in, sign out, connect my agent, revoke connection.
 */
export function AccountPage() {
  const { t } = useTranslation();
  const { message } = App.useApp();
  const { projectKey } = useScope();
  const [bearer, setBearer] = useState('');
  const [pending, setPending] = useState<string | null>(null);

  async function run(action: string, fn: () => Promise<unknown>) {
    setPending(action);
    try {
      await fn();
      message.success(t('account.done'));
    } catch (error) {
      message.error(
        `${t('account.failed')}: ${error instanceof ApiError ? `${error.code} — ${error.message}` : String(error)}`,
      );
    } finally {
      setPending(null);
    }
  }

  return (
    <Space direction="vertical" size={16} style={{ width: '100%' }} data-testid="page-account">
      <Typography.Title level={4} style={{ margin: 0 }}>
        {t('account.title')}
      </Typography.Title>
      <Card title={t('account.login')}>
        <Space.Compact style={{ maxWidth: 420, width: '100%' }}>
          <Input.Password
            placeholder={t('account.loginBearer')}
            value={bearer}
            onChange={(e) => setBearer(e.target.value)}
            aria-label={t('account.loginBearer')}
          />
          <Button
            type="primary"
            loading={pending === 'login'}
            onClick={() => run('login', () => apiPost('/api/team/login', { bearer }))}
          >
            {t('account.loginSubmit')}
          </Button>
        </Space.Compact>
      </Card>
      <Card title={t('account.logout')}>
        <Button
          loading={pending === 'logout'}
          onClick={() => run('logout', () => apiPost('/api/team/logout', {}))}
        >
          {t('account.logoutSubmit')}
        </Button>
      </Card>
      <Card title={t('account.connect')}>
        <Button
          loading={pending === 'connect'}
          onClick={() =>
            run('connect', () =>
              apiPost('/api/team/access', {
                project: projectKey ?? 'demo',
                operation: 'inspect',
                payload: {},
              }),
            )
          }
        >
          {t('account.connectSubmit')}
        </Button>
      </Card>
      <Card title={t('account.revoke')}>
        <Button
          danger
          loading={pending === 'revoke'}
          onClick={() => run('revoke', () => apiPost('/api/team/session/revoke', {}))}
        >
          {t('account.revokeSubmit')}
        </Button>
      </Card>
    </Space>
  );
}
