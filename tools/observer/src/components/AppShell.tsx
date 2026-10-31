import {
  AuditOutlined,
  BellOutlined,
  BookOutlined,
  DashboardOutlined,
  DownOutlined,
  GlobalOutlined,
  InboxOutlined,
  KeyOutlined,
  ProjectOutlined,
  SearchOutlined,
  SettingOutlined,
  TeamOutlined,
  ToolOutlined,
} from '@ant-design/icons';
import { useQuery } from '@tanstack/react-query';
import {
  Avatar,
  Badge,
  Button,
  Dropdown,
  Input,
  Layout,
  Menu,
  Space,
  Typography,
  type MenuProps,
} from 'antd';
import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { useLocation, useNavigate } from 'react-router';
import i18n from '../i18n';
import { projectsQueryOptions } from '../lib/queries';
import { useScope } from '../lib/scope';

const { Header, Sider, Content } = Layout;

function selectedMenuKey(pathname: string): string {
  if (pathname.startsWith('/projects')) return '/projects';
  for (const key of ['/workbench', '/inbox', '/knowledge', '/members', '/activity', '/settings', '/account', '/advanced']) {
    if (pathname.startsWith(key)) return key;
  }
  return '/workbench';
}

export function AppShell({ children }: { children: ReactNode }) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const location = useLocation();
  const { projectKey, setProjectKey } = useScope();
  const projects = useQuery(projectsQueryOptions());

  const menuItems: MenuProps['items'] = [
    { key: '/workbench', icon: <DashboardOutlined />, label: t('nav.workbench') },
    { key: '/projects', icon: <ProjectOutlined />, label: t('nav.projects') },
    { key: '/inbox', icon: <InboxOutlined />, label: t('nav.inbox') },
    { key: '/knowledge', icon: <BookOutlined />, label: t('nav.knowledge') },
    { key: '/members', icon: <TeamOutlined />, label: t('nav.members') },
    { type: 'divider' },
    { key: '/activity', icon: <AuditOutlined />, label: t('nav.activity') },
    { key: '/settings', icon: <SettingOutlined />, label: t('nav.settings') },
    { key: '/account', icon: <KeyOutlined />, label: t('nav.account') },
  ];

  const projectItems = projects.data?.projects ?? [];
  const selectedProject = projectItems.find((p) => p.key === projectKey);
  const scopeMenu: MenuProps = {
    items: [
      { key: '__org__', label: t('shell.scope') },
      ...projectItems.map((p) => ({ key: p.key, label: `${p.title} (${p.key})` })),
    ],
    onClick: ({ key }) => setProjectKey(key === '__org__' ? null : key),
  };

  return (
    <Layout style={{ minHeight: '100vh' }}>
      <Header
        style={{
          position: 'sticky',
          top: 0,
          zIndex: 10,
          display: 'flex',
          alignItems: 'center',
          gap: 16,
          paddingInline: 16,
          borderBottom: '1px solid #f0f0f0',
        }}
      >
        <Typography.Text strong style={{ fontSize: 18, letterSpacing: 1, whiteSpace: 'nowrap', flexShrink: 0 }}>
          {t('app.wordmark')}
        </Typography.Text>
        <Typography.Text type="secondary" className="shell-subtitle" style={{ fontSize: 13, whiteSpace: 'nowrap' }}>
          {t('app.title')}
        </Typography.Text>
        <Dropdown menu={scopeMenu} trigger={['click']}>
          <Button>
            <Space size={4}>
              {selectedProject ? `${selectedProject.title} (${selectedProject.key})` : t('shell.scope')}
              <DownOutlined />
            </Space>
          </Button>
        </Dropdown>
        <Input
          placeholder={t('shell.searchPlaceholder')}
          prefix={<SearchOutlined />}
          allowClear
          style={{ maxWidth: 260, flex: '0 1 260px' }}
          className="shell-search"
        />
        <div style={{ flex: 1 }} />
        <Dropdown
          menu={{
            items: [
              { key: 'zh-CN', label: '中文' },
              { key: 'en-US', label: 'English' },
            ],
            selectedKeys: [i18n.language],
            onClick: ({ key }) => void i18n.changeLanguage(key),
          }}
          trigger={['click']}
        >
          <Button data-testid="language-switcher" icon={<GlobalOutlined />} aria-label={t('shell.language')}>
            {i18n.language === 'zh-CN' ? '中文' : 'EN'}
          </Button>
        </Dropdown>
        <Badge dot>
          <Button
            type="text"
            size="small"
            aria-label={t('shell.notifications')}
            icon={<BellOutlined style={{ fontSize: 16 }} />}
          />
        </Badge>
        <Space size={8}>
          <Button type="text" size="small" aria-label={t('shell.role')} style={{ height: 'auto', padding: 2 }}>
            <Avatar size="small">{t('shell.role').slice(0, 1)}</Avatar>
          </Button>
          <Typography.Text type="secondary" className="shell-role" style={{ fontSize: 13 }}>
            {t('shell.role')}
          </Typography.Text>
        </Space>
      </Header>
      <Layout>
        <Sider
          width={220}
          breakpoint="lg"
          collapsedWidth={0}
          style={{ borderRight: '1px solid #f0f0f0' }}
        >
          <div style={{ display: 'flex', flexDirection: 'column', height: '100%' }}>
            <Menu
              mode="inline"
              items={menuItems}
              selectedKeys={[selectedMenuKey(location.pathname)]}
              onClick={({ key }) => navigate(key)}
              style={{ flex: 1, borderInlineEnd: 'none', paddingTop: 8 }}
            />
            <Menu
              mode="inline"
              selectable={false}
              items={[
                {
                  key: '/advanced',
                  icon: <ToolOutlined />,
                  label: <Typography.Text type="secondary">{t('nav.advanced')}</Typography.Text>,
                },
              ]}
              onClick={({ key }) => navigate(key)}
              style={{ borderInlineEnd: 'none' }}
            />
          </div>
        </Sider>
        <Content style={{ padding: 24, minWidth: 0 }}>{children}</Content>
      </Layout>
    </Layout>
  );
}
