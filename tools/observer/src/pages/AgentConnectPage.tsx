import { useQuery } from '@tanstack/react-query';
import { Button, Card, Col, Descriptions, Row, Space, Tabs, Typography } from 'antd';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { ObservationForbidden } from '../components/states';
import { ObservationValue } from '../components/ObservationValue';
import { TechnicalDetails } from '../components/TechnicalDetails';
import { actionWords } from '../core/observation';
import { humanText } from '../i18n/human-language';
import { connectionTechnicalItems } from '../lib/connection-technical';
import { overviewQueryOptions, projectsQueryOptions } from '../lib/queries';
import { useRevocation } from '../lib/revocation';
import { useScope } from '../lib/scope';

function humanOther(count: number, language: string): string {
  return humanText('actionsOther', language).replace('{{count}}', String(count));
}

/**
 * The connect-your-agent page (prototype 12), verified sources only:
 * the service address from the overview's mcp_url, the identity's effective
 * permissions, and an explicit "independent revocation is unavailable" note.
 * No bearer or credential value is ever rendered.
 */
export function AgentConnectPage() {
  const { t, i18n } = useTranslation();
  const { projectKey } = useScope();
  const { revoked } = useRevocation();
  const projects = useQuery(projectsQueryOptions(revoked));
  const project = projectKey ?? projects.data?.projects?.[0]?.key ?? null;
  const overview = useQuery(overviewQueryOptions(project, revoked));
  const identity = overview.data?.identity;
  const mcpUrl = overview.data?.mcp_url;
  const [copied, setCopied] = useState(false);
  const projectTitle = projects.data?.projects?.find((entry) => entry.key === project)?.title;

  // What this connection may do, in words: the service's own list, plus the two things it states as flags.
  const ceiling = identity?.membership_action_ceiling;
  const allowedNames = new Set<string>(Array.isArray(ceiling) ? ceiling.filter((name): name is string => typeof name === 'string') : []);
  if (identity?.can_manage_members === true) allowedNames.add('access.manage_project');
  if (identity?.can_read_project_audit === true) allowedNames.add('audit.read_project');
  const known = Array.isArray(ceiling) || identity?.can_manage_members === true || identity?.can_read_project_audit === true;
  const allowed = actionWords([...allowedNames], { language: i18n.language });
  const notIncluded = actionWords(
    known ? ['planning.approve', 'review.decide', 'access.manage_project', 'audit.read_project'].filter((name) => !allowedNames.has(name)) : [],
    { language: i18n.language },
  );
  const allowedText = !known
    ? t('common.notProvided')
    : [...allowed.shown, ...(allowed.other > 0 ? [humanOther(allowed.other, i18n.language)] : [])].join(t('common.listSeparator')) || t('agentConnect.noActions');

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
              <Row gutter={[16, 16]}>
                <Col xs={24} lg={14}>
                <Card title={t('agentConnect.instructions')} data-testid="connect-instructions">
                  <Space direction="vertical" size={12}>
                    <Typography.Text strong>{t('agentConnect.serviceAddress')}</Typography.Text>
                    <Typography.Text code>{mcpUrl ?? t('common.notProvided')}</Typography.Text>
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
                        try {
                          // The full instructions contain the address only,
                          // never a credential value (AWR-OBS-008).
                          await navigator.clipboard.writeText(`MCP: ${mcpUrl}`);
                        } catch {
                          // Clipboard can be denied; do not leave an unhandled rejection.
                        }
                      }}>
                        {t('agentConnect.copyInstructions')}
                      </Button>
                    </Space>
                    <Typography.Text type="secondary" data-testid="credential-row">
                      {t('agentConnect.credentialRow')}
                    </Typography.Text>
                  </Space>
                </Card>
                </Col>
                <Col xs={24} lg={10}>
                <Card title={t('agentConnect.capabilities')} data-testid="connect-capabilities">
                  <Descriptions
                    size="small"
                    column={1}
                    items={[
                      { key: 'project', label: t('agentConnect.project'), children: projectTitle ?? <ObservationValue value={project} /> },
                      { key: 'role', label: t('account.role'), children: <ObservationValue value={identity?.role} field="role" /> },
                      {
                        key: 'roles',
                        label: t('agentConnect.businessRoles'),
                        children: <ObservationValue value={identity?.business_roles} field="business_roles" />,
                      },
                      { key: 'allowed', label: t('agentConnect.allowedActions'), children: <span data-testid="connect-allowed">{allowedText}</span> },
                      ...(notIncluded.shown.length > 0
                        ? [{ key: 'missing', label: t('agentConnect.notIncluded'), children: <span data-testid="connect-not-included">{notIncluded.shown.join(t('common.listSeparator'))}</span> }]
                        : []),
                    ]}
                  />
                  <TechnicalDetails items={connectionTechnicalItems(identity, project)} />
                </Card>
                </Col>
                <Col span={24}>
                <Card title={t('agentConnect.existing')} data-testid="connect-existing">
                  <Typography.Text type="secondary">{t('agentConnect.existingLater')}</Typography.Text>
                </Card>
                </Col>
                <Col span={24}>
                  <Typography.Text type="secondary" data-testid="connect-revocation-note">{t('agentConnect.revocationNote')}</Typography.Text>
                </Col>
              </Row>
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
