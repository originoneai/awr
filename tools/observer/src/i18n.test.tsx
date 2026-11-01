import { screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it } from 'vitest';
import i18n from './i18n';
import { DEMO_OVERVIEW, DEMO_PROJECTS, renderApp, stubFetch } from './test-utils';

afterEach(async () => {
  await i18n.changeLanguage('zh-CN');
});

describe('i18n', () => {
  it('defaults to zh-CN nav labels', () => {
    stubFetch({ '/api/team/projects': DEMO_PROJECTS, '/api/team/overview': DEMO_OVERVIEW });
    renderApp('/workbench');
    expect(screen.getByRole('menuitem', { name: /工作台/ })).toBeTruthy();
  });

  it('switches rendered nav labels with the language', async () => {
    stubFetch({ '/api/team/projects': DEMO_PROJECTS, '/api/team/overview': DEMO_OVERVIEW });
    renderApp('/workbench');
    await i18n.changeLanguage('en-US');
    await waitFor(() =>
      expect(screen.getByRole('menuitem', { name: /Workbench/ })).toBeTruthy(),
    );
    expect(screen.getByRole('menuitem', { name: /Inbox/ })).toBeTruthy();
    expect(screen.queryByRole('menuitem', { name: /工作台/ })).toBeNull();
  });

  it('switches via the top-bar language switcher', async () => {
    stubFetch({ '/api/team/projects': DEMO_PROJECTS, '/api/team/overview': DEMO_OVERVIEW });
    const user = userEvent.setup();
    renderApp('/workbench');
    await user.click(screen.getByTestId('language-switcher'));
    await user.click(await screen.findByText('English'));
    await waitFor(() =>
      expect(screen.getByRole('menuitem', { name: /Workbench/ })).toBeTruthy(),
    );
    expect(i18n.language).toBe('en-US');
  });
});
