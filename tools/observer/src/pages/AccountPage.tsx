import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Alert, App, Button, Card, Col, Descriptions, Input, Row, Space, Typography } from 'antd';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Link } from 'react-router';
import { apiPost, ApiError } from '../lib/api';
import { formatUnixMs } from '../lib/format';
import { projectsQueryOptions } from '../lib/queries';
import { ObservationForbidden } from '../components/states';
import { useRevocation } from '../lib/revocation';


/** Read-only observation cards for the account area (AWR-OBS-008). */
function AccountObservationCards() {
  const { t, i18n } = useTranslation();
  const { revoked } = useRevocation();
  const projects = useQuery(projectsQueryOptions(revoked));
  const session = projects.data?.session;

  if (revoked) {
    // A revoked session clears the observation cards; the login entry below
    // stays available for signing in again (AWR-OBS-008).
    return <ObservationForbidden />;
  }
  return (
    <Row gutter={[16, 16]}>
      <Col xs={24} lg={10}>
        <Card title={t('account.sessionTitle')} data-testid="account-session-card">
          <Descriptions
            size="small"
            column={1}
            items={[
              {
                key: 'created',
                label: t('account.sessionCreated'),
                children: session?.created_at_ms != null
                  ? formatUnixMs(session.created_at_ms, i18n.language)
                  : t('common.notProvided'),
              },
              {
                key: 'expires',
                label: t('account.sessionExpires'),
                children: session?.expires_at_ms != null
                  ? formatUnixMs(session.expires_at_ms, i18n.language)
                  : t('common.notProvided'),
              },
              {
                key: 'lastSeen',
                label: t('account.sessionLastSeen'),
                children: session?.last_seen_ms != null
                  ? formatUnixMs(session.last_seen_ms, i18n.language)
                  : t('common.notProvided'),
              },
            ]}
          />
        </Card>
      </Col>
      <Col xs={24} lg={14}>
        <Card title={t('account.revokeAllMineTitle')} data-testid="account-revoke-all-mine">
          <Space direction="vertical" size={12}>
            <RevokeAllMineButton />
            <Alert
              type="info"
              showIcon
              data-testid="independent-revoke-unavailable"
              title={t('account.independentUnavailable')}
            />
            <Typography.Text type="secondary" data-testid="revocation-boundary">
              {t('account.boundary')}
            </Typography.Text>
          </Space>
        </Card>
      </Col>
      <Col span={24}>
        <Card title={t('account.resumeTitle')}>
          <Link to="/workbench">{t('account.resumeEntry')}</Link>
          <Typography.Paragraph type="secondary" style={{ marginTop: 8, marginBottom: 0 }}>
            {t('account.adminNote')}
          </Typography.Paragraph>
        </Card>
      </Col>
    </Row>
  );
}

function RevokeAllMineButton() {
  const { t } = useTranslation();
  const { message } = App.useApp();
  const { revoke } = useRevocation();
  const mutation = useMutation({
    mutationFn: () => apiPost('/api/team/session/revoke', { all_mine: true }),
    onSuccess: () => {
      revoke();
      message.success(t('account.done'));
    },
    onError: (error: unknown) => {
      message.error(`${t('account.failed')}: ${error instanceof ApiError ? `${error.code} — ${error.message}` : String(error)}`);
    },
  });
  return (
    <Button danger loading={mutation.isPending} onClick={() => mutation.mutate()}>
      {t('account.revokeAllMine')}
    </Button>
  );
}

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

  return (
    <Space direction="vertical" size={16} style={{ width: '100%' }} data-testid="page-account">
      <Typography.Title level={4} style={{ margin: 0 }}>
        {t('account.title')}
      </Typography.Title>
      <AccountObservationCards />
      <Row gutter={[16, 16]}>
        <Col xs={24} lg={14}>
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
        </Col>
        <Col xs={24} lg={10}>
          <Space direction="vertical" size={16} style={{ width: '100%' }}>
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
          </Space>
        </Col>
      </Row>

    </Space>
  );
}
