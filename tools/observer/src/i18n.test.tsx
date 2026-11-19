import { screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it } from 'vitest';
import i18n from './i18n';
import { DEMO_PROJECTS, DEMO_WORK_NEXT, renderApp, stubFetch } from './test-utils';

afterEach(async () => {
  await i18n.changeLanguage('zh-CN');
});

describe('i18n', () => {
  it('defaults to zh-CN nav labels', () => {
    stubFetch({ '/api/team/projects': DEMO_PROJECTS, '/api/team/read': DEMO_WORK_NEXT });
    renderApp('/workbench');
    expect(screen.getByRole('menuitem', { name: /工作台/ })).toBeTruthy();
  });

  it('switches rendered nav labels with the language', async () => {
    stubFetch({ '/api/team/projects': DEMO_PROJECTS, '/api/team/read': DEMO_WORK_NEXT });
    renderApp('/workbench');
    await i18n.changeLanguage('en-US');
    await waitFor(() =>
      expect(screen.getByRole('menuitem', { name: /Workbench/ })).toBeTruthy(),
    );
    expect(screen.getByRole('menuitem', { name: /Inbox/ })).toBeTruthy();
    expect(screen.queryByRole('menuitem', { name: /工作台/ })).toBeNull();
  });

  it('opens the new lessons destination with translated shell labels', async () => {
    stubFetch({ '/api/team/projects': DEMO_PROJECTS, '/api/team/read': DEMO_WORK_NEXT });
    const user = userEvent.setup();
    const { router } = renderApp('/workbench');
    await i18n.changeLanguage('en-US');
    await user.click(await screen.findByRole('link', { name: 'Lessons' }));
    await screen.findByTestId('page-lessons');
    expect(router.state.location.pathname).toBe('/lessons');
    expect(screen.getByText('Lessons', { selector: '[aria-current="page"]' })).toBeTruthy();
    expect(i18n.language).toBe('en-US');
  });
});
