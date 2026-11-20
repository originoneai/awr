import { readFileSync } from 'node:fs';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { createMemoryRouter, RouterProvider } from 'react-router';
import { afterEach, describe, expect, it, vi } from 'vitest';
import i18n from '../i18n';
import enUS from '../i18n/en-US';
import zhCN from '../i18n/zh-CN';
import { defaultVisibleText, findHumanLanguageViolations } from '../lib/human-language-guard';
import { revokeObservationReads } from '../lib/queries';
import { routes } from '../router';
import { demoOverviewModelResponse } from '../test-fixtures/overview-model';
import { DEMO_PROJECTS, renderApp } from '../test-utils';

const NOW = 1_795_089_600_000;
const ROLES = ['observer', 'developer', 'reviewer', 'supervisor', 'deliverer', 'administrator'];
const DIRECTORY = {
  configured: true,
  organization: '示例组织',
  departments: [{ key: 'group-app', name: '应用小组' }, { key: 'group-platform', name: '平台小组' }],
  updated_at_ms: NOW,
  members: [
    { name: '成员甲', available: true, departments: [{ key: 'group-app', name: '应用小组' }] },
    { name: '成员乙', available: true, departments: [{ key: 'group-app', name: '应用小组' }] },
    { name: '成员丙', available: true, departments: [{ key: 'group-app', name: '应用小组' }] },
    { name: '成员丁', available: true, departments: [{ key: 'group-platform', name: '平台小组' }] },
  ],
};

function json(payload: unknown) {
  return new Response(JSON.stringify(payload), { status: 200, headers: { 'content-type': 'application/json' } });
}

function capabilities() {
  return {
    ok: true,
    result: {
      identity: { actor_id: 'synthetic-actor', role: 'reader', business_roles: ['observer'] },
      // This is the verbatim live-sample shape. It names duties but has no permission matrix.
      business_role_presets: {
        explicit_combined_duties: true,
        omitted_on_update: 'preserve_existing_declaration',
        role_names_grant_authority: false,
        roles: ROLES,
        semantics: 'membership_grants_intersect_declared_ceiling_then_one_live_delegation',
      },
    },
  };
}

interface SetupOptions {
  projects?: unknown;
  directory?: unknown;
  overview?: unknown;
  capabilities?: unknown;
  pending?: boolean;
  overviewGate?: Promise<void>;
  capabilitiesFailure?: { current: boolean };
}

function setupSettings(options: SetupOptions = {}) {
  const calls: string[] = [];
  const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = String(input);
    calls.push(url);
    if (options.pending && !url.includes('/api/team/projects')) return new Promise<Response>(() => {});
    if (url.includes('/api/team/projects')) return json(options.projects ?? DEMO_PROJECTS);
    if (url.includes('/api/team/directory')) return json(options.directory ?? { ok: true, data: DIRECTORY });
    if (url.includes('/api/team/overview-model')) {
      if (options.overviewGate) await options.overviewGate;
      return json(options.overview ?? demoOverviewModelResponse());
    }
    if (url.includes('/api/team/read')) {
      const query = JSON.parse(String(init?.body)).query;
      if (query.op === 'capabilities') {
        if (options.capabilitiesFailure?.current) return json({ ok: false, error: { code: 'Unavailable', message: 'private synthetic failure' } });
        return json(options.capabilities ?? capabilities());
      }
    }
    return json({ ok: false, error: { code: 'NoRoute', message: 'Unexpected synthetic route' } });
  });
  vi.stubGlobal('fetch', fetchMock);
  return { calls, fetchMock };
}

async function changeLanguage(language: string) {
  await act(async () => { await i18n.changeLanguage(language); });
}

function normalized(text: string | null) {
  return (text ?? '').replace(/\s+/g, ' ');
}

function sectionTitles(container: HTMLElement) {
  return [...container.querySelectorAll('.obs-section-title')].map((element) => element.textContent);
}

afterEach(async () => {
  vi.unstubAllGlobals();
  vi.useRealTimers();
  await changeLanguage('zh-CN');
});

