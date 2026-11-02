import { useQuery } from '@tanstack/react-query';
import {
  Avatar,
  Button,
  Dropdown,
  Input,
  Layout,
  Menu,
  Tooltip,
  theme,
  type MenuProps,
} from 'antd';
import {
  Activity,
  Bell,
  BookOpen,
  ChevronDown,
  Folder,
  KeyRound,
  Languages,
  LayoutDashboard,
  Search,
  Settings,
  SquareCheckBig,
  Users,
  Wrench,
  type LucideIcon,
} from 'lucide-react';
import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { matchPath, NavLink, useLocation } from 'react-router';
import i18n from '../i18n';
import { projectsQueryOptions } from '../lib/queries';
import { useRevocation } from '../lib/revocation';
import { useScope } from '../lib/scope';

const { Header, Sider, Content } = Layout;

const NAVIGATION = [
  { path: '/workbench', icon: LayoutDashboard, label: 'workbench' },
  { path: '/projects', icon: Folder, label: 'projects' },
  { path: '/inbox', icon: SquareCheckBig, label: 'inbox' },
  { path: '/knowledge', icon: BookOpen, label: 'knowledge' },
  { path: '/members', icon: Users, label: 'members' },
  { path: '/activity', icon: Activity, label: 'activity' },
  { path: '/settings', icon: Settings, label: 'settings' },
  { path: '/account', icon: KeyRound, label: 'account' },
] as const;

function selectedNavigation(pathname: string) {
  if (matchPath({ path: '/agent-connect', end: true }, pathname)) return '/members';
  return NAVIGATION.find(({ path }) => matchPath({ path: `${path}/*`, end: true }, pathname))?.path
    ?? '/workbench';
}

function NavigationIcon({ icon: Icon }: { icon: LucideIcon }) {
  return <Icon size={16} strokeWidth={1.5} aria-hidden />;
}

export function AppShell({ children }: { children: ReactNode }) {
  const { t } = useTranslation();
  const location = useLocation();
  const { token } = theme.useToken();
  const { projectKey, setProjectKey } = useScope();
  const { revoked } = useRevocation();
  const projects = useQuery(projectsQueryOptions(revoked));
  const selectedKey = selectedNavigation(location.pathname);

  const menuItems: MenuProps['items'] = [];
  for (const [index, item] of NAVIGATION.entries()) {
    if (index === 5) menuItems.push({ type: 'divider' });
    menuItems.push({
      key: item.path,
      icon: <NavigationIcon icon={item.icon} />,
      label: <NavLink to={item.path}>{t(`nav.${item.label}`)}</NavLink>,
    });
  }

  const projectItems = revoked ? [] : (projects.data?.projects ?? []);
  const selectedProject = projectItems.find((project) => project.key === projectKey);
  const scopeLabel = selectedProject
    ? `${selectedProject.title} (${selectedProject.key})`
    : (!revoked && projectKey) || t('shell.scope');
  const scopeMenu: MenuProps = {
    items: [
      { key: '__org__', label: t('shell.scope') },
      ...projectItems.map((project) => ({ key: project.key, label: `${project.title} (${project.key})` })),
    ],
    selectedKeys: [projectKey ?? '__org__'],
    onClick: ({ key }) => setProjectKey(key === '__org__' ? null : key),
  };

  return (
    <Layout hasSider style={{ minHeight: '100vh' }}>
      <Sider
        width={240}
        breakpoint="lg"
        collapsedWidth={0}
        className="shell-sider"
        style={{ borderRight: `1px solid ${token.colorSplit}` }}
      >
        <div className="shell-sider-inner">
          <div className="shell-brand" data-testid="awr-brand">
            <img src="/awr-placeholder-logo.png" alt={t('app.wordmark')} />
          </div>
          <Menu
            mode="inline"
            className="shell-nav"
            items={menuItems}
            selectedKeys={[selectedKey]}
            style={{ flex: 1, borderInlineEnd: 'none', paddingTop: 8 }}
          />
          <Menu
            mode="inline"
            className="shell-nav"
            selectable={false}
            items={[{
              key: '/advanced',
              icon: <Wrench size={16} strokeWidth={1.5} aria-hidden />,
              label: <NavLink to="/advanced">{t('nav.advanced')}</NavLink>,
            }]}
            style={{ borderInlineEnd: 'none' }}
          />
        </div>
      </Sider>
      <Layout>
        <Header
          className="shell-header"
          style={{
            position: 'sticky',
            top: 0,
            zIndex: 10,
            borderBottom: `1px solid ${token.colorSplit}`,
          }}
        >
          <Dropdown menu={scopeMenu} trigger={['click']}>
            <Button data-testid="scope-switcher" className="shell-scope">
              <span className="shell-scope-label">{scopeLabel}</span>
              <ChevronDown size={16} strokeWidth={1.5} aria-hidden />
            </Button>
          </Dropdown>
          <div className="shell-header-spacer" />
          <Tooltip title={t('shell.later')}>
            <Input
              placeholder={t('shell.searchPlaceholder')}
              prefix={<Search size={16} strokeWidth={1.5} aria-hidden />}
              disabled
              className="shell-search"
            />
          </Tooltip>
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
            <Button
              data-testid="language-switcher"
              icon={<Languages size={16} strokeWidth={1.5} aria-hidden />}
              aria-label={t('shell.language')}
            >
              <span className="shell-language-label">{i18n.language === 'zh-CN' ? '中文' : 'EN'}</span>
            </Button>
          </Dropdown>
          <Tooltip title={t('shell.later')}>
            <Button
              type="text"
              disabled
              aria-label={t('shell.notifications')}
              icon={<Bell size={16} strokeWidth={1.5} aria-hidden />}
            />
          </Tooltip>
          <Tooltip title={t('shell.later')}>
            <Button
              type="text"
              disabled
              className="shell-avatar-button"
              aria-label={t('shell.identity')}
            >
              <Avatar style={{ background: token.colorFillSecondary, color: token.colorPrimary }}>
                {(selectedProject?.title ?? t('app.wordmark')).slice(0, 1)}
              </Avatar>
            </Button>
          </Tooltip>
        </Header>
        <Content className="shell-content">{children}</Content>
      </Layout>
    </Layout>
  );
}
