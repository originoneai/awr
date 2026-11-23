import { readFileSync } from 'node:fs';
import { act, fireEvent, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { ProjectListEntry, ProjectListModel } from './components/projects/project-list-model';
import i18n from './i18n';
import { queryKeys } from './lib/queries';
import type { OverviewInput } from './core/overview';
import { DEMO_OVERVIEW, DEMO_PROJECTS, renderApp, stubFetch } from './test-utils';

const NOW = Date.parse('2026-11-19T12:00:00+08:00');
const DAY = 86_400_000;

function input(): OverviewInput {
  const complete = { coverage: 'complete', items: [] } as const;
  return {
    tasks: complete,
    dependencies: complete,
    decisions: complete,
    reviewRounds: complete,
    handoffs: complete,
    approvals: complete,
    members: { coverage: 'complete', items: [{ personId: 'person-a', displayName: 'Owner A' }] },
    activity: complete,
    options: { nowUnixMs: NOW, stalledAfterDays: 7 },
  };
}

function model(project = 'project-a', options: {
  periodUpdates?: ProjectListModel['periodUpdates'];
  input?: OverviewInput;
} = {}): ProjectListModel {
  return {
    ok: true,
    project,
    // The render clock walks forward from this real answer moment to the input's explicit
    // derivation clock (options.nowUnixMs), so the update column stays deterministic.
    read_at_unix_ms: Date.now(),
    input: options.input ?? input(),
    coverage: {
      tasks: { status: 'complete', missing: [] }, dependencies: { status: 'complete', missing: [] },
      decisions: { status: 'complete', missing: [] }, reviewRounds: { status: 'complete', missing: [] },
      handoffs: { status: 'complete', missing: [] }, approvals: { status: 'complete', missing: [] },
      members: { status: 'complete', missing: [] }, activity: { status: 'complete', missing: [] },
    },
    freshness: {
      report_received: { status: 'not_measured' },
      authoritative: { status: 'measured', at_unix_ms: NOW },
      page_read: { status: 'measured', at_unix_ms: NOW },
    },
    ...(options.periodUpdates === undefined ? {} : { periodUpdates: options.periodUpdates }),
  };
}

function entry(key: string, title: string | null, overrides: Partial<ProjectListEntry> = {}): ProjectListEntry {
  return { key, title, role: 'reader', leadPersonId: 'person-a', groupName: 'Group A', takeover: null, ...overrides };
}

function json(payload: unknown) {
  return new Response(JSON.stringify(payload), { status: 200, headers: { 'content-type': 'application/json' } });
}

function installFetch(options: {
  projects?: ProjectListEntry[] | { error: string };
  models?: Record<string, ProjectListModel | { error: string }>;
  /** The shell read's reply; unset keeps the unrouted default (an unpinned deployment). */
  shell?: unknown;
  hold?: boolean;
  revoked?: boolean;
} = {}) {
  const projects = options.projects ?? [entry('project-a', 'Project Alpha')];
  const models = options.models ?? Object.fromEntries((Array.isArray(projects) ? projects : []).map((project) => [project.key, model(project.key)]));
  return vi.stubGlobal('fetch', vi.fn(async (input: RequestInfo | URL) => {
    if (options.hold) return new Promise<Response>(() => {});
    const url = String(input);
    if (options.revoked) return json({ ok: false, error: { code: 'Unauthenticated', message: 'synthetic revocation' } });
    if (url.includes('/api/team/overview-model')) {
      const key = new URL(url, 'http://synthetic.invalid').searchParams.get('project') ?? '';
      const reply = models[key] ?? { error: 'NoRoute' };
      return 'error' in reply ? json({ ok: false, error: { code: reply.error, message: 'synthetic read failure' } }) : json(reply);
    }
    if (url.includes('/api/team/projects')) {
      return 'error' in projects
        ? json({ ok: false, error: { code: projects.error, message: 'synthetic read failure' } })
        : json({ ok: true, projects, view: 'team' });
    }
    if (url.includes('/api/team/overview')) return json(DEMO_OVERVIEW);
    if (options.shell !== undefined && url.includes('/api/team/shell')) return json(options.shell);
    return json({ ok: false, error: { code: 'NoRoute', message: 'unexpected synthetic read' } });
  }));
}

function rows() {
  return [...screen.getByTestId('projects-table').querySelectorAll('tbody tr.ant-table-row')];
}

function currentUpdate(status = 'ok', summary = 'Recorded current summary') {
  return {
    id: 'synthetic-current', startsOn: '2026-11-16', endsOn: '2026-11-22', status, summary,
    recordedAtUnixMs: NOW,
  };
}

afterEach(async () => {
  await act(async () => { await i18n.changeLanguage('zh-CN'); });
  vi.unstubAllGlobals();
});

describe('project navigation and reads', () => {
  it('encodes project keys in links without altering them', async () => {
    installFetch({ projects: [entry('demo /?', 'Encoded project')] });
    renderApp('/projects');
    const link = await screen.findByRole('link', { name: 'Encoded project' });
    expect(link.getAttribute('href')).toBe('/projects/demo%20%2F%3F');
  });

  it('uses the recorded readable project key when the read supplies no title', async () => {
    installFetch({ projects: [entry('support-workspace', null)] });
    renderApp('/projects');
    const link = await screen.findByRole('link', { name: 'support-workspace' });
    expect(link.getAttribute('href')).toBe('/projects/support-workspace');
  });

  it('keeps project tabs in their contractual order', async () => {
    stubFetch({ '/api/team/projects': DEMO_PROJECTS, '/api/team/overview': DEMO_OVERVIEW });
    renderApp('/projects/demo');
    await screen.findByRole('tab', { name: '概览' });
    expect(screen.getAllByRole('tab').map((tab) => tab.textContent)).toEqual([
      '概览', '工作计划', '依赖', '成果与验收', '项目知识',
    ]);
  });
});

describe('project list prototype and count details', () => {
  function installFourProjects() {
    const projects = [
      entry('project-a', 'Project Alpha'),
      entry('project-b', 'Project Beta'),
      entry('project-c', 'Project Gamma'),
      entry('project-d', 'Project Taken Over', {
        groupName: undefined,
        takeover: { resultId: 'synthetic-result', verifiedTasks: 0, workstreams: 4, questions: 4, recordedAtUnixMs: NOW - 30_000 },
      }),
    ];
    installFetch({
      projects,
      models: {
        'project-a': model('project-a', { periodUpdates: { coverage: 'complete', items: [currentUpdate('risk', 'A recorded risk')] } }),
        'project-b': model('project-b', { periodUpdates: { coverage: 'complete', items: [currentUpdate('ok', 'On recorded track')] } }),
        'project-c': model('project-c', { periodUpdates: { coverage: 'complete', items: [{
          ...currentUpdate('ok', 'An old judgement'), startsOn: '2026-11-02', endsOn: '2026-11-08', recordedAtUnixMs: NOW - 12 * DAY,
        }] } }),
      },
    });
    return projects;
  }

  it('renders the title, one-line summary, all columns, risk emphasis, late update and takeover fact', async () => {
    installFourProjects();
    renderApp('/projects');
    await screen.findByRole('link', { name: 'Project Alpha' });
    expect(screen.getByRole('heading', { level: 1, name: '项目' })).toBeTruthy();
    expect(screen.getByText('你能看到的 4 个')).toBeTruthy();
    expect(screen.getByTestId('projects-lead').textContent).toBe('4 个项目，1 个有风险，1 个没有更新，1 个刚接管');
    expect(screen.getByRole('heading', { level: 2, name: '全部项目' })).toBeTruthy();
    expect([...screen.getByTestId('projects-table').querySelectorAll('thead th')].map((cell) => cell.textContent)).toEqual([
      '项目', '状态', '一句话', '完成得怎么样', '更新', '',
    ]);
    expect(rows()).toHaveLength(4);
    expect(screen.getByRole('link', { name: 'Project Alpha' }).closest('tr')?.className).toContain('obs-table-highlighted-row');
    expect(screen.getByText('该更新了')).toBeTruthy();
    expect(screen.getByText('上一次更新是 12 天前')).toBeTruthy();
    const takeover = screen.getByRole('link', { name: '接管后还没有按验收核验过的任务' });
    expect(takeover.getAttribute('href')).toBe('/takeover/synthetic-result');
    expect(screen.getByText('读到 4 条工作线，按验收核验过的是 0 项，有 4 个问题要回答')).toBeTruthy();
    expect(screen.getByTestId('projects-section-count').textContent).toBe('4');
    expect(screen.getByTestId('freshness-badge')).toBeTruthy();
  });

  it('opens exactly the same number of detail rows as each clicked summary count', async () => {
    installFourProjects();
    const user = userEvent.setup();
    const { router } = renderApp('/projects');
    await screen.findByRole('link', { name: 'Project Alpha' });
    for (const [testId, detail, count] of [
      ['risk', 'risk', 1], ['no-update', 'no-update', 1], ['takeover', 'takeover', 1], ['all', 'all', 4],
    ] as const) {
      await user.click(screen.getByTestId(`projects-summary-${testId}-link`));
      expect(router.state.location.search).toBe(detail === 'all' ? '' : `?detail=${detail}`);
      expect(rows()).toHaveLength(count);
      expect(screen.getByTestId('projects-section-count').textContent).toBe(String(count));
    }
    await user.click(screen.getByTestId('projects-section-count-link'));
    expect(router.state.location.pathname).toBe('/projects');
    expect(rows()).toHaveLength(4);
  });

  it('opens a project from its row and from the keyboard', async () => {
    installFetch();
    const { router } = renderApp('/projects');
    const title = await screen.findByRole('link', { name: 'Project Alpha' });
    const row = title.closest('tr');
    expect(row).not.toBeNull();
    fireEvent.keyDown(row!, { key: 'Enter' });
    await waitFor(() => expect(router.state.location.pathname).toBe('/projects/project-a'));
    expect(screen.queryByRole('button', { name: /派发|认领|批准|复核|回答|合入/ })).toBeNull();
  });

  it('threads revocation through every observation read', () => {
    const source = readFileSync('src/pages/ProjectsPage.tsx', 'utf8');
    expect(source).toContain('projectsQueryOptions(revoked)');
    expect(source).toContain('overviewModelQueryOptions(entry.key, revoked)');
    expect(source).toContain('enabled: options.enabled && !entry.takeover');
  });

  it('clamps unknown detail, tab and chain parameters instead of selecting a bogus detail', async () => {
    installFourProjects();
    renderApp('/projects?detail=bogus&tab=zzz&chain=99');
    await screen.findByRole('link', { name: 'Project Alpha' });
    expect(rows()).toHaveLength(4);
    expect(screen.getByTestId('projects-section-count').textContent).toBe('4');
  });

  it('shows unknown summary numbers as a non-clickable dash and a known zero as zero', async () => {
    installFetch({
      projects: [entry('project-a', 'Project Alpha'), entry('project-b', 'Project Beta')],
      models: {
        'project-a': model('project-a', { periodUpdates: { coverage: 'complete', items: [currentUpdate('ok')] } }),
        'project-b': { error: 'ReadTimeout' },
      },
    });
    renderApp('/projects');
    await screen.findByRole('link', { name: 'Project Alpha' });
    expect(screen.getByTestId('projects-summary-risk').textContent).toBe('，— 个有风险');
    expect(screen.queryByTestId('projects-summary-risk-link')).toBeNull();
    expect(screen.getByTestId('projects-summary-no-update').textContent).toContain('—');
    expect(screen.getByTestId('projects-summary-takeover').textContent).toContain('0 个刚接管');
    expect(screen.getByTestId('projects-summary-takeover-link')).toBeTruthy();
  });

  it('keeps untrusted titles as truncated text, never markup or an exposed project key', async () => {
    const longTitle = 'Long synthetic project title '.repeat(12).trim();
    installFetch({ projects: [entry('secret-project-key', longTitle)] });
    renderApp('/projects');
    const link = await screen.findByRole('link', { name: longTitle });
    expect(link.className).toContain('projects-ellipsis');
    expect(link.getAttribute('title')).toBe(longTitle);
    expect(link.querySelector('b')).toBeNull();
    expect(screen.getByTestId('page-projects').textContent).not.toContain('secret-project-key');
    const css = readFileSync('src/pages/ProjectsPage.css', 'utf8');
    expect(css).toMatch(/\.projects-ellipsis\s*\{[^}]*-webkit-line-clamp:\s*1;[^}]*white-space:\s*normal;/s);
  });
  it('highlights only rows whose recorded health is risk', async () => {
    installFourProjects();
    renderApp('/projects');
    await screen.findByRole('link', { name: 'Project Alpha' });
    for (const [name, highlighted] of [
      ['Project Alpha', true], ['Project Beta', false], ['Project Gamma', false], ['Project Taken Over', false],
    ] as const) {
      expect(screen.getByRole('link', { name }).closest('tr')?.className.includes('obs-table-highlighted-row'), name).toBe(highlighted);
    }
  });

  it('does not show the zero-verification takeover link for three, null, or missing accepted counts', async () => {
    for (const verifiedTasks of [3, null, undefined]) {
      installFetch({
        projects: [entry('project-d', 'Project Taken Over', {
          takeover: { resultId: 'synthetic-result', verifiedTasks, workstreams: 4, questions: 4 },
        })],
      });
      const { unmount } = renderApp('/projects');
      await screen.findByRole('link', { name: 'Project Taken Over' });
      expect(screen.queryByRole('link', { name: '接管后还没有按验收核验过的任务' })).toBeNull();
      expect(screen.getByText('完成度还没有读全')).toBeTruthy();
      unmount();
    }
  });

  it('does not use another project\'s model for a mismatched response', async () => {
    installFetch({
      projects: [entry('project-a', 'Project Alpha'), entry('project-b', 'Project Beta')],
      models: {
        'project-a': model('project-b', {
          periodUpdates: { coverage: 'complete', items: [currentUpdate('risk', 'Mismatched project B risk')] },
        }),
        'project-b': model('project-b'),
      },
    });
    renderApp('/projects');
    const projectA = await screen.findByRole('link', { name: 'Project Alpha' });
    const rowA = projectA.closest('tr');
    expect(rowA?.textContent).not.toContain('Mismatched project B risk');
    expect(rowA?.textContent).toContain('结果未知');
    expect(rowA?.textContent).toContain('完成度还没有读全');
    expect(screen.getByTestId('projects-summary-risk').textContent).toBe('，— 个有风险');
    expect(screen.queryByTestId('projects-summary-risk-link')).toBeNull();
  });

  it('floors update-column elapsed time in whole days', async () => {
    installFetch({
      projects: [entry('project-a', 'Project Alpha'), entry('project-b', 'Project Beta')],
      models: {
        'project-a': model('project-a', {
          periodUpdates: { coverage: 'complete', items: [{ ...currentUpdate('ok'), recordedAtUnixMs: NOW - 2.6 * DAY }] },
        }),
        'project-b': model('project-b', {
          periodUpdates: { coverage: 'complete', items: [{ ...currentUpdate('ok'), recordedAtUnixMs: NOW - 0.9 * DAY }] },
        }),
      },
    });
    renderApp('/projects');
    const projectA = await screen.findByRole('link', { name: 'Project Alpha' });
    expect(projectA.closest('tr')?.querySelector('td:nth-child(5)')?.textContent).toBe('2 天前');
    expect(screen.getByRole('link', { name: 'Project Beta' }).closest('tr')?.querySelector('td:nth-child(5)')?.textContent).toBe('今天');
  });

  it('picks the current period in the deployment\'s pinned display zone, not the viewer\'s', async () => {
    // The derivation clock is 2026-11-19 04:00 in UTC and still the evening of the 18th in Los
    // Angeles, so a period that starts on the 19th is current in one zone and not yet begun in the
    // other. The shell read pins the zone; the viewer's own zone must not decide it.
    const startsOnThe19th = {
      ...currentUpdate('ok', 'Period that starts on the 19th'),
      startsOn: '2026-11-19', endsOn: '2026-11-19', recordedAtUnixMs: NOW - 60 * 60_000,
    };
    const rowTextUnder = async (zone: string, expectation: (text: string) => void) => {
      installFetch({
        models: { 'project-a': model('project-a', { periodUpdates: { coverage: 'complete', items: [startsOnThe19th] } }) },
        shell: { ok: true, source: 'demo', organizationName: null, perspective: null, displayTimeZone: zone },
      });
      const { queryClient, unmount } = renderApp('/projects');
      await screen.findByRole('link', { name: 'Project Alpha' });
      await waitFor(() => expect(queryClient.getQueryState(queryKeys.shell)?.status).toBe('success'));
      await waitFor(() => expectation(rows()[0]?.textContent ?? ''));
      unmount();
    };
    await rowTextUnder('UTC', (text) => expect(text).toContain('Period that starts on the 19th'));
    await rowTextUnder('America/Los_Angeles', (text) => expect(text).not.toContain('Period that starts on the 19th'));
  });

  it('removes a trailing Chinese full stop from the recorded one-sentence update', async () => {
    installFetch({
      projects: [entry('project-a', 'Project Alpha'), entry('project-b', 'Project Beta')],
      models: {
        'project-a': model('project-a', {
          periodUpdates: { coverage: 'complete', items: [currentUpdate('ok', '本周接口联调完成。')] },
        }),
        'project-b': model('project-b', {
          periodUpdates: { coverage: 'complete', items: [currentUpdate('ok', '本周接口联调完成')] },
        }),
      },
    });
    renderApp('/projects');
    const projectA = await screen.findByRole('link', { name: 'Project Alpha' });
    expect(projectA.closest('tr')?.querySelector('td:nth-child(3)')?.textContent).toBe('本周接口联调完成');
    expect(screen.getByRole('link', { name: 'Project Beta' }).closest('tr')?.querySelector('td:nth-child(3)')?.textContent).toBe('本周接口联调完成');
  });
});