describe('settings prototype structure and exact static copy', () => {
  it('renders the heading, read-only notice, existing-page links and all static sections in prototype order', async () => {
    setupSettings();
    renderApp('/settings?scope=demo');
    const page = await screen.findByTestId('page-settings');
    await within(page).findByTestId('settings-organization-tree');
    expect(within(page).getByRole('heading', { name: '管理设置', level: 1 })).toBeTruthy();
    expect(page.querySelector('.obs-page-subtitle')?.textContent).toBe('示例组织');
    const notice = within(page).getByTestId('settings-scope-notice');
    expect(normalized(notice.textContent)).toBe('这个网页管什么：看，是全部。能改的只有账号、Agent 连接、成员和职责这几样。任务上的事——接任务、回答、复核、改计划——都由各人在自己的 Agent 里做，网页不提交。');
    expect(within(notice).getByRole('link', { name: '账户与连接' }).getAttribute('href')).toBe('/account?scope=demo');
    expect(within(notice).getByRole('link', { name: '连接我的 Agent' }).getAttribute('href')).toBe('/agent-connect?scope=demo');
    expect(sectionTitles(within(page).getByTestId('settings-left') as HTMLElement)).toEqual(['六种职责和各自能做的事', '状态怎么读']);
    expect(sectionTitles(within(page).getByTestId('settings-right') as HTMLElement)).toEqual(['组织和部门', '页面用词', '什么算依据']);
    expect(within(page).getByTestId('settings-duty-note').textContent).toBe('一个人可以有多个职责。提交的人不能复核自己的成果，这一条由服务端把关，不靠页面。');
    expect(within(page).getByTestId('settings-organization-note').textContent).toBe('现在的成员目录里，部门是平铺的一层。“小组属于哪个部门”还没有，部门和最高层那两页的汇总要靠它。');
    expect(within(page).getByTestId('settings-vocabulary-note').textContent).toBe('其他行业只留了这个位置：换一套词，页面结构不变。现在不做。');
    expect(within(page).getByTestId('settings-vocabulary').textContent).toContain('软件研发');
    expect(within(page).getByTestId('settings-vocabulary').textContent).toContain('任务、完成标准、复核、工作线');
    expect(within(page).getByTestId('settings-evidence').textContent).toContain('测试结果');
    expect(within(page).getByTestId('settings-evidence').textContent).toContain('绑定到代码版本');
    expect(within(page).getByTestId('settings-evidence').textContent).toContain('复核通过');
    expect(within(page).getByTestId('settings-evidence').textContent).toContain('复核人不是提交人');
    expect(within(page).getByTestId('settings-evidence').textContent).toContain('人审通过一份文件');
    expect(within(page).getByTestId('settings-evidence').textContent).toContain('给不写代码的工作用');
    expect(within(page).getAllByText('在用')).toHaveLength(3);
    expect(within(page).getByText('预留')).toBeTruthy();
    expect(within(page).getByTestId('freshness-badge')).toBeTruthy();
  });

  it('renders every prototype task and health status with its exact meaning', async () => {
    setupSettings();
    renderApp('/settings?scope=demo');
    const table = (await screen.findByTestId('settings-status-table')).querySelector('table') as HTMLTableElement;
    const rows = [...table.querySelectorAll('tbody tr')].map((row) => [...row.querySelectorAll('td')].map((cell) => cell.textContent));
    expect(rows).toEqual([
      ['已核验', '每条完成标准都有测试结果或复核记录对着，成果已经合入'],
      ['标了完成，没核验', 'Agent 报告说做完了，但没有提交复核，所以不算完成'],
      ['复核退回', '复核的人指出了必须改的问题'],
      ['等复核', '成果已经提交，在等一位不是提交人的人来复核'],
      ['在做', '有 Agent 正在推进，最近有报告'],
      ['停着', 'Agent 停下了，在等一个回答'],
      ['在等', '在等别的任务或者项目外的一方'],
      ['可以开始', '完成标准写好了，还没有人开工'],
      ['没开始', '排在计划里，前面的事还没做完'],
      ['正常', '这一期按计划在走'],
      ['有风险', '有事卡住了，可能影响承诺'],
      ['偏离', '已经确定赶不上承诺'],
      ['没有更新', '超过约定的时间没有人写更新'],
    ]);
    expect(table.querySelectorAll('.obs-tag')).toHaveLength(13);
    expect(table.querySelectorAll('tbody tr')[0].querySelector('.obs-tag')?.getAttribute('title')).toBe('每条完成标准都有测试结果或复核记录对着，成果已经合入');
  });

  it('uses service duty names, never a hardcoded permission matrix, and shows the missing-definition state', async () => {
    setupSettings();
    renderApp('/settings?scope=demo');
    const duty = await screen.findByTestId('settings-duty-table');
    await waitFor(() => expect([...duty.querySelectorAll('thead th')].map((header) => header.textContent))
      .toEqual(['', '只读', '开发', '复核', '主管', '交付', '管理员']));
    expect(within(duty).getByTestId('settings-duty-definitions-missing').textContent).toContain('还没有读到职责定义');
    for (const ability of ['看项目和任务', '接任务、推进、提交成果', '复核别人的成果', '派工，批准和发布拆解', '管理成员和权限']) {
      expect(duty.textContent).not.toContain(ability);
    }
    expect(duty.textContent).not.toContain('observer');
    expect(duty.textContent).not.toContain('business_role_presets');
  });

  it('renders only directory organization facts and known member counts', async () => {
    setupSettings();
    renderApp('/settings?scope=demo');
    const organization = await screen.findByTestId('settings-organization-tree');
    expect(organization.textContent).toContain('示例组织');
    expect(organization.textContent).toContain('应用小组 · 3 人');
    expect(organization.textContent).toContain('平台小组 · 1 人');
    expect(organization.textContent).not.toContain('研发部');
    expect(organization.textContent).not.toContain('交付小组');
  });

  it('escapes untrusted department names and encodes both existing-page links', async () => {
    const project = 'project/with ?';
    const markup = '<img src=x onerror=alert(1)>';
    const longName = '很长的部门名称'.repeat(20);
    setupSettings({
      projects: { ...DEMO_PROJECTS, projects: [{ key: project, title: 'Synthetic project' }] },
      directory: { ok: true, data: { ...DIRECTORY, organization: markup, departments: [{ key: 'group', name: longName }], members: [] } },
    });
    renderApp('/settings');
    const organization = await screen.findByTestId('settings-organization-tree');
    expect(organization.textContent).toContain('组织名称未知');
    expect(organization.textContent).toContain(longName);
    expect(organization.textContent).not.toContain(markup);
    expect(organization.querySelector('img')).toBeNull();
    expect(organization.querySelector('.settings-org-node-label')?.classList.contains('settings-org-node-label')).toBe(true);
    expect(organization.querySelector('.settings-org-node-label[data-truncate="true"]')).not.toBeNull();
    expect(screen.getByRole('link', { name: '账户与连接' }).getAttribute('href')).toBe(`/account?scope=${encodeURIComponent(project)}`);
    expect(screen.getByRole('link', { name: '连接我的 Agent' }).getAttribute('href')).toBe(`/agent-connect?scope=${encodeURIComponent(project)}`);
  });
});

