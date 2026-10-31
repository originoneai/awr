import { createBrowserRouter, Navigate, type RouteObject } from 'react-router';
import { AccountPage } from './pages/AccountPage';
import { InboxPage } from './pages/InboxPage';
import { PlaceholderPage } from './pages/PlaceholderPage';
import { ProjectsPage } from './pages/ProjectsPage';
import { ProjectShell } from './pages/ProjectShell';
import { WorkbenchPage } from './pages/WorkbenchPage';
import { RootLayout } from './RootLayout';

export const routes: RouteObject[] = [
  {
    path: '/',
    element: <RootLayout />,
    children: [
      { index: true, element: <Navigate to="/workbench" replace /> },
      { path: 'workbench', element: <WorkbenchPage /> },
      { path: 'projects', element: <ProjectsPage /> },
      { path: 'projects/:project', element: <ProjectShell /> },
      { path: 'inbox', element: <InboxPage /> },
      { path: 'knowledge', element: <PlaceholderPage pageKey="knowledge" /> },
      { path: 'members', element: <PlaceholderPage pageKey="members" /> },
      { path: 'activity', element: <PlaceholderPage pageKey="activity" /> },
      { path: 'settings', element: <PlaceholderPage pageKey="settings" /> },
      { path: 'account', element: <AccountPage /> },
      { path: 'advanced', element: <PlaceholderPage pageKey="advanced" /> },
      { path: '*', element: <Navigate to="/workbench" replace /> },
    ],
  },
];

/** Data router (createBrowserRouter) — ScrollRestoration lives in RootLayout. */
export const router = createBrowserRouter(routes);
