import { QueryClientProvider } from '@tanstack/react-query';
import { App as AntdApp } from 'antd';
import { act, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import { MemoryRouter, Route, Routes, useLocation, type Location } from 'react-router';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { AppShell } from './components/AppShell';
import { CraftThemeProvider } from './components/CraftThemeProvider';
import i18n from './i18n';
import { PerspectiveLimitationNote } from './lib/perspective/PerspectiveLimitationNote';
import { usePerspective } from './lib/perspective/PerspectiveProvider';
import { PERSPECTIVE_STORAGE_KEY } from './lib/perspective/usePerspectiveSelection';
import { createAppQueryClient, revokeObservationReads } from './lib/queries';
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

function PerspectiveProbe() {
  const { selected } = usePerspective();
  return (
    <div>
      <h1>工作台</h1>
      <output data-testid="page-perspective">{i18n.t(`shell.perspectives.${selected}.title`)}</output>
      <PerspectiveLimitationNote />
    </div>
  );
}

function pagePerspective() {
  return screen.getByTestId('page-perspective').textContent;
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
                        <PerspectiveProbe />
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

  it('remembers a selection across a fresh mount and shares it with the page', async () => {
    const user = userEvent.setup();
    const first = renderShell();
    await user.click(screen.getByRole('button', { name: '小组负责人' }));
    expect(window.localStorage.getItem(PERSPECTIVE_STORAGE_KEY)).toBe('lead');
    expect(first.search()).toContain('perspective=lead');
    first.unmount();

    renderShell();
    expect(pressed('小组负责人')).toBe('true');
    expect(pressed('做事的人')).toBe('false');
    expect(pagePerspective()).toBe('小组交付');
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

  it('shares the safe fallback with the page after a forged URL', async () => {
    const shell = renderShell(narrowFacts(), '/workbench?scope=demo&perspective=top');
    expect(pressed('做事的人')).toBe('true');
    expect(pagePerspective()).toBe('我的工作');
    expect(screen.queryByRole('button', { name: '最高层' })).toBeNull();
    expect(screen.getByRole('heading', { name: '工作台' })).toBeTruthy();
    await waitFor(() => expect(shell.search()).toContain('perspective=me'));
    expect(shell.search()).toContain('scope=demo');
  });

  it('updates the page and switcher in the same render on revocation', async () => {
    window.localStorage.setItem(PERSPECTIVE_STORAGE_KEY, 'top');
    const shell = renderShell(fullFacts(), '/workbench?perspective=top');
    expect(pressed('最高层')).toBe('true');
    act(() => revokeObservationReads(shell.queryClient));
    expect(screen.queryByTestId('perspective-switcher')).toBeNull();
    expect(pagePerspective()).toBe('我的工作');
    expect(screen.getByTestId('perspective-limitation-note').closest('.shell-content')).not.toBeNull();
    expect(document.querySelector('.shell-header')?.textContent).not.toContain('未知');
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
    expect(screen.getByTestId('perspective-limitation-note').textContent).toContain('小组与部门登记情况未知');
    expect(document.querySelector('.shell-header')?.textContent).not.toContain('小组与部门登记情况未知');
    await waitFor(() => expect(shell.search()).toContain('perspective=me'));
  });

  it('describes a missing department without visible header limitation text', () => {
    const facts = { ...fullFacts(), directory: { groupsRegistered: true, departmentsRegistered: false } };
    renderShell(facts);
    const switcher = screen.getByTestId('perspective-switcher');
    expect(switcher.getAttribute('title')).toContain('尚未登记部门');
    expect(switcher.textContent).not.toContain('尚未登记部门');
    expect(screen.getByTestId('perspective-limitation-note').textContent).toContain('尚未登记部门');
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

  it('uses the shared control with Home/End and a single Tab stop', async () => {
    const user = userEvent.setup();
    renderShell();
    const group = screen.getByRole('group', { name: '视角' });
    expect(group.classList.contains('obs-segmented-control')).toBe(true);
    const buttons = within(group).getAllByRole('button');
    buttons[0].focus();
    await user.keyboard('{End}');
    expect(document.activeElement).toBe(buttons[3]);
    expect(buttons.map((button) => button.tabIndex)).toEqual([-1, -1, -1, 0]);
    expect(buttons[0].getAttribute('aria-pressed')).toBe('true');
    await user.tab();
    expect(group.contains(document.activeElement)).toBe(false);
    await user.tab({ shift: true });
    expect(document.activeElement).toBe(buttons[3]);
    await user.keyboard('{Home}{ArrowLeft}');
    expect(document.activeElement).toBe(buttons[3]);
    await user.keyboard('{Home}');
    expect(document.activeElement).toBe(buttons[0]);
  });

  it('renders no perspective chrome with one height while the page keeps its resolved title', () => {
    renderShell(personalFacts(), '/workbench?perspective=top');
    const header = document.querySelector('.shell-header');
    expect(header?.querySelector('.shell-perspective')).toBeNull();
    expect(header?.textContent).not.toContain('视角');
    expect(header?.textContent).not.toContain('我的工作');
    expect(screen.queryByTestId('perspective-limitation-note')).toBeNull();
    expect(pagePerspective()).toBe('我的工作');
    expect(screen.getByRole('heading', { name: '工作台' })).toBeTruthy();
  });

  it('throws a clear developer error when usePerspective is called outside its provider', () => {
    vi.spyOn(console, 'error').mockImplementation(() => {});
    expect(() => render(<PerspectiveProbe />)).toThrow('usePerspective must be used within PerspectiveProvider');
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