describe('seven designed read states in Chinese and English', () => {
  const cases = [
    { state: 'data', testId: 'page-projects', zh: '全部项目', en: 'All projects' },
    { state: 'loading', testId: 'projects-loading', zh: '正在读取项目', en: 'Loading projects' },
    { state: 'empty', testId: 'projects-empty', zh: '还没有可查看的项目', en: 'No projects are available to view' },
    { state: 'error', testId: 'projects-read-error', zh: '项目清单暂时读取不到', en: 'Projects cannot be loaded right now' },
    { state: 'partial', testId: 'projects-partial', zh: '部分项目的事实暂时读取不到', en: 'Some project facts cannot be loaded' },
    { state: 'revoked', testId: 'state-forbidden', zh: '暂时无法查看', en: 'Access unavailable' },
    { state: 'unknown', testId: 'projects-unknown', zh: '部分结果还不能确定', en: 'Some outcomes are not known yet' },
  ] as const;

  it.each(cases.flatMap((item) => (['zh-CN', 'en-US'] as const).map((language) => ({ ...item, language }))))(
    '$state has designed product language in $language',
    async ({ state, testId, zh, en, language }) => {
      await act(async () => { await i18n.changeLanguage(language); });
      if (state === 'data') installFetch();
      if (state === 'loading') installFetch({ hold: true });
      if (state === 'empty') installFetch({ projects: [] });
      if (state === 'error') installFetch({ projects: { error: 'ReadTimeout' } });
      if (state === 'partial') installFetch({
        projects: [entry('project-a', 'Project Alpha'), entry('project-b', 'Project Beta')],
        models: { 'project-a': model('project-a'), 'project-b': { error: 'ReadTimeout' } },
      });
      if (state === 'revoked') installFetch({ revoked: true });
      if (state === 'unknown') installFetch({
        models: { 'project-a': model('project-a', { periodUpdates: { coverage: 'unread', reason: 'read-failed' } }) },
      });
      renderApp('/projects');
      const element = await screen.findByTestId(testId);
      await waitFor(() => expect(element.textContent).toContain(language === 'zh-CN' ? zh : en));
      expect(element.textContent).not.toMatch(/undefined|null|NaN|ReadTimeout|Unauthenticated|periodUpdates|recordedAtUnixMs/);
      if (state !== 'revoked') expect(screen.getByTestId('freshness-badge')).toBeTruthy();
    },
  );
});
