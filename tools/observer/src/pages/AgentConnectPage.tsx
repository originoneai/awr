import { useQuery } from '@tanstack/react-query';
import { Button, Card, Descriptions, Space, Tabs, Typography } from 'antd';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { ObservationForbidden } from '../components/states';
import { overviewQueryOptions, projectsQueryOptions } from '../lib/queries';
import { useRevocation } from '../lib/revocation';
import { useScope } from '../lib/scope';

/**
 * The connect-your-agent page (prototype 12), verified sources only:
 * the service address from the overview's mcp_url, the identity's effective
 * permissions, and an explicit "independent revocation is unavailable" note.
 * No bearer or credential value is ever rendered.
 */
export function AgentConnectPage() {
  const { t } = useTranslation();
  const { projectKey } = useScope();
  const { revoked } = useRevocation();
  const projects = useQuery(projectsQueryOptions(revoked));
  const project = projectKey ?? projects.data?.projects?.[0]?.key ?? null;
  const overview = useQuery(overviewQueryOptions(project, revoked));
  const identity = overview.data?.identity;
  const mcpUrl = overview.data?.mcp_url;
  const [copied, setCopied] = useState(false);

  if (revoked) return <ObservationForbidden />;

  return (
    <Space direction="vertical" size={16} style={{ width: '100%' }} data-testid="page-agent-connect">
      <Typography.Title level={4} style={{ margin: 0 }}>
        {t('nav.agentConnect')}
      </Typography.Title>
      <Tabs
        items={[
          {
            key: 'mine',
            label: t('agentConnect.myConnections'),
            children: (
              <Space direction="vertical" size={16} style={{ width: '100%' }}>
                <Card title={t('agentConnect.instructions')} data-testid="connect-instructions">
                  <Space direction="vertical" size={12}>
                    <Typography.Text strong>{t('agentConnect.serviceAddress')}</Typography.Text>
                    <Typography.Text code>{mcpUrl ?? t('freshness.state.not_measured')}</Typography.Text>
                    <Space>
                      <Button size="small" onClick={async () => {
                        if (!mcpUrl) return;
                        try {
                          await navigator.clipboard.writeText(mcpUrl);
                          setCopied(true);
                        } catch {
                          setCopied(false);
                        }
                      }}>
                        {copied ? t('account.copied') : t('account.copy')}
                      </Button>
                      <Button size="small" onClick={async () => {
                        if (!mcpUrl) return;
                        // The full instructions contain the address only,
                        // never a credential value (AWR-OBS-008).
                        await navigator.clipboard.writeText(`MCP: ${mcpUrl}`);
                      }}>
                        {t('agentConnect.copyInstructions')}
                      </Button>
                    </Space>
                    <Typography.Text type="secondary" data-testid="credential-row">
                      {t('agentConnect.credentialRow')}
                    </Typography.Text>
                  </Space>
                </Card>

                <Card title={t('agentConnect.capabilities')} data-testid="connect-capabilities">
                  <Descriptions
                    size="small"
                    column={1}
                    items={[
                      { key: 'actor', label: t('account.actorId'), children: identity?.actor_id ?? t('freshness.state.not_measured') },
                      { key: 'client', label: t('account.clientId'), children: identity?.client_id ?? t('freshness.state.not_measured') },
                      { key: 'role', label: t('account.role'), children: identity?.role ?? t('freshness.state.not_measured') },
                      { key: 'project', label: t('agentConnect.project'), children: project ?? t('freshness.state.not_measured') },
                      {
                        key: 'roles',
                        label: t('agentConnect.businessRoles'),
                        children: identity?.business_roles?.join(', ') ?? t('freshness.state.not_measured'),
                      },
                      {
                        key: 'ceiling',
                        label: t('agentConnect.actionCeiling'),
                        children: identity?.membership_action_ceiling?.join(', ') ?? t('freshness.state.not_measured'),
                      },
                      {
                        key: 'manageMembers',
                        label: t('agentConnect.canManageMembers'),
                        children: identity?.can_manage_members == null
                          ? t('freshness.state.not_measured')
                          : String(identity.can_manage_members),
                      },
                      {
                        key: 'readAudit',
                        label: t('agentConnect.canReadProjectAudit'),
                        children: identity?.can_read_project_audit == null
                          ? t('freshness.state.not_measured')
                          : String(identity.can_read_project_audit),
                      },
                    ]}
                  />
                </Card>

                <Card title={t('agentConnect.existing')} data-testid="connect-existing">
                  <Typography.Text type="secondary">{t('agentConnect.existingLater')}</Typography.Text>
                </Card>
                <Typography.Text type="secondary">{t('account.independentUnavailable')}</Typography.Text>
              </Space>
            ),
          },
          {
            key: 'records',
            label: t('agentConnect.records'),
            children: <Typography.Text type="secondary">{t('agentConnect.recordsLater')}</Typography.Text>,
          },
        ]}
      />
    </Space>
  );
}
