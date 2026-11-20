import { screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it } from 'vitest';
import i18n from './i18n';
import { DEMO_OVERVIEW, DEMO_PROJECTS, renderApp, stubFetch } from './test-utils';

const CANONICAL_SHELL = {
  ok: true,
  source: 'demo',
  perspective: {
    supervisedGroupCount: 0,
    memberGroupProjectCount: 1,
    visibleProjectGroupCount: 3,
    visibleProjectDepartmentCount: 2,
    canViewAllProjects: true,
    directory: { groupsRegistered: true, departmentsRegistered: true },
    pending: { me: 0, lead: 0, dept: 0, top: 0 },
  },
};

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
    setup({ ok: true, source: 'live', perspective: null }, '/projects');
    expect(await screen.findByTestId('data-source-trigger')).toBeTruthy();
    expect(screen.queryByTestId('shell-demo-note')).toBeNull();
    expect(screen.queryByTestId('perspective-switcher')).toBeNull();
    await user.click(screen.getByTestId('data-source-trigger'));
    expect((await screen.findByTestId('data-source-explanation')).textContent)
      .toContain('这一页读取的是线上项目的当前记录');
  });

  it('does not guess the source when its bridge read fails', async () => {
    const user = userEvent.setup();
    const { fetchMock } = setup(undefined);
    await waitFor(() => expect(fetchMock).toHaveBeenCalled());
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
});
