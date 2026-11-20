import type { QueryClient } from '@tanstack/react-query';
import { screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it } from 'vitest';
import i18n from './i18n';
import { queryKeys } from './lib/queries';
import { DEMO_OVERVIEW, DEMO_PROJECTS, renderApp, stubFetch } from './test-utils';

const CANONICAL_PERSPECTIVE = {
  supervisedGroupCount: 0,
  memberGroupProjectCount: 1,
  visibleProjectGroupCount: 3,
  visibleProjectDepartmentCount: 2,
  canViewAllProjects: true,
  directory: { groupsRegistered: true, departmentsRegistered: true },
  pending: { me: 0, lead: 0, dept: 0, top: 0 },
};

const CANONICAL_SHELL = {
  ok: true,
  source: 'demo',
  perspective: CANONICAL_PERSPECTIVE,
};

/** The shell read has finished (answered or failed) and the page has re-rendered from it. An
 * absence asserted before this point would pass whatever the reply said. */
async function shellReadSettled(queryClient: QueryClient) {
  await waitFor(() => {
    const state = queryClient.getQueryState(queryKeys.shell);
    expect(state?.status).not.toBe('pending');
    expect(state?.fetchStatus).toBe('idle');
  });
}

function setup(shell: unknown, path = '/workbench') {
  const fetchMock = stubFetch({
    '/api/team/shell': shell,
    '/api/team/projects': DEMO_PROJECTS,
    '/api/team/overview': DEMO_OVERVIEW,
  });
  const rendered = renderApp(path);
  return { fetchMock, ...rendered };
}

afterEach(async () => {
  await i18n.changeLanguage('zh-CN');
});

