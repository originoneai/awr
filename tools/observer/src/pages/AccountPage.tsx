import { useMutation, useQueryClient } from '@tanstack/react-query';
import { App, Button, Card, Input, Space, Typography } from 'antd';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { apiPost, ApiError } from '../lib/api';
import { useRevocation } from '../lib/revocation';

/** Account and connection entries stay separate from business observation pages. */
export function AccountPage() {
  const { t } = useTranslation();
  const { message } = App.useApp();
  const queryClient = useQueryClient();
  const { revoke, resume } = useRevocation();
  const [bearer, setBearer] = useState('');

  function showError(error: unknown) {
    message.error(
      `${t('account.failed')}: ${error instanceof ApiError ? `${error.code} — ${error.message}` : String(error)}`,
    );
  }

  const login = useMutation({
    mutationFn: () => apiPost('/api/team/login', { bearer }),
    onSuccess: async () => {
      setBearer('');
      resume();
      await queryClient.invalidateQueries();
      message.success(t('account.done'));
    },
    onError: showError,
  });
  const logout = useMutation({
    mutationFn: () => apiPost('/api/team/logout', {}),
    onSuccess: () => {
      revoke();
      message.success(t('account.done'));
    },
    onError: showError,
  });
  const revokeConnections = useMutation({
    mutationFn: () => apiPost('/api/team/session/revoke', {}),
    onSuccess: () => {
      revoke();
      message.success(t('account.done'));
    },
    onError: showError,
  });

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
            loading={login.isPending}
            onClick={() => login.mutate()}
          >
            {t('account.loginSubmit')}
          </Button>
        </Space.Compact>
      </Card>
      <Card title={t('account.logout')}>
        <Button loading={logout.isPending} onClick={() => logout.mutate()}>
          {t('account.logoutSubmit')}
        </Button>
      </Card>
      <Card title={t('account.connect')}>
        <Button type="primary" href="/agent-connect">
          {t('account.connectSubmit')}
        </Button>
      </Card>
      <Card title={t('account.revoke')}>
        <Button
          danger
          loading={revokeConnections.isPending}
          onClick={() => revokeConnections.mutate()}
        >
          {t('account.revokeSubmit')}
        </Button>
      </Card>
    </Space>
  );
}
