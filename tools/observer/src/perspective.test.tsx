import { QueryClientProvider } from '@tanstack/react-query';
import { App as AntdApp } from 'antd';
import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import { MemoryRouter, Route, Routes, useLocation, type Location } from 'react-router';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { AppShell } from './components/AppShell';
import { CraftThemeProvider } from './components/CraftThemeProvider';
import i18n from './i18n';
import { createAppQueryClient, revokeObservationReads } from './lib/queries';
import { PERSPECTIVE_STORAGE_KEY } from './lib/perspective/usePerspectiveSelection';
import type { PerspectiveFacts } from './lib/perspective/types';
import { RevocationProvider } from './lib/revocation';
import { ScopeProvider } from './lib/scope';
import { DEMO_PROJECTS, stubFetch } from './test-utils';

function fullFacts(): PerspectiveFacts {
  return {
    supervisedGroupCount: 1,
    memberGroupProjectCount: 1,
    visibleProjectGroupCount: 3,
    visibleProjectDepartmentCount: 2,
    canViewAllProjects: true,
    directory: { groupsRegistered: true, departmentsRegistered: true },
    pending: { me: 0, lead: 0, dept: 0, top: 0 },
  };
}

function narrowFacts(): PerspectiveFacts {
  return {
    ...fullFacts(),
    visibleProjectGroupCount: 1,
    visibleProjectDepartmentCount: 1,
    canViewAllProjects: false,
  };
}

function personalFacts(): PerspectiveFacts {
  return { ...narrowFacts(), supervisedGroupCount: 0, memberGroupProjectCount: 0 };
}

function pressed(name: string) {
  return screen.getByRole('button', { name }).getAttribute('aria-pressed');
}

function renderShell(facts: PerspectiveFacts = fullFacts(), initialPath = '/workbench') {
  const fetchMock = stubFetch({ '/api/team/projects': DEMO_PROJECTS });
  const queryClient = createAppQueryClient({ retry: false });
  const locationRef: { current: Location | null } = { current: null };
  function LocationProbe() {
    locationRef.current = useLocation();
    return null;
  }
  const tree = (currentFacts: PerspectiveFacts): ReactNode => (
    <QueryClientProvider client={queryClient}>
      <CraftThemeProvider>
        <AntdApp>
          <MemoryRouter initialEntries={[initialPath]}>
            <LocationProbe />
            <RevocationProvider>
              <ScopeProvider>
                <Routes>
                  <Route
                    path="/workbench"
                    element={(
                      <AppShell perspectiveFacts={currentFacts}>
                        <h1>工作台</h1>
                      </AppShell>
                    )}
                  />
                </Routes>
              </ScopeProvider>
            </RevocationProvider>
          </MemoryRouter>
        </AntdApp>
      </CraftThemeProvider>
    </QueryClientProvider>
  );
  const utils = render(tree(facts));
  return {
    ...utils,
    queryClient,
    fetchMock,
    search: () => locationRef.current?.search ?? '',
    rerenderFacts(next: PerspectiveFacts) {
      act(() => utils.rerender(tree(next)));
    },
  };
}

beforeEach(() => {
  window.localStorage.removeItem(PERSPECTIVE_STORAGE_KEY);
});

afterEach(async () => {
  vi.restoreAllMocks();
  window.localStorage.removeItem(PERSPECTIVE_STORAGE_KEY);
  await i18n.changeLanguage('zh-CN');
});