describe('shell data source and demo facts', () => {
  it('derives all four perspective buttons from canonical facts and explains synthetic reads', async () => {
    const user = userEvent.setup();
    const { fetchMock } = setup(CANONICAL_SHELL);
    expect((await screen.findByTestId('shell-demo-note')).textContent).toBe('示例数据，全部是合成的');
    expect(screen.getByText('视角', { selector: '.shell-perspective-caption > span' })).toBeTruthy();
    expect(screen.getAllByRole('button', { name: /^(做事的人|小组负责人|部门负责人|最高层)$/ })).toHaveLength(4);

    const trigger = screen.getByTestId('data-source-trigger');
    const callsBefore = fetchMock.mock.calls.length;
    await user.click(trigger);
    const explanation = await screen.findByTestId('data-source-explanation');
    expect(explanation.textContent).toContain('这一页读取的是演示数据，全部是合成的');
    expect(explanation.textContent).toContain('只读');
    expect(explanation.querySelectorAll('button, a, input, select, textarea')).toHaveLength(0);
    expect(fetchMock.mock.calls.length).toBe(callsBefore);
  });

  it('keeps the controls on another page and uses live bridge facts without a demo note', async () => {
    const user = userEvent.setup();
    const { queryClient } = setup({ ok: true, source: 'live', perspective: null }, '/projects');
    expect(await screen.findByTestId('data-source-trigger')).toBeTruthy();
    await shellReadSettled(queryClient);
    expect(screen.queryByTestId('shell-demo-note')).toBeNull();
    expect(screen.queryByTestId('perspective-switcher')).toBeNull();
    await user.click(screen.getByTestId('data-source-trigger'));
    expect((await screen.findByTestId('data-source-explanation')).textContent)
      .toContain('这一页读取的是线上项目的当前记录');
  });

  it('does not guess the source when its bridge read fails', async () => {
    const user = userEvent.setup();
    const { fetchMock, queryClient } = setup(undefined);
    await waitFor(() => expect(fetchMock).toHaveBeenCalled());
    await shellReadSettled(queryClient);
    expect(screen.queryByTestId('shell-demo-note')).toBeNull();
    expect(screen.queryByTestId('perspective-switcher')).toBeNull();
    await user.click(screen.getByTestId('data-source-trigger'));
    expect((await screen.findByTestId('data-source-explanation')).textContent)
      .toContain('暂时无法确认这一页的数据来源');
  });

  it('uses product language in English and never exposes bridge field names', async () => {
    const user = userEvent.setup();
    setup({ ok: true, source: 'live', perspective: null });
    await i18n.changeLanguage('en-US');
    await user.click(await screen.findByTestId('data-source-trigger'));
    expect((await screen.findByTestId('data-source-explanation')).textContent)
      .toContain('This page reads current records from the online project');
    expect(document.body.textContent).not.toMatch(/\b(?:undefined|null|NaN)\b/);
    expect(document.body.textContent).not.toMatch(/\b(?:source|perspective|canViewAllProjects|groupsRegistered)\b/);
  });
  it('takes nothing from a reply that is not ok, and reads a source it does not know as unknown', async () => {
    const user = userEvent.setup();
    // Not ok: the whole reply is discarded, so neither the source nor the facts it carries count.
    const notOk = setup({ ok: false, source: 'demo', perspective: CANONICAL_PERSPECTIVE });
    await shellReadSettled(notOk.queryClient);
    expect(screen.queryByTestId('shell-demo-note')).toBeNull();
    expect(screen.queryByTestId('perspective-switcher')).toBeNull();
    await user.click(screen.getByTestId('data-source-trigger'));
    expect((await screen.findByTestId('data-source-explanation')).textContent)
      .toContain('暂时无法确认这一页的数据来源');
    notOk.unmount();

    // An unknown source value is unknown on its own: no demo notice and no guessed explanation.
    const sandbox = setup({ ok: true, source: 'sandbox', perspective: null });
    await shellReadSettled(sandbox.queryClient);
    expect(screen.queryByTestId('shell-demo-note')).toBeNull();
    await user.click(screen.getByTestId('data-source-trigger'));
    expect((await screen.findByTestId('data-source-explanation')).textContent)
      .toContain('暂时无法确认这一页的数据来源');
  });

  it('discards perspective facts with an impossible count or a wrong type instead of opening a view', async () => {
    const cases: Array<[string, unknown]> = [
      ['a negative count', { ...CANONICAL_PERSPECTIVE, supervisedGroupCount: -1 }],
      ['a fractional count', { ...CANONICAL_PERSPECTIVE, visibleProjectGroupCount: 1.5 }],
      ['a count that is text', { ...CANONICAL_PERSPECTIVE, memberGroupProjectCount: '1' }],
      ['a pending slot that is missing', { ...CANONICAL_PERSPECTIVE, pending: { me: 0, lead: 0, dept: 0 } }],
      ['a grant that is not a boolean', { ...CANONICAL_PERSPECTIVE, canViewAllProjects: 'yes' }],
    ];
    for (const [name, perspective] of cases) {
      const { queryClient, unmount } = setup({ ok: true, source: 'demo', perspective });
      // The demo source is still read, so the notice proves the reply was taken in.
      expect(await screen.findByTestId('shell-demo-note'), name).toBeTruthy();
      await shellReadSettled(queryClient);
      expect(screen.queryByTestId('perspective-switcher'), name).toBeNull();
      expect(screen.queryAllByRole('button', { name: /^(小组负责人|部门负责人|最高层)$/ }), name).toHaveLength(0);
      unmount();
    }
  });

  it('names the organization from the shell reply and falls back to the authorization wording when it is blank', async () => {
    const { queryClient, unmount } = setup({ ok: true, source: 'demo', organizationName: '示例组织', perspective: null });
    await shellReadSettled(queryClient);
    await waitFor(() => expect(screen.getByTestId('scope-switcher').textContent).toBe('示例组织'));
    unmount();
    for (const organizationName of ['   ', null]) {
      const next = setup({ ok: true, source: 'demo', organizationName, perspective: null });
      await shellReadSettled(next.queryClient);
      expect(screen.getByTestId('scope-switcher').textContent, String(organizationName)).toBe('全部授权范围');
      next.unmount();
    }
  });
});
