import { QueryClientProvider } from '@tanstack/react-query';
import { App, Button, theme } from 'antd';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it } from 'vitest';
import { createMemoryRouter, RouterProvider } from 'react-router';
import { RootLayout } from './RootLayout';
import { THEME_STORAGE_KEY } from './components/CraftThemeProvider';
import { createAppQueryClient } from './lib/queries';
import { craftThemes, type CraftThemeMode } from './theme';
import { DEMO_PROJECTS, stubFetch } from './test-utils';

function ThemeTokenProbe({ kind }: { kind: 'page' | 'message' | 'modal' | 'notification' }) {
  const { token } = theme.useToken();
  return (
    <span
      data-testid={`${kind}-theme-token`}
      data-surface={token.colorBgContainer}
      data-text={token.colorText}
    >
      Theme probe
    </span>
  );
}

function FeedbackTrigger() {
  const { message, modal, notification } = App.useApp();
  const showFeedback = () => {
    message.info({ key: 'theme-message', duration: 0, content: <ThemeTokenProbe kind="message" /> });
    modal.info({ title: 'Theme probe', content: <ThemeTokenProbe kind="modal" /> });
    notification.info({
      key: 'theme-notification',
      duration: 0,
      title: <ThemeTokenProbe kind="notification" />,
    });
  };
  return (
    <>
      <ThemeTokenProbe kind="page" />
      <Button onClick={showFeedback}>Show feedback</Button>
    </>
  );
}

function renderRootFeedback(mode: CraftThemeMode) {
  window.localStorage.setItem(THEME_STORAGE_KEY, mode);
  stubFetch({ '/api/team/projects': DEMO_PROJECTS });
  const queryClient = createAppQueryClient({ retry: false });
  const router = createMemoryRouter([{
    path: '/',
    element: <RootLayout />,
    children: [{ index: true, element: <FeedbackTrigger /> }],
  }]);
  return render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
}

afterEach(() => {
  window.localStorage.removeItem(THEME_STORAGE_KEY);
});

describe('RootLayout feedback theme regression', () => {
  it.each(['light', 'dark'] as const)('keeps message, modal and notification on the active %s theme', async (mode) => {
    renderRootFeedback(mode);
    expect(screen.getByTestId('page-theme-token').getAttribute('data-surface'))
      .toBe(craftThemes[mode].color.surface);

    await userEvent.click(screen.getByRole('button', { name: 'Show feedback' }));
    for (const kind of ['message', 'modal', 'notification'] as const) {
      const probe = await screen.findByTestId(`${kind}-theme-token`);
      expect(probe.getAttribute('data-surface'), `${kind} surface`).toBe(craftThemes[mode].color.surface);
      expect(probe.getAttribute('data-text'), `${kind} text`).toBe(craftThemes[mode].color.ink);
    }
  });
});