describe('settings read failures and revocation', () => {
  it('drops previously loaded duty names as soon as their read fails', async () => {
    const failure = { current: false };
    setupSettings({ capabilitiesFailure: failure });
    const { queryClient } = renderApp('/settings?scope=demo');
    const loadedDuty = await screen.findByTestId('settings-duty-table');
    await waitFor(() => expect(loadedDuty.textContent).toContain('只读'));
    failure.current = true;
    await act(async () => { await queryClient.refetchQueries({ queryKey: ['team', 'capabilities', 'demo'] }); });
    await waitFor(() => expect(screen.getByTestId('settings-duty-table').textContent).toContain('职责定义读取失败'));
    expect(screen.getByTestId('settings-duty-table').textContent).not.toContain('只读');
    expect(screen.getByTestId('settings-duty-table').textContent).not.toContain('private synthetic failure');
  });

  it.each(['zh-CN', 'en-US'])('shows loading, empty, total failure, partial failure and unknown states in %s', async (language) => {
    await changeLanguage(language);
    setupSettings({ pending: true });
    const loading = renderApp('/settings?scope=demo');
    expect((await screen.findByTestId('page-settings')).getAttribute('data-state')).toBe('loading');
    expect(screen.getAllByTestId('settings-loading').length).toBeGreaterThan(0);
    expect(screen.getByTestId('freshness-badge')).toBeTruthy();
    loading.unmount();

    setupSettings({ directory: { ok: true, data: { configured: false, organization: null, departments: [], members: [], updated_at_ms: null } } });
    const empty = renderApp('/settings?scope=demo');
    expect((await screen.findByTestId('settings-organization-empty')).textContent).toBe(i18n.t('settings.organizationEmpty'));
    expect(screen.getByTestId('page-settings').getAttribute('data-state')).toBe('empty');
    empty.unmount();

    setupSettings({ projects: { ...DEMO_PROJECTS, projects: [] } });
    const noProjects = renderApp('/settings');
    expect((await screen.findByTestId('settings-duty-empty')).textContent).toBe(i18n.t('settings.dutiesEmpty'));
    expect(screen.getByTestId('settings-organization-empty')).toBeTruthy();
    expect(screen.getByTestId('page-settings').getAttribute('data-state')).toBe('empty');
    noProjects.unmount();

    const failedRead = { ok: false, error: { code: 'Unavailable', message: 'private synthetic failure' } };
    setupSettings({ capabilities: failedRead, directory: failedRead, overview: failedRead });
    const failed = renderApp('/settings?scope=demo');
    await screen.findByTestId('settings-read-failed');
    expect(screen.getByTestId('page-settings').getAttribute('data-state')).toBe('failed');
    expect(screen.getByTestId('settings-duty-read-failed')).toBeTruthy();
    expect(screen.getByTestId('settings-organization-read-failed')).toBeTruthy();
    expect(document.body.textContent).not.toContain('private synthetic failure');
    expect(document.body.textContent).not.toContain('Unavailable');
    failed.unmount();

    setupSettings({ projects: failedRead });
    const discoveryFailed = renderApp('/settings');
    await screen.findByTestId('settings-read-failed');
    expect(screen.getByTestId('settings-duty-read-failed')).toBeTruthy();
    expect(screen.queryByTestId('settings-loading')).toBeNull();
    expect(screen.getByTestId('page-settings').getAttribute('data-state')).toBe('failed');
    discoveryFailed.unmount();

    setupSettings({ capabilities: failedRead });
    const partial = renderApp('/settings?scope=demo');
    await screen.findByTestId('settings-read-partial');
    expect(screen.getByTestId('page-settings').getAttribute('data-state')).toBe('partial');
    expect(screen.getByTestId('settings-organization-tree').textContent).toContain('示例组织');
    expect(screen.getByTestId('settings-duty-read-failed')).toBeTruthy();
    partial.unmount();

    setupSettings({ overview: demoOverviewModelResponse(undefined, { ordering: 'incomparable' }) });
    renderApp('/settings?scope=demo');
    await screen.findByTestId('settings-read-unknown');
    expect(screen.getByTestId('page-settings').getAttribute('data-state')).toBe('unknown');
    expect(screen.getByTestId('freshness-badge')).toBeTruthy();
  });

  it.each(['zh-CN', 'en-US'])('clears all records and stops every read after revocation in %s', async (language) => {
    await changeLanguage(language);
    const { fetchMock } = setupSettings();
    const { queryClient } = renderApp('/settings?scope=demo');
    expect(await screen.findByTestId('settings-organization-tree')).toBeTruthy();
    const before = fetchMock.mock.calls.length;
    revokeObservationReads(queryClient);
    expect(await screen.findByTestId('state-forbidden')).toBeTruthy();
    expect(screen.queryByTestId('page-settings')).toBeNull();
    expect(document.body.textContent).not.toContain('示例组织');
    expect(document.body.textContent).not.toContain('应用小组');
    expect(fetchMock.mock.calls.length).toBe(before);
  });

  it('allows a read retry without adding any task business action', async () => {
    const failedRead = { ok: false, error: { code: 'Unavailable' } };
    setupSettings({ capabilities: failedRead, directory: failedRead, overview: failedRead });
    renderApp('/settings?scope=demo');
    const notice = await screen.findByTestId('settings-read-failed');
    await userEvent.setup().click(within(notice).getByRole('button', { name: '重新读取' }));
    expect(screen.queryByRole('button', { name: /派发|认领|批准|复核|回答|合入/ })).toBeNull();
  });
});

