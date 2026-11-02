import { useQuery } from '@tanstack/react-query';
import {
  Avatar,
  Button,
  Drawer,
  Dropdown,
  Grid,
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
  Menu as MenuIcon,
  Search,
  Settings,
  SquareCheckBig,
  Users,
  Wrench,
  X as CloseIcon,
  type LucideIcon,
} from 'lucide-react';
import { useEffect, useState, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { matchPath, NavLink, useLocation } from 'react-router';
import i18n from '../i18n';
import { projectsQueryOptions } from '../lib/queries';
import { useRevocation } from '../lib/revocation';
import { useScope } from '../lib/scope';
import { craftTokens } from '../theme';

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

function useShellMenuItems() {
  const { t } = useTranslation();
  const menuItems: MenuProps['items'] = [];
  for (const [index, item] of NAVIGATION.entries()) {
    if (index === 5) menuItems.push({ type: 'divider' });
    menuItems.push({
      key: item.path,
      icon: <NavigationIcon icon={item.icon} />,
      label: <NavLink to={item.path}>{t(`nav.${item.label}`)}</NavLink>,
    });
  }
  const advancedItems: MenuProps['items'] = [{
    key: '/advanced',
    icon: <NavigationIcon icon={Wrench} />,
    label: <NavLink to="/advanced">{t('nav.advanced')}</NavLink>,
  }];
  return { menuItems, advancedItems };
}

/** The single navigation source: the same menus render in the desktop sider and the mobile drawer. */
function ShellNavigationMenus({
  menuItems,
  advancedItems,
  selectedKey,
  onNavigate,
}: {
  menuItems: MenuProps['items'];
  advancedItems: MenuProps['items'];
  selectedKey: string;
  onNavigate?: MenuProps['onClick'];
}) {
  return (
    <>
      <Menu
        mode="inline"
        className="shell-nav"
        items={menuItems}
        selectedKeys={[selectedKey]}
        onClick={onNavigate}
        style={{ flex: 1, borderInlineEnd: 'none', paddingTop: 8 }}
      />
      <Menu
        mode="inline"
        className="shell-nav"
        selectable={false}
        items={advancedItems}
        onClick={onNavigate}
        style={{ borderInlineEnd: 'none' }}
      />
    </>
  );
}

export function AppShell({ children }: { children: ReactNode }) {
  const { t } = useTranslation();
  const location = useLocation();
  const { token } = theme.useToken();
  const { projectKey, setProjectKey } = useScope();
  const { revoked } = useRevocation();
  const projects = useQuery(projectsQueryOptions(revoked));
  const selectedKey = selectedNavigation(location.pathname);
  const { menuItems, advancedItems } = useShellMenuItems();

  // Below lg the sider is replaced by a drawer behind a header entry; until the media queries
  // resolve (and whenever matchMedia is unavailable) the desktop shell stays up.
  const wideNavigation = Grid.useBreakpoint().lg ?? true;
  const [navigationOpen, setNavigationOpen] = useState(false);

  // A drawer left open while the viewport grows would sit on top of the sider navigation:
  // growing to the desktop layout closes it, shrinking back never reopens it on its own.
  useEffect(() => {
    if (wideNavigation) setNavigationOpen(false);
  }, [wideNavigation]);

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

  const brand = (
    <div className="shell-brand" data-testid="awr-brand">
      <img src="/awr-placeholder-logo.png" alt={t('app.wordmark')} />
    </div>
  );

  return (
    <Layout hasSider style={{ minHeight: '100vh' }}>
      {wideNavigation && (
        <Sider
          width={240}
          className="shell-sider"
          style={{ borderRight: `1px solid ${token.colorSplit}` }}
        >
          <div className="shell-sider-inner">
            {brand}
            <ShellNavigationMenus
              menuItems={menuItems}
              advancedItems={advancedItems}
              selectedKey={selectedKey}
            />
          </div>
        </Sider>
      )}
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
          {!wideNavigation && (
            <Button
              type="text"
              className="shell-nav-toggle"
              data-testid="nav-toggle"
              aria-label={t('shell.openNav')}
              onClick={() => setNavigationOpen(true)}
              style={{ width: craftTokens.touchTarget, height: craftTokens.touchTarget, padding: 0 }}
              icon={<MenuIcon size={16} strokeWidth={1.5} aria-hidden />}
            />
          )}
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
      <Drawer
        className="shell-drawer"
        placement="left"
        width={craftTokens.siderWidth}
        open={navigationOpen}
        onClose={() => setNavigationOpen(false)}
        title={brand}
        closable={{
          closeIcon: <CloseIcon size={16} strokeWidth={1.5} aria-hidden />,
          'aria-label': t('shell.closeNav'),
        }}
        styles={{ body: { padding: 0 } }}
      >
        <ShellNavigationMenus
          menuItems={menuItems}
          advancedItems={advancedItems}
          selectedKey={selectedKey}
          onNavigate={() => setNavigationOpen(false)}
        />
      </Drawer>
    </Layout>
  );
}
