import { useQuery } from '@tanstack/react-query';
import {
  Avatar,
  Badge,
  Breadcrumb,
  Button,
  Dropdown,
  Layout,
  Menu,
  theme,
  type MenuProps,
} from 'antd';
import {
  Activity,
  Bell,
  BookOpen,
  Building2,
  ChevronDown,
  ChevronRight,
  Folder,
  LayoutDashboard,
  Moon,
  Search,
  Settings,
  SquareCheckBig,
  Sun,
  Users,
  type LucideIcon,
} from 'lucide-react';
import { useCallback, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { Link, matchPath, NavLink, useLocation, useNavigate } from 'react-router';
import { readsAsName } from '../core/observation/owner';
import { PerspectiveProvider } from '../lib/perspective/PerspectiveProvider';
import { PerspectiveSwitcher } from '../lib/perspective/PerspectiveSwitcher';
import { unknownPerspectiveFacts, type PerspectiveFacts, type PerspectiveId } from '../lib/perspective/types';
import { projectLabel } from '../lib/project-label';
import { ordinaryDecisionsQueryOptions, overviewQueryOptions, projectsQueryOptions } from '../lib/queries';
import { useRevocation } from '../lib/revocation';
import { useScope } from '../lib/scope';
import type { TeamOverviewResponse } from '../lib/types';
import { craftTokens } from '../theme';
import { useCraftTheme } from './CraftThemeProvider';
import { SearchPanel } from './SearchPanel';
import { ThemeToggle } from './ThemeToggle';
import { useCompactViewport, useMediaQuery } from '../lib/viewport';

const { Header, Sider, Content } = Layout;
const COMPACT_SEARCH_QUERY = '(max-width: 1240px)';

const PRIMARY_NAVIGATION = [
  { path: '/workbench', icon: LayoutDashboard, label: 'workbench' },
  { path: '/projects', icon: Folder, label: 'projects' },
  { path: '/inbox', icon: SquareCheckBig, label: 'inbox' },
  { path: '/lessons', icon: BookOpen, label: 'lessons' },
] as const;

const ADMIN_NAVIGATION = [
  { path: '/members', icon: Users, label: 'members' },
  { path: '/activity', icon: Activity, label: 'activity' },
  { path: '/settings', icon: Settings, label: 'settings' },
] as const;

const NAVIGATION = [...PRIMARY_NAVIGATION, ...ADMIN_NAVIGATION];
const PROJECT_DETAIL_PATHS = ['/tasks/*', '/goals/*', '/documents/*', '/updates/*', '/takeover/*'];

type NavigationPath = (typeof NAVIGATION)[number]['path'];

interface Crumb {
  label: string;
  to?: string;
}

function selectedNavigation(pathname: string): NavigationPath {
  if (matchPath({ path: '/members/*', end: true }, pathname)
    || matchPath({ path: '/account', end: true }, pathname)
    || matchPath({ path: '/agent-connect', end: true }, pathname)) return '/members';
  if (PROJECT_DETAIL_PATHS.some((path) => matchPath({ path, end: true }, pathname))) return '/projects';
  return NAVIGATION.find(({ path }) => matchPath({ path: `${path}/*`, end: true }, pathname))?.path
    ?? '/workbench';
}

function NavigationIcon({ icon: Icon }: { icon: LucideIcon }) {
  return <Icon size={16} strokeWidth={1.5} aria-hidden />;
}

function currentMemberName(overview: TeamOverviewResponse | undefined): string | null {
  if (!overview) return null;
  const identity = overview.identity as unknown as Record<string, unknown> | undefined;
  const identityName = identity?.display_name;
  if (typeof identityName === 'string' && readsAsName(identityName)) return identityName.trim();
  const actorId = identity?.actor_id;
  if (typeof actorId !== 'string') return null;
  for (const member of overview.members ?? []) {
    const matches = ['actor_id', 'person_id', 'id'].some((key) => member[key] === actorId);
    const name = member.display_name ?? member.name;
    if (matches && typeof name === 'string' && readsAsName(name)) return name.trim();
  }
  return null;
}

function nameInitial(name: string) {
  return Array.from(name.trim())[0]?.toLocaleUpperCase() ?? '';
}

function ShellNavigation({ selectedKey, pendingLabel }: { selectedKey: NavigationPath; pendingLabel: string | null }) {
  const { t } = useTranslation();
  const { projectKey } = useScope();
  const scopeSearch = projectKey ? `?scope=${encodeURIComponent(projectKey)}` : '';
  const items = (navigation: typeof PRIMARY_NAVIGATION | typeof ADMIN_NAVIGATION): MenuProps['items'] => (
    navigation.map((item) => ({
      key: item.path,
      icon: <NavigationIcon icon={item.icon} />,
      style: { paddingInline: 12 },
      label: (
        <NavLink
          to={`${item.path}${scopeSearch}`}
          style={{ display: 'flex', alignItems: 'center', gap: 8, minWidth: 0 }}
        >
          <span style={{ flex: 1, minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis' }}>
            {t(`nav.${item.label}`)}
          </span>
          {item.path === '/inbox' && pendingLabel ? (
            <span
              data-testid="inbox-count"
              style={{
                flex: '0 0 auto',
                minWidth: 20,
                height: 20,
                marginLeft: 'auto',
                padding: '0 6px',
                borderRadius: 10,
                background: 'var(--craft-color-accent)',
                color: 'var(--craft-color-on-accent)',
                fontFamily: craftTokens.font.meta,
                fontSize: 12,
                fontVariantNumeric: 'tabular-nums',
                lineHeight: '20px',
                textAlign: 'center',
              }}
            >
              {pendingLabel}
            </span>
          ) : null}
        </NavLink>
      ),
    }))
  );
  const menuStyle = { display: 'grid', gap: 2, borderInlineEnd: 'none', padding: 0 } as const;

  return (
    <div style={{ flex: 1, minHeight: 0 }}>
      <nav aria-label={t('shell.mainNavigation')}>
        <Menu
          mode="inline"
          className="shell-nav"
          items={items(PRIMARY_NAVIGATION)}
          selectedKeys={[selectedKey]}
          style={menuStyle}
        />
      </nav>
      <div
        className="shell-side-label"
        style={{ padding: '24px 12px 8px', color: 'var(--craft-color-muted)', fontSize: 12, lineHeight: '16px' }}
      >
        {t('nav.groupAdmin')}
      </div>
      <nav aria-label={t('nav.groupAdmin')}>
        <Menu
          mode="inline"
          className="shell-nav"
          items={items(ADMIN_NAVIGATION)}
          selectedKeys={[selectedKey]}
          style={menuStyle}
        />
      </nav>
    </div>
  );
}

export function AppShell({ children, perspectiveFacts: suppliedPerspectiveFacts }: {
  children: ReactNode;
  /** Already-read facts supplied by the composition layer; this task adds no member-directory read. */
  perspectiveFacts?: PerspectiveFacts;
}) {
  const { t, i18n } = useTranslation();
  const location = useLocation();
  const navigate = useNavigate();
  const { token } = theme.useToken();
  const { mode, toggle } = useCraftTheme();
  const { projectKey, setProjectKey } = useScope();
  const { revoked } = useRevocation();
  const projects = useQuery(projectsQueryOptions(revoked));
  // Shell indicators subscribe to reads the open page already owns; they never add another source.
  const overview = useQuery({ ...overviewQueryOptions(projectKey, revoked), enabled: false });
  const pending = useQuery({ ...ordinaryDecisionsQueryOptions(projectKey ?? '', 'open', null, revoked), enabled: false });
  const compactNavigation = useCompactViewport();
  const compactSearch = useMediaQuery(COMPACT_SEARCH_QUERY);
  const selectedKey = selectedNavigation(location.pathname);

  const projectItems = revoked ? [] : (projects.data?.projects ?? []);
  const selectedProject = projectItems.find((project) => project.key === projectKey);
  const selectedProjectIndex = selectedProject ? projectItems.indexOf(selectedProject) + 1 : 1;
  const selectedProjectLabel = selectedProject
    ? projectLabel(selectedProject, selectedProjectIndex)
    : t('shell.currentProject');
  const scopeLabel = selectedProject ? selectedProjectLabel : t('shell.scope');
  const scopeSearch = projectKey ? `?scope=${encodeURIComponent(projectKey)}` : '';
  const inboxPath = `/inbox${scopeSearch}`;
  const scopeMenu: MenuProps = {
    items: [
      { key: '__org__', label: t('shell.scope') },
      ...projectItems.map((project, index) => ({ key: project.key, label: projectLabel(project, index + 1) })),
    ],
    selectedKeys: [projectKey ?? '__org__'],
    onClick: ({ key }) => setProjectKey(key === '__org__' ? null : key),
  };
  const accountMenu: MenuProps = {
    items: [
      { key: 'account', label: <Link to={`/account${scopeSearch}`}>{t('nav.account')}</Link> },
      { key: 'agent-connect', label: <Link to={`/agent-connect${scopeSearch}`}>{t('nav.agentConnect')}</Link> },
      { type: 'divider' },
      { key: 'zh-CN', label: '中文' },
      { key: 'en-US', label: 'English' },
    ],
    selectedKeys: [i18n.language],
    onClick: ({ key }) => {
      if (key === 'zh-CN' || key === 'en-US') void i18n.changeLanguage(key);
    },
  };

  const pendingData = pending.data?.result.data;
  const pendingCount = pendingData?.items.length ?? null;
  const pendingLabel = pendingData && pendingCount !== null && pendingCount > 0
    ? `${pendingCount}${pendingData.truncated || pendingData.next_cursor !== null ? '+' : ''}`
    : null;
  const memberName = currentMemberName(overview.data) ?? t('shell.you');
  const MobileThemeIcon = mode === 'dark' ? Sun : Moon;
  const mobileThemeLabel = t(mode === 'dark' ? 'shell.switchToLight' : 'shell.switchToDark');
  const perspectiveFacts = revoked
    ? unknownPerspectiveFacts()
    : suppliedPerspectiveFacts ?? unknownPerspectiveFacts(pendingCount);
  const requestedPerspective = new URLSearchParams(location.search).get('perspective');
  const changePerspective = useCallback((next: PerspectiveId) => {
    const params = new URLSearchParams(location.search);
    params.set('perspective', next);
    navigate({ pathname: location.pathname, search: params.toString() }, { replace: true });
  }, [location.pathname, location.search, navigate]);

  const brand = (mobile = false) => (
    <div
      className={mobile ? 'shell-brand shell-brand-mobile' : 'shell-brand'}
      data-testid="awr-brand"
    >
      <img
        className="shell-brand-mark shell-brand-mark-color"
        src="/awr-logo-color-72.png"
        srcSet="/awr-logo-color-72.png 1x, /awr-logo-color-144.png 2x, /awr-logo-color-288.png 4x"
        width={craftTokens.brand.markWidth}
        height={craftTokens.brand.markHeight}
        alt=""
        aria-hidden
        data-testid="brand-mark-color"
      />
      <img
        className="shell-brand-mark shell-brand-mark-white"
        src="/awr-logo-white-72.png"
        srcSet="/awr-logo-white-72.png 1x, /awr-logo-white-144.png 2x, /awr-logo-white-288.png 4x"
        width={craftTokens.brand.markWidth}
        height={craftTokens.brand.markHeight}
        alt=""
        aria-hidden
        data-testid="brand-mark-white"
      />
      <span className="shell-brand-wordmark" data-testid="brand-wordmark">
        AWR
      </span>
    </div>
  );

  const avatarMenu = (
    <Dropdown
      menu={accountMenu}
      trigger={['click']}
      placement="bottomRight"
      popupRender={(menu) => <nav aria-label={t('shell.accountNavigation')}>{menu}</nav>}
    >
      <Button
        type="text"
        className="shell-avatar-button"
        data-testid="avatar-menu-trigger"
        aria-label={t('shell.avatarMenu', { name: memberName })}
        style={{ width: 28, height: 28, padding: 0, borderRadius: '50%', flex: '0 0 auto' }}
      >
        <Avatar
          aria-hidden
          data-testid="current-member-avatar"
          size={28}
          style={{ background: token.colorFillSecondary, color: token.colorTextSecondary }}
        >
          {nameInitial(memberName)}
        </Avatar>
      </Button>
    </Dropdown>
  );

  const tab = new URLSearchParams(location.search).get('tab');
  const tabKey = tab && ['plan', 'deps', 'acceptance', 'knowledge'].includes(tab)
    ? tab as 'plan' | 'deps' | 'acceptance' | 'knowledge'
    : null;
  const projectPath = projectKey ? `/projects/${encodeURIComponent(projectKey)}` : '/projects';
  const projectCrumbs: Crumb[] = selectedProject
    ? [{ label: t('nav.projects'), to: `/projects${scopeSearch}` }, { label: selectedProjectLabel, to: projectPath }]
    : [{ label: t('nav.projects'), to: `/projects${scopeSearch}` }];
  let crumbs: Crumb[];
  if (matchPath({ path: '/projects/:project', end: true }, location.pathname)) {
    crumbs = [
      { label: t('nav.projects'), to: `/projects${scopeSearch}` },
      { label: selectedProjectLabel, to: tabKey ? projectPath : undefined },
    ];
    if (tabKey) crumbs.push({ label: t(`project.tabs.${tabKey}`) });
  } else if (matchPath({ path: '/projects', end: true }, location.pathname)) {
    crumbs = [{ label: t('nav.projects') }];
  } else if (matchPath({ path: '/tasks/:task', end: true }, location.pathname)) {
    crumbs = [...projectCrumbs, { label: t('nav.task') }];
  } else if (matchPath({ path: '/goals/:goal', end: true }, location.pathname)) {
    crumbs = [...projectCrumbs, { label: t('nav.goal') }];
  } else if (matchPath({ path: '/documents/:document', end: true }, location.pathname)) {
    crumbs = [...projectCrumbs, { label: t('nav.document') }];
  } else if (matchPath({ path: '/updates/:update', end: true }, location.pathname)) {
    crumbs = [...projectCrumbs, { label: t('nav.update') }];
  } else if (matchPath({ path: '/members/:member', end: true }, location.pathname)) {
    crumbs = [{ label: t('nav.members'), to: `/members${scopeSearch}` }, { label: t('nav.person') }];
  } else if (matchPath({ path: '/takeover/:result?', end: true }, location.pathname)) {
    crumbs = [...projectCrumbs, { label: t('nav.takeover') }];
  } else if (matchPath({ path: '/account', end: true }, location.pathname)) {
    crumbs = [{ label: t('nav.members'), to: `/members${scopeSearch}` }, { label: t('nav.account') }];
  } else if (matchPath({ path: '/agent-connect', end: true }, location.pathname)) {
    crumbs = [{ label: t('nav.members'), to: `/members${scopeSearch}` }, { label: t('nav.agentConnect') }];
  } else {
    const current = NAVIGATION.find((item) => item.path === selectedKey) ?? PRIMARY_NAVIGATION[0];
    crumbs = [{ label: t(`nav.${current.label}`) }];
  }

  return (
    <PerspectiveProvider
      facts={perspectiveFacts}
      requestedPerspective={requestedPerspective}
      onChange={changePerspective}
    >
      <Layout hasSider={!compactNavigation} style={{ minHeight: '100vh', background: 'var(--craft-color-page)' }}>
      {!compactNavigation ? (
        <Sider
          width={craftTokens.siderWidth}
          className="shell-sider"
          style={{
            position: 'sticky',
            top: 0,
            height: '100vh',
            padding: '16px 12px',
            borderRight: `1px solid ${token.colorSplit}`,
          }}
        >
          <div className="shell-sider-inner">
            {brand()}
            <ShellNavigation selectedKey={selectedKey} pendingLabel={pendingLabel} />
            <ThemeToggle />
          </div>
        </Sider>
      ) : (
        <nav
          aria-label={t('shell.mobileNavigation')}
          data-testid="mobile-navigation"
          style={{
            display: 'flex',
            alignItems: 'center',
            gap: 8,
            minWidth: 0,
            padding: '12px 16px',
            background: 'var(--craft-color-side)',
            borderBottom: `1px solid ${token.colorSplit}`,
          }}
        >
          {brand(true)}
          <select
            aria-label={t('shell.pageSelector')}
            data-testid="page-selector"
            value={selectedKey}
            onChange={(event) => navigate(`${event.target.value}${scopeSearch}`)}
            style={{
              flex: 1,
              minWidth: 0,
              maxWidth: '100%',
              height: 32,
              padding: '0 28px 0 12px',
              border: `1px solid ${token.colorBorder}`,
              borderRadius: 8,
              background: 'var(--craft-color-surface)',
              color: 'var(--craft-color-ink)',
              fontFamily: 'inherit',
              fontSize: 13,
            }}
          >
            {NAVIGATION.map((item) => (
              <option key={item.path} value={item.path}>{t(`nav.${item.label}`)}</option>
            ))}
          </select>
          <Button
            type="text"
            aria-label={mobileThemeLabel}
            data-testid="mobile-theme-toggle"
            onClick={toggle}
            icon={<MobileThemeIcon size={16} strokeWidth={1.5} aria-hidden />}
            style={{ width: craftTokens.touchTarget, height: craftTokens.touchTarget, padding: 0, flex: '0 0 auto' }}
          />
        </nav>
      )}
      <Layout style={{ minWidth: 0, background: 'var(--craft-color-page)' }}>
        <Header
          className="shell-header"
          style={{
            position: compactNavigation ? 'static' : 'sticky',
            top: 0,
            zIndex: 20,
            display: 'flex',
            alignItems: 'center',
            flexWrap: compactNavigation ? 'wrap' : 'nowrap',
            gap: compactNavigation ? 8 : 16,
            height: compactNavigation ? 'auto' : 56,
            minHeight: 56,
            padding: compactNavigation ? '8px 16px' : '0 32px',
            lineHeight: 'normal',
            background: 'var(--craft-color-page)',
            borderBottom: `1px solid ${token.colorSplit}`,
          }}
        >
          <Breadcrumb
            aria-label={t('shell.breadcrumbs')}
            separator={<ChevronRight size={16} strokeWidth={1.5} aria-hidden />}
            style={{
              flex: compactNavigation ? '1 1 auto' : '0 1 auto',
              minWidth: 0,
              maxWidth: '100%',
              overflow: 'hidden',
              color: 'var(--craft-color-muted)',
              fontSize: 13,
              whiteSpace: 'nowrap',
            }}
            items={[
              {
                title: (
                  <span style={{ display: 'inline-flex', alignItems: 'center', gap: 8, minWidth: 0 }}>
                    <Building2 size={16} strokeWidth={1.5} aria-hidden />
                    <Dropdown menu={scopeMenu} trigger={['click']}>
                      <Button
                        type="text"
                        data-testid="scope-switcher"
                        className="shell-scope"
                        style={{ maxWidth: compactNavigation ? 160 : 300, height: 32, paddingInline: 4 }}
                      >
                        <span className="shell-scope-label">{scopeLabel}</span>
                        <ChevronDown size={16} strokeWidth={1.5} aria-hidden />
                      </Button>
                    </Dropdown>
                  </span>
                ),
              },
              ...crumbs.map((crumb, index) => ({
                title: crumb.to ? (
                  <Link to={crumb.to} style={{ whiteSpace: 'nowrap' }}>{crumb.label}</Link>
                ) : (
                  <span
                    aria-current={index === crumbs.length - 1 ? 'page' : undefined}
                    title={crumb.label}
                    style={{
                      display: 'block',
                      flex: '0 1 auto',
                      minWidth: 0,
                      maxWidth: compactNavigation ? 160 : 320,
                      overflow: 'hidden',
                      textOverflow: 'ellipsis',
                      whiteSpace: 'nowrap',
                      color: 'var(--craft-color-ink)',
                      fontWeight: 500,
                    }}
                  >
                    {crumb.label}
                  </span>
                ),
              })),
            ]}
          />
          {compactNavigation ? avatarMenu : null}
          <SearchPanel
            projectKey={projectKey}
            renderTrigger={(openSearch) => !compactNavigation && !compactSearch ? (
              <Button
                disabled={revoked}
                className="shell-search"
                aria-label={t('shell.search')}
                aria-keyshortcuts="Meta+K Control+K"
                onClick={(event) => openSearch(event.currentTarget)}
                icon={<Search size={16} strokeWidth={1.5} aria-hidden />}
                style={{
                  display: 'flex',
                  alignItems: 'center',
                  flex: '0 0 200px',
                  width: 200,
                  height: 32,
                  marginLeft: 'auto',
                  paddingInline: 12,
                  textAlign: 'left',
                }}
              >
                <span style={{ flex: 1 }}>{t('shell.search')}</span>
                <span
                  aria-hidden
                  style={{
                    height: 20,
                    padding: '0 6px',
                    border: `1px solid ${token.colorBorder}`,
                    borderRadius: 4,
                    color: 'var(--craft-color-muted)',
                    fontFamily: craftTokens.font.meta,
                    fontSize: 12,
                    lineHeight: '18px',
                  }}
                >
                  ⌘K
                </span>
              </Button>
            ) : null}
          />
          <PerspectiveSwitcher alignEnd={compactSearch} />
          {!compactNavigation ? (
            <>
              <Badge dot={pendingCount !== null && pendingCount > 0} color="var(--craft-color-accent)">
                <Button
                  type="text"
                  aria-label={pendingCount === null
                    ? t('nav.inbox')
                    : t('shell.pendingNotifications', { count: pendingCount })}
                  onClick={() => navigate(inboxPath)}
                  icon={<Bell size={16} strokeWidth={1.5} aria-hidden />}
                  style={{ width: 32, height: 32, padding: 0 }}
                />
              </Badge>
              {avatarMenu}
            </>
          ) : null}
        </Header>
        <Content className="shell-content" style={compactNavigation ? { padding: 16 } : undefined}>{children}</Content>
      </Layout>
      </Layout>
    </PerspectiveProvider>
  );
}