describe('settings review-round regressions', () => {
  it('treats a directory-only failure as partial and offers retry', async () => {
    setupSettings({ directory: { ok: false, error: { code: 'Unavailable', message: 'private directory failure' } } });
    renderApp('/settings?scope=demo');
    const notice = await screen.findByTestId('settings-read-partial');
    expect(screen.getByTestId('page-settings').getAttribute('data-state')).toBe('partial');
    expect(screen.getByTestId('settings-organization-read-failed')).toBeTruthy();
    expect(within(notice).getByRole('button', { name: '重新读取' })).toBeTruthy();
    expect(document.body.textContent).not.toContain('private directory failure');
  });

  it('treats an unauthenticated directory read as revoked and clears content', async () => {
    setupSettings({ directory: { ok: false, error: { code: 'Unauthenticated', message: 'revoked synthetic directory' } } });
    // Isolate the page guard from QueryCache's global session-expiry fallback: both paths
    // must clear protected content, and this test must fail if the directory check is removed.
    const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const router = createMemoryRouter(routes, { initialEntries: ['/settings?scope=demo'] });
    render(<QueryClientProvider client={queryClient}><RouterProvider router={router} /></QueryClientProvider>);
    expect(await screen.findByTestId('state-forbidden')).toBeTruthy();
    expect(screen.queryByTestId('page-settings')).toBeNull();
    expect(document.body.textContent).not.toContain('示例组织');
    expect(document.body.textContent).not.toContain('revoked synthetic directory');
  });

  it('keeps the page loading while only the overview model is pending', async () => {
    setupSettings({ overviewGate: new Promise<void>(() => {}) });
    renderApp('/settings?scope=demo');
    expect(await screen.findByTestId('settings-organization-tree')).toBeTruthy();
    expect(await screen.findByTestId('settings-duty-definitions-missing')).toBeTruthy();
    expect(screen.getByTestId('page-settings').getAttribute('data-state')).toBe('loading');
  });

  it('refetches projects capabilities overview model and directory on retry', async () => {
    const failedRead = { ok: false, error: { code: 'Unavailable', message: 'private directory failure' } };
    const { calls } = setupSettings({ directory: failedRead });
    renderApp('/settings?scope=demo');
    const notice = await screen.findByTestId('settings-read-partial');
    calls.length = 0;
    await userEvent.setup().click(within(notice).getByRole('button', { name: '重新读取' }));
    const count = (path: string) => calls.filter((url) => url.includes(path)).length;
    await waitFor(() => {
      expect(count('/api/team/projects')).toBe(1);
      expect(count('/api/team/read')).toBe(1);
      expect(count('/api/team/overview-model')).toBe(1);
      expect(count('/api/team/directory')).toBe(1);
    });
  });

  it('reports partial coverage when duty definitions are missing', async () => {
    setupSettings();
    renderApp('/settings?scope=demo');
    expect(await screen.findByTestId('settings-duty-definitions-missing')).toBeTruthy();
    expect(screen.getByTestId('settings-read-coverage').textContent).toBe(i18n.t('settings.coveragePartial'));
    expect(screen.getByTestId('settings-read-coverage').textContent).not.toBe(i18n.t('settings.coverageComplete'));
  });

  it('collects departments that appear only on members', async () => {
    const memberOnly = { key: 'member-only', name: '成员专属小组' };
    setupSettings({
      directory: {
        ok: true,
        data: {
          ...DIRECTORY,
          departments: [DIRECTORY.departments[0]],
          members: [{ name: '成员戊', available: true, departments: [memberOnly] }],
        },
      },
    });
    renderApp('/settings?scope=demo');
    const organization = await screen.findByTestId('settings-organization-tree');
    expect(organization.textContent).toContain('成员专属小组 · 1 人');
  });

  it('marks only organization and department names longer than 80 characters for truncation', async () => {
    const longOrganization = `${'示例组织'.repeat(20)}长`;
    const longDepartment = `${'应用小组'.repeat(20)}长`;
    setupSettings({
      directory: { ok: true, data: { ...DIRECTORY, organization: longOrganization, departments: [{ key: 'long', name: longDepartment }], members: [] } },
    });
    const longView = renderApp('/settings?scope=demo');
    const longTree = await screen.findByTestId('settings-organization-tree');
    const longLabels = longTree.querySelectorAll('.settings-org-node-label');
    expect(longLabels[0].getAttribute('data-truncate')).toBe('true');
    expect(longLabels[0].textContent).toBe(longOrganization);
    expect(longLabels[1].getAttribute('data-truncate')).toBe('true');
    expect(longLabels[1].textContent).toContain(longDepartment);
    longView.unmount();

    const boundaryOrganization = '示例组织'.repeat(20);
    const boundaryDepartment = '应用小组'.repeat(20);
    setupSettings({
      directory: { ok: true, data: { ...DIRECTORY, organization: boundaryOrganization, departments: [{ key: 'boundary', name: boundaryDepartment }], members: [] } },
    });
    renderApp('/settings?scope=demo');
    const boundaryTree = await screen.findByTestId('settings-organization-tree');
    const boundaryLabels = boundaryTree.querySelectorAll('.settings-org-node-label');
    expect(boundaryLabels[0].getAttribute('data-truncate')).toBeNull();
    expect(boundaryLabels[0].textContent).toBe(boundaryOrganization);
    expect(boundaryLabels[1].getAttribute('data-truncate')).toBeNull();
    expect(boundaryLabels[1].textContent).toContain(boundaryDepartment);
  });

  it('does not render empty role names as duty columns', async () => {
    const response = capabilities();
    setupSettings({
      capabilities: {
        ...response,
        result: {
          ...response.result,
          business_role_presets: { ...response.result.business_role_presets, roles: ['', 'observer'] },
        },
      },
    });
    renderApp('/settings?scope=demo');
    const duty = await screen.findByTestId('settings-duty-table');
    await waitFor(() => expect([...duty.querySelectorAll('thead th')].map((header) => header.textContent))
      .toEqual(['', '只读']));
  });
});

