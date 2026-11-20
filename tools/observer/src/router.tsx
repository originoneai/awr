import { createBrowserRouter, Navigate, useLocation, type RouteObject } from 'react-router';
import { AccountPage } from './pages/AccountPage';
import { ActivityPage } from './pages/ActivityPage';
import { SettingsPage } from './pages/SettingsPage';
import { InboxPage } from './pages/InboxPage';
import { MembersPage } from './pages/MembersPage';
import { PlaceholderPage, type PlaceholderPageKey } from './pages/PlaceholderPage';
import { AgentConnectPage } from './pages/AgentConnectPage';
import { ProjectsPage } from './pages/ProjectsPage';
import { ProjectShell } from './pages/ProjectShell';
import { WorkbenchPage } from './pages/WorkbenchPage';
import { RootLayout } from './RootLayout';

type NewPlaceholderKey = 'lessons' | 'task' | 'person' | 'goal' | 'document' | 'update' | 'takeover';

/** The shared placeholder resolves nav labels at runtime; this adapter adds only keys declared above. */
function placeholder(pageKey: NewPlaceholderKey) {
  return <PlaceholderPage pageKey={pageKey as PlaceholderPageKey} />;
}

function LegacyRedirect({ to }: { to: string }) {
  const location = useLocation();
  return <Navigate to={{ pathname: to, search: location.search }} replace />;
}

export const routes: RouteObject[] = [
  {
    path: '/',
    element: <RootLayout />,
    children: [
      { index: true, element: <Navigate to="/workbench" replace /> },
      { path: 'workbench', element: <WorkbenchPage /> },
      { path: 'projects', element: <ProjectsPage /> },
      // ProjectShell routes all five project pages: overview, plan, dependencies, acceptance and knowledge.
      { path: 'projects/:project', element: <ProjectShell /> },
      { path: 'tasks/:task', element: placeholder('task') },
      { path: 'members/:member', element: placeholder('person') },
      { path: 'goals/:goal', element: placeholder('goal') },
      { path: 'documents/:document', element: placeholder('document') },
      { path: 'updates/:update', element: placeholder('update') },
      { path: 'inbox', element: <InboxPage /> },
      { path: 'lessons', element: placeholder('lessons') },
      { path: 'members', element: <MembersPage /> },
      { path: 'activity', element: <ActivityPage /> },
      { path: 'settings', element: <SettingsPage /> },
      { path: 'takeover', element: placeholder('takeover') },
      { path: 'takeover/:result', element: placeholder('takeover') },
      // Account and Agent connection flows remain available outside the read-only business pages.
      { path: 'account', element: <AccountPage /> },
      { path: 'agent-connect', element: <AgentConnectPage /> },
      { path: 'home', element: <LegacyRedirect to="/workbench" /> },
      { path: 'knowledge', element: <LegacyRedirect to="/lessons" /> },
      { path: 'advanced', element: <LegacyRedirect to="/settings" /> },
      { path: '*', element: <Navigate to="/workbench" replace /> },
    ],
  },
];

/** Data router (createBrowserRouter) — ScrollRestoration lives in RootLayout. */
export const router = createBrowserRouter(routes);
