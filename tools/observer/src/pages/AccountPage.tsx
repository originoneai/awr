import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Alert, App, Button, Card, Descriptions, Input, Space, Typography } from 'antd';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Link } from 'react-router';
import { apiPost, ApiError } from '../lib/api';
import { formatUnixMs } from '../lib/format';
import { overviewQueryOptions, projectsQueryOptions, workSnapshotQueryOptions } from '../lib/queries';
import { ObservationForbidden } from '../components/states';
import { useRevocation } from '../lib/revocation';
import { useScope } from '../lib/scope';


/** Read-only observation cards for the account area (AWR-OBS-008). */
function AccountObservationCards() {
  const { t, i18n } = useTranslation();
  const { projectKey } = useScope();
  const { revoked } = useRevocation();
  const projects = useQuery(projectsQueryOptions(revoked));
  const project = projectKey ?? projects.data?.projects?.[0]?.key ?? null;
  const overview = useQuery(overviewQueryOptions(project, revoked));
  const firstWork = overview.data?.works?.[0];
  const snapshot = useQuery(workSnapshotQueryOptions(
    firstWork ? project : null,
    firstWork?.key ?? '',
    firstWork?.workstream_id ?? '',
    revoked,
  ));
  const identity = overview.data?.identity;
  const session = projects.data?.session;
  const mcpUrl = overview.data?.mcp_url;
  const actorName = snapshot.data?.result.data.observation?.client?.actor_name;
  const [copied, setCopied] = useState(false);

  if (revoked) {
    // A revoked session clears the observation cards; the login entry below
    // stays available for signing in again (AWR-OBS-008).
    return <ObservationForbidden />;
  }
  return (
    <>
      <Card title={t('account.identityTitle')} data-testid="account-identity-card">
        <Descriptions
          size="small"
          column={1}
          items={[
            { key: 'actor', label: t('account.actorId'), children: identity?.actor_id ?? t('freshness.state.not_measured') },
            { key: 'client', label: t('account.clientId'), children: identity?.client_id ?? t('freshness.state.not_measured') },
            { key: 'role', label: t('account.role'), children: identity?.role ?? t('freshness.state.not_measured') },
          ]}
        />
        <Typography.Text type="secondary" data-testid="self-reported-agent-name">
          {t('account.selfReportedName')}: {actorName ?? t('freshness.state.not_measured')}
          {' '}({t('account.selfReportedNote')})
        </Typography.Text>
      </Card>

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
                : t('freshness.state.not_measured'),
            },
            {
              key: 'expires',
              label: t('account.sessionExpires'),
              children: session?.expires_at_ms != null
                ? formatUnixMs(session.expires_at_ms, i18n.language)
                : t('freshness.state.not_measured'),
            },
            {
              key: 'lastSeen',
              label: t('account.sessionLastSeen'),
              children: session?.last_seen_ms != null
                ? formatUnixMs(session.last_seen_ms, i18n.language)
                : t('freshness.state.not_measured'),
            },
          ]}
        />
      </Card>

      <Card title={t('account.connectGuide')} data-testid="account-connect-guide">
        <Space direction="vertical" size={8}>
          <Typography.Text code>{mcpUrl ?? t('freshness.state.not_measured')}</Typography.Text>
          {mcpUrl ? (
            <Button size="small" onClick={async () => {
              try {
                await navigator.clipboard.writeText(mcpUrl);
                setCopied(true);
              } catch {
                setCopied(false);
              }
            }}>
              {copied ? t('account.copied') : t('account.copy')}
            </Button>
          ) : null}
          <Typography.Text type="secondary">{t('account.noBearerNote')}</Typography.Text>
        </Space>
      </Card>

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

      <Card title={t('account.resumeTitle')}>
        <Link to="/workbench">{t('account.resumeEntry')}</Link>
        <Typography.Paragraph type="secondary" style={{ marginTop: 8, marginBottom: 0 }}>
          {t('account.adminNote')}
        </Typography.Paragraph>
      </Card>
    </>
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
      <AccountObservationCards />
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