describe('settings vocabulary and source craft guards', () => {
  it('has identical Chinese and English settings keys', () => {
    const flatten = (value: unknown, prefix = ''): string[] => Object.entries(value as Record<string, unknown>)
      .flatMap(([key, child]) => typeof child === 'string' ? [`${prefix}${key}`] : flatten(child, `${prefix}${key}.`));
    expect(flatten(enUS.settings).sort()).toEqual(flatten(zhCN.settings).sort());
  });

  it.each(['zh-CN', 'en-US'])('keeps visible copy, titles and accessibility labels in product language in %s', async (language) => {
    await changeLanguage(language);
    setupSettings();
    renderApp('/settings?scope=demo');
    await screen.findByTestId('settings-organization-tree');
    // The legacy scanner calls the mandatory prototype word “绑定” engineering copy and the
    // ordinary English word “completion” an engineering token. Neither is an internal identifier.
    const violations = findHumanLanguageViolations(document.body)
      .filter((violation) => violation.match !== '绑定' && violation.match !== 'completion');
    expect(violations).toEqual([]);
    expect(defaultVisibleText(document.body)).not.toMatch(/\b(?:undefined|null|NaN|business_role_presets|membership_action_ceiling|protocol_version|parent_id|project_id)\b/);
  });

  it('keeps inline styles, raw colors and non-token dimensions out of production source', () => {
    const page = readFileSync('src/pages/SettingsPage.tsx', 'utf8');
    const css = readFileSync('src/pages/SettingsPage.css', 'utf8').replace(/\/\*[\s\S]*?\*\//g, '');
    expect(page).not.toMatch(/\bstyle\s*=/);
    for (const [file, source] of [['SettingsPage.tsx', page], ['SettingsPage.css', css]] as const) {
      expect(source, `${file} has a raw color`).not.toMatch(/#[\da-f]{3,8}\b|\b(?:rgba?|hsla?)\s*\(/i);
      expect(source, `${file} has an untokenized string dimension`).not.toMatch(/['"]\d+(?:px|rem|em|vh|vw)['"]/i);
    }
    const withoutTokenDeclarations = css.replace(/--settings-[\w-]+\s*:\s*[^;]+;/g, '').replace(/@media[^\{]+/g, '').replace(/\b1px\b/g, '');
    expect(withoutTokenDeclarations).not.toMatch(/(?<![\w.-])-?(?:\d+\.\d+|\d+|\.\d+)(?:px|rem|em|vh|vw)\b/i);
    expect(css).toMatch(/\.settings-org-node-label\[data-truncate="true"\]\s*\{[^}]*text-overflow:\s*ellipsis[^}]*white-space:\s*nowrap/s);
    expect(css).toMatch(/\.settings-page \.obs-data-table table\s*\{[^}]*border-collapse:\s*collapse/s);
    expect(css).toMatch(/\.settings-page \.obs-data-table \.ant-table-tbody > tr:last-child > td\s*\{[^}]*border-bottom:\s*0/s);
  });
});