describe('perspective shell integration', () => {
  it('lists only authorized heights and selects the narrowest one with pending work', () => {
    const facts = { ...narrowFacts(), visibleProjectGroupCount: 2, pending: { me: 0, lead: 0, dept: 3, top: null } };
    renderShell(facts);
    expect(screen.getAllByRole('button', { name: /^(做事的人|小组负责人|部门负责人)$/ })).toHaveLength(3);
    expect(screen.queryByRole('button', { name: '最高层' })).toBeNull();
    expect(pressed('部门负责人')).toBe('true');
    expect(pressed('做事的人')).toBe('false');
  });

  it('honors an authorized URL request without treating it as an authorization fact', () => {
    renderShell(fullFacts(), '/workbench?perspective=dept');
    expect(pressed('部门负责人')).toBe('true');
    expect(pressed('最高层')).toBe('false');
  });

  it('remembers a selection across a fresh mount', async () => {
    const user = userEvent.setup();
    const first = renderShell();
    await user.click(screen.getByRole('button', { name: '小组负责人' }));
    expect(window.localStorage.getItem(PERSPECTIVE_STORAGE_KEY)).toBe('lead');
    expect(first.search()).toContain('perspective=lead');
    first.unmount();

    renderShell();
    expect(pressed('小组负责人')).toBe('true');
    expect(pressed('做事的人')).toBe('false');
  });

  it('uses the default and keeps switching when storage reads and writes are blocked', async () => {
    vi.spyOn(Storage.prototype, 'getItem').mockImplementation(() => { throw new Error('Blocked'); });
    vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => { throw new Error('Blocked'); });
    const facts = { ...fullFacts(), pending: { me: 0, lead: 0, dept: 2, top: 0 } };
    const user = userEvent.setup();
    const first = renderShell(facts);
    expect(pressed('部门负责人')).toBe('true');
    await user.click(screen.getByRole('button', { name: '小组负责人' }));
    expect(pressed('小组负责人')).toBe('true');
    first.unmount();

    renderShell(facts);
    expect(pressed('部门负责人')).toBe('true');
  });

  it('silently rejects a forged URL, preserves other parameters and leaves the page usable', async () => {
    const shell = renderShell(narrowFacts(), '/workbench?scope=demo&perspective=top');
    expect(pressed('做事的人')).toBe('true');
    expect(screen.queryByRole('button', { name: '最高层' })).toBeNull();
    expect(screen.getByRole('heading', { name: '工作台' })).toBeTruthy();
    await waitFor(() => expect(shell.search()).toContain('perspective=me'));
    expect(shell.search()).toContain('scope=demo');
  });

  it('immediately falls back on revocation even with a residual URL and stored height', async () => {
    window.localStorage.setItem(PERSPECTIVE_STORAGE_KEY, 'top');
    const shell = renderShell(fullFacts(), '/workbench?perspective=top');
    expect(pressed('最高层')).toBe('true');
    act(() => revokeObservationReads(shell.queryClient));
    expect(screen.queryByTestId('perspective-switcher')).toBeNull();
    expect(screen.getByTestId('perspective-current').textContent).toBe('我的工作');
    expect(screen.getByRole('heading', { name: '工作台' })).toBeTruthy();
    await waitFor(() => expect(shell.search()).toContain('perspective=me'));
  });

  it('immediately falls back when a previously known directory fact becomes unknown', async () => {
    const shell = renderShell(fullFacts(), '/workbench?perspective=top');
    expect(pressed('最高层')).toBe('true');
    const unknownDirectory = {
      ...fullFacts(),
      directory: { groupsRegistered: true, departmentsRegistered: null },
    };
    shell.rerenderFacts(unknownDirectory);
    expect(screen.queryByRole('button', { name: '最高层' })).toBeNull();
    expect(pressed('做事的人')).toBe('true');
    expect(screen.getByText('小组与部门登记情况未知')).toBeTruthy();
    await waitFor(() => expect(shell.search()).toContain('perspective=me'));
  });

  it('shows the exact missing-department explanation and at most the first two heights', () => {
    const facts = { ...fullFacts(), directory: { groupsRegistered: true, departmentsRegistered: false } };
    renderShell(facts);
    expect(screen.getByText('尚未登记部门')).toBeTruthy();
    expect(screen.getByRole('button', { name: '小组负责人' })).toBeTruthy();
    expect(screen.queryByRole('button', { name: '部门负责人' })).toBeNull();
    expect(screen.queryByRole('button', { name: '最高层' })).toBeNull();
  });

  it('moves with arrow keys without selecting, then selects with Enter or Space', async () => {
    const user = userEvent.setup();
    renderShell();
    const me = screen.getByRole('button', { name: '做事的人' });
    me.focus();
    await user.keyboard('{ArrowRight}');
    const lead = screen.getByRole('button', { name: '小组负责人' });
    expect(document.activeElement).toBe(lead);
    expect(pressed('做事的人')).toBe('true');
    await user.keyboard('{Enter}');
    expect(pressed('小组负责人')).toBe('true');
    await user.keyboard('{ArrowRight}');
    expect(document.activeElement).toBe(screen.getByRole('button', { name: '部门负责人' }));
    await user.keyboard('{Space}');
    expect(pressed('部门负责人')).toBe('true');
  });

  it('hides the switcher with one height while leaving the page title in place', () => {
    renderShell(personalFacts(), '/workbench?perspective=top');
    expect(screen.queryByTestId('perspective-switcher')).toBeNull();
    expect(screen.getByTestId('perspective-current').textContent).toBe('我的工作');
    expect(screen.getByRole('heading', { name: '工作台' })).toBeTruthy();
  });

  it('adds no fact read of its own', async () => {
    const shell = renderShell(fullFacts());
    await waitFor(() => expect(shell.fetchMock).toHaveBeenCalled());
    const urls = shell.fetchMock.mock.calls.map(([input]) => String(input));
    expect(urls.some((url) => url.includes('/api/team/overview'))).toBe(false);
    expect(urls.some((url) => url.includes('/api/team/read'))).toBe(false);
  });

  it('keeps all rendered perspective copy free of empty artifacts and raw field names', () => {
    renderShell(fullFacts());
    expect(document.body.textContent).not.toMatch(/\b(?:undefined|null|NaN)\b/);
    expect(document.body.textContent).not.toMatch(/\b(?:perspectiveFacts|visibleProjectGroupCount|canViewAllProjects|groupsRegistered)\b/);
  });
});
