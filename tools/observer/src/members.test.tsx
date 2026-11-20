import { readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { act, fireEvent, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import i18n from './i18n';
import { humanText } from './i18n/human-language';
import { findHumanLanguageViolations } from './lib/human-language-guard';
import { revokeObservationReads } from './lib/queries';
import type { TeamOverviewModelResponse } from './lib/types';
import type { ReadSlice } from './core/overview';
import { DEMO_PROJECTS, mockViewport, renderApp } from './test-utils';

declare module 'vitest' {
  interface Assertion {
    toHaveTextContent(expected: string): void;
  }
}

expect.extend({
  toHaveTextContent(received: HTMLElement | null, expected: string) {
    const actual = received?.textContent ?? '';
    return {
      pass: actual.includes(expected),
      message: () => `Expected ${JSON.stringify(actual)} to contain ${JSON.stringify(expected)}`,
    };
  },
  toHaveAttribute(received: Element | null, name: string, value?: string) {
    const actual = received?.getAttribute(name);
    const asymmetric = value as unknown as { asymmetricMatch?: (actual: string | null) => boolean };
    const pass = value === undefined ? received?.hasAttribute(name) === true
      : typeof asymmetric.asymmetricMatch === 'function' ? asymmetric.asymmetricMatch(actual ?? null) : actual === value;
    return { pass, message: () => `Expected attribute ${name} to match ${String(value)}; received ${String(actual)}` };
  },
});

const NOW = 1794600000000;
const nodeRequire = createRequire(import.meta.url);
const { projectDirectory } = nodeRequire('../server/directory.js') as { projectDirectory: (value: unknown) => unknown };
const DIRECTORY = {
  configured: true,
  organization: 'Example company',
  departments: [{ key: 'engineering', name: 'Engineering' }],
  updated_at_ms: NOW,
  members: [
    { personId: 'person-taylor', name: 'Taylor', available: true, departments: [{ key: 'engineering', name: 'Engineering' }] },
    { personId: 'person-riley', name: 'Riley', available: true, departments: [{ key: 'research', name: 'Research' }] },
    { personId: 'person-unavailable', name: null, available: false, departments: [] },
  ],
};

function recordedIdentityDirectory() {
  const sample = JSON.parse(readFileSync('test/fixtures/directory-web-read-identities.json', 'utf8')) as { data: unknown };
  const directory = projectDirectory(sample.data);
  expect(directory).not.toBeNull();
  return directory;
}

function overviewModel(): TeamOverviewModelResponse {
  const complete = <T,>(items: T[] = []): ReadSlice<T> => ({ coverage: 'complete', items });
  const tasks = [
    {
      id: 'task / ?', title: 'A very long task title that must remain readable without widening the member table',
      ownerPersonId: 'person-taylor', agentName: 'Claude desktop', projectId: 'demo', groupId: 'engineering',
      started: true, agentActive: true, externalWait: null, acceptance: [],
      report: { completed: false, reportedAtUnixMs: NOW - 1000 },
    },
    {
      id: 'planned-task', title: 'Recorded preparation', ownerPersonId: 'person-riley', agentName: 'Codex',
      projectId: 'demo', groupId: 'research', started: false, agentActive: false, externalWait: null, acceptance: [],
    },
  ];
  const approvals = ['first', 'second', 'third'].map((id) => ({
    id, kind: 'plan-approval' as const, recipientPersonId: 'person-taylor', projectId: 'demo', title: `Pending ${id}`,
  }));
  return {
    ok: true,
    project: 'demo',
    read_at_unix_ms: NOW,
    input: {
      tasks: complete(tasks),
      dependencies: complete(),
      decisions: complete(),
      reviewRounds: complete(),
      handoffs: complete(),
      approvals: complete(approvals),
      members: complete([
        { personId: 'person-taylor', displayName: 'Taylor', groupId: 'engineering', actorIds: ['actor-taylor'] },
        { personId: 'person-riley', displayName: 'Riley', groupId: 'research', actorIds: ['actor-riley'] },
      ]),
      activity: complete(),
      options: {
        nowUnixMs: NOW,
        stalledAfterDays: 3,
        views: { me: { personId: 'person-taylor', projectId: 'demo' } },
      },
    },
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
  };
}

function identityOverviewModel(): TeamOverviewModelResponse {
  const model = overviewModel();
  const task = model.input.tasks.items![0];
  model.input.members = { coverage: 'complete', items: [
    { personId: 'directory-person', displayName: 'Project coordinator', groupId: 'engineering' },
    { personId: 'directory-person-two', displayName: 'Project coordinator', groupId: 'research' },
  ] };
  model.input.tasks = { coverage: 'complete', items: [
    { ...task, id: 'identity-first', title: 'First recorded task', ownerPersonId: 'directory-person', groupId: 'engineering' },
    { ...task, id: 'identity-second', title: 'Second recorded task', ownerPersonId: 'directory-person-two', groupId: 'research' },
  ] };
  model.input.decisions = { coverage: 'complete', items: [
    { id: 'identity-pending-first', taskId: 'identity-first', state: 'open', current: true,
      question: 'First pending question', allowedResponderIds: ['directory-person'], createdAtUnixMs: NOW },
    { id: 'identity-pending-second-a', taskId: 'identity-second', state: 'open', current: true,
      question: 'Second pending question A', allowedResponderIds: ['directory-person-two'], createdAtUnixMs: NOW },
    { id: 'identity-pending-second-b', taskId: 'identity-second', state: 'open', current: true,
      question: 'Second pending question B', allowedResponderIds: ['directory-person-two'], createdAtUnixMs: NOW },
  ] };
  model.input.approvals = { coverage: 'complete', items: [] };
  return model;
}

/** Three open decisions waiting on Taylor's answer: the members table counts exactly these
 * blocking waits, while the workbench inbox would also count her three confirmations. */
function withTaylorDecisionWaits(model: TeamOverviewModelResponse): TeamOverviewModelResponse {
  model.input.decisions = { coverage: 'complete', items: [1, 2, 3].map((number) => ({
    id: `decision-${number}`, taskId: 'task / ?', state: 'open', current: true,
    question: `Blocking question ${number}`, allowedResponderIds: ['person-taylor'], createdAtUnixMs: NOW,
  })) };
  return model;
}

function setup() {
  let directoryResponse: unknown = { ok: true, data: DIRECTORY };
  let overviewResponse: unknown = overviewModel();
  let connectionsResponse: unknown = {
    ok: true,
    data: {
      items: [
        { connection_id: `agent_${'1'.repeat(64)}`, kind: 'agent', client_name: 'Taylor · Claude desktop', state: 'active', can_revoke: true, revocation_scope: 'connection' },
        { connection_id: `agent_${'2'.repeat(64)}`, kind: 'agent', client_name: 'Riley · Codex', state: 'active', can_revoke: true, revocation_scope: 'connection' },
      ],
      next_cursor: null,
    },
  };
  let directoryBarrier: Promise<void> | null = null;
  const reads: string[] = [];
  const fetch = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const path = String(input);
    if (path.includes('/api/team/projects')) return Response.json(DEMO_PROJECTS);
    if (path === '/api/team/directory') {
      reads.push(String(init?.body));
      if (directoryBarrier) await directoryBarrier;
      return Response.json(directoryResponse);
    }
    if (path.includes('/api/team/overview-model')) return Response.json(overviewResponse);
    if (path === '/api/team/connections/list') return Response.json(connectionsResponse);
    return Response.json({ ok: false, error: { code: 'NoRoute' } });
  });
  vi.stubGlobal('fetch', fetch);
  return {
    fetch,
    reads,
    setDirectoryResponse: (value: unknown) => { directoryResponse = value; },
    setOverviewResponse: (value: unknown) => { overviewResponse = value; },
    setConnectionsResponse: (value: unknown) => { connectionsResponse = value; },
    holdDirectory: () => { directoryBarrier = new Promise(() => {}); },
  };
}

async function loaded() {
  await screen.findByTestId('member-row-person-taylor');
}

afterEach(async () => {
  await act(async () => { await i18n.changeLanguage('zh-CN'); });
});

describe('members and Agents panorama', () => {
  it.each(['zh-CN', 'en-US'])('renders the prototype blocks and product language in %s', async (language) => {
    const { setOverviewResponse } = setup();
    await act(async () => { await i18n.changeLanguage(language); });
    setOverviewResponse(withTaylorDecisionWaits(overviewModel()));
    renderApp('/members?scope=demo');
    await loaded();
    const page = screen.getByTestId('page-members');
    expect(within(page).getByRole('heading', { level: 1, name: i18n.t('members.title') })).toBeTruthy();
    expect(screen.getByTestId('members-summary')).toHaveTextContent(i18n.t('members.summary', { members: 3, groups: 2 }));
    expect(screen.getByTestId('member-table')).toBeTruthy();
    expect(screen.getByTestId('members-flat-note')).toHaveTextContent(i18n.t('members.flatHierarchy'));
    expect(screen.getByTestId('member-doing-person-taylor')).toHaveTextContent('A very long task title');
    expect(screen.getByTestId('member-pending-person-taylor')).toHaveTextContent('3');
    // The prototype's quiet zero is the words 没有 / None, pinned literally: reading the catalog here would follow any edit of it.
    expect(screen.getByTestId('member-pending-person-riley')).toHaveTextContent(language === 'zh-CN' ? '没有' : 'None');
    expect(screen.getByTestId('member-pending-unknown-2')).toHaveTextContent('—');
    expect(screen.getByTestId('member-row-person-taylor')).toHaveAttribute('data-highlighted', 'true');
    expect(screen.getAllByText(i18n.t('members.roleMissing')).length).toBeGreaterThan(0);
    expect(screen.getByTestId('freshness-badge')).toBeTruthy();
    expect(within(page).getByRole('link', { name: i18n.t('members.viewConnections') })).toHaveAttribute('href', '/account?scope=demo');
    expect(within(page).getByRole('link', { name: i18n.t('members.connectAgent') })).toHaveAttribute('href', '/agent-connect?scope=demo');
    expect(page.querySelector('details')).toBeNull();
    expect(page.textContent).not.toMatch(/person-taylor|actor-taylor|task \/ \?|agent_111|connection_id|agentActive/);
    expect(findHumanLanguageViolations(page)).toEqual([]);
  });

  it('derives active and inactive Agent lamps from recorded activity facts', async () => {
    setup();
    renderApp('/members?scope=demo');
    await loaded();
    expect(screen.getByTestId('member-agent-person-taylor-0')).toHaveAttribute('data-agent-state', 'active');
    expect(screen.getByTestId('member-agent-person-riley-0')).toHaveAttribute('data-agent-state', 'inactive');
  });

  it('shows unknown Agent lamps when activity is explicitly unread', async () => {
    const { setOverviewResponse } = setup();
    const model = overviewModel();
    model.input.tasks.items![0].agentActive = null;
    model.input.tasks.items![1].agentActive = null;
    setOverviewResponse(model);
    renderApp('/members?scope=demo');
    await loaded();
    expect(screen.getByTestId('member-agent-person-taylor-0')).toHaveAttribute('data-agent-state', 'unknown');
    expect(screen.getByTestId('member-agent-person-riley-0')).toHaveAttribute('data-agent-state', 'unknown');
  });

  it('keeps member search with counts matching visible rows', async () => {
    setup();
    renderApp('/members?scope=demo');
    await loaded();
    const user = userEvent.setup();
    const search = screen.getByRole('textbox', { name: '搜索成员或部门' });
    await user.type(search, 'Research');
    expect(screen.queryByTestId('member-row-person-taylor')).toBeNull();
    expect(screen.getByTestId('member-row-person-riley')).toBeTruthy();
    expect(screen.getByTestId('members-section-count')).toHaveTextContent('1');
    await user.clear(search);
    expect(screen.getByTestId('member-row-person-taylor')).toBeTruthy();
  });

  it('keeps department filtering and applies a selected group to the same member rows', async () => {
    setup();
    renderApp('/members?scope=demo');
    await loaded();
    expect(screen.getByRole('combobox', { name: '筛选部门' })).toBeTruthy();
    const user = userEvent.setup();
    await user.click(screen.getByTestId('members-summary-groups'));
    await user.click(await screen.findByRole('button', { name: 'Engineering' }));
    await waitFor(() => expect(screen.queryByTestId('member-row-person-riley')).toBeNull());
    expect(screen.getByTestId('member-row-person-taylor')).toBeTruthy();
    expect(screen.getByTestId('members-section-count')).toHaveTextContent('1');
  });

  it('opens member details from the same array as the section count', async () => {
    setup();
    renderApp('/members?scope=demo');
    await loaded();
    await userEvent.setup().click(screen.getByTestId('members-section-count'));
    expect(await screen.findByTestId('members-detail')).toBeTruthy();
    expect(screen.getAllByTestId('members-detail-row')).toHaveLength(3);
    expect(screen.getByTestId('members-detail-count')).toHaveTextContent('3');
  });

  it('opens group details from the same array as the group count', async () => {
    setup();
    renderApp('/members?scope=demo');
    await loaded();
    await userEvent.setup().click(screen.getByTestId('members-summary-groups'));
    expect(await screen.findByTestId('members-detail')).toHaveTextContent('Engineering');
    expect(screen.getAllByTestId('members-detail-row')).toHaveLength(2);
    expect(screen.getByTestId('members-detail-count')).toHaveTextContent('2');
  });

  it('opens pending details from the same array as the pending count', async () => {
    const { setOverviewResponse } = setup();
    setOverviewResponse(withTaylorDecisionWaits(overviewModel()));
    renderApp('/members?scope=demo');
    await loaded();
    await userEvent.setup().click(screen.getByTestId('member-pending-person-taylor'));
    expect(await screen.findByTestId('members-detail')).toBeTruthy();
    expect(screen.getAllByTestId('members-detail-row')).toHaveLength(3);
    expect(screen.getByTestId('members-detail-count')).toHaveTextContent('3');
    expect(screen.getByTestId('members-detail')).toHaveTextContent(i18n.t('members.pendingKinds.decision'));
  });

  it('renders a known zero as quiet text and leaves unknown counts closed', async () => {
    setup();
    renderApp('/members?scope=demo');
    await loaded();
    const knownEmpty = screen.getByTestId('member-pending-person-riley');
    expect(knownEmpty.tagName).toBe('SPAN');
    expect(knownEmpty).toHaveTextContent(i18n.t('members.noPending'));
    expect(knownEmpty.className).toContain('members-dim');
    expect(screen.queryByTestId('members-detail')).toBeNull();
    expect(screen.getByTestId('member-pending-unknown-2').tagName).not.toBe('BUTTON');
  });

  it('nulls the wait count only when the wait facts are unread', async () => {
    const { setOverviewResponse } = setup();
    // An unread confirmations plane leaves the blocking-wait count known: the members count
    // reads waits alone, so Taylor's three open decisions still read as three items.
    const unreadConfirmations = withTaylorDecisionWaits(overviewModel());
    unreadConfirmations.input.approvals = { coverage: 'unread', items: [], reason: 'not-read' };
    unreadConfirmations.coverage.approvals = { status: 'unread', missing: [] };
    setOverviewResponse(unreadConfirmations);
    const first = renderApp('/members?scope=demo');
    await loaded();
    expect(screen.getByTestId('member-pending-person-taylor').tagName).toBe('BUTTON');
    expect(screen.getByTestId('member-pending-person-taylor')).toHaveTextContent('3');
    first.unmount();
    // An unread decisions plane makes the same count unknown; it must never read as zero.
    const unreadWaits = withTaylorDecisionWaits(overviewModel());
    unreadWaits.input.decisions = { coverage: 'unread', items: [], reason: 'not-read' };
    unreadWaits.coverage.decisions = { status: 'unread', missing: [] };
    setOverviewResponse(unreadWaits);
    renderApp('/members?scope=demo');
    await loaded();
    for (const id of ['person-taylor', 'person-riley']) {
      const cell = screen.getByTestId(`member-pending-${id}`);
      expect(cell.tagName).not.toBe('BUTTON');
      expect(cell).toHaveTextContent('—');
      expect(cell.textContent).not.toMatch(/null|\d/);
    }
  });

  it('counts the waits that block work and leaves inbox confirmations and dispatches to the workbench', async () => {
    const { setOverviewResponse } = setup();
    const model = overviewModel();
    model.input.handoffs = { coverage: 'complete', items: [{
      id: 'handoff-1', taskId: 'planned-task', kind: 'responsibility', status: 'proposed',
      pending: true, fromPersonId: 'person-taylor', toPersonId: 'person-riley', updatedAtUnixMs: NOW,
    }] };
    setOverviewResponse(model);
    renderApp('/members?scope=demo');
    await loaded();
    // Taylor holds three confirmations and Riley one dispatch; none of them blocks a task,
    // so the members queue reads the quiet zero instead of a count.
    for (const id of ['person-taylor', 'person-riley']) {
      const cell = screen.getByTestId(`member-pending-${id}`);
      expect(cell.tagName).not.toBe('BUTTON');
      expect(cell).toHaveTextContent(i18n.t('members.noPending'));
    }
  });

  it('composes the doing cell for a recorded reviewer from their review queue', async () => {
    const { setDirectoryResponse, setOverviewResponse } = setup();
    setDirectoryResponse({ ok: true, data: { ...DIRECTORY, members: DIRECTORY.members.map((member, index) => ({
      ...member, ...(index === 0 ? { duty: '复核' } : {}),
    })) } });
    const model = overviewModel();
    model.input.tasks = { coverage: 'complete', items: [...model.input.tasks.items!, {
      id: 'reviewed-task', title: '接口第 3 版：提交与错误反馈', ownerPersonId: 'person-riley', agentName: 'Codex',
      projectId: 'demo', groupId: 'research', started: true, agentActive: true, externalWait: null, acceptance: [],
      report: { completed: false, reportedAtUnixMs: NOW - 1000 },
    }] };
    model.input.reviewRounds = { coverage: 'complete', items: [{
      id: 'round-1', taskId: 'reviewed-task', roundIndex: 1, state: 'open', current: true, independent: true,
      authorPersonId: 'person-riley', reviewerPersonId: 'person-taylor',
      authorActorId: 'actor-riley', reviewerActorId: 'actor-taylor', openedAtUnixMs: NOW - 2 * 86400000,
    }] };
    setOverviewResponse(model);
    renderApp('/members?scope=demo');
    await loaded();
    const doing = screen.getByTestId('member-doing-person-taylor');
    expect(doing).toHaveTextContent('复核“接口第 3 版”等 1 件');
    expect(doing.tagName).not.toBe('A');
    // A developer reads their own running task, and Taylor's own task loses to her review queue.
    expect(screen.getByTestId('member-doing-person-riley')).toHaveTextContent('接口第 3 版：提交与错误反馈');
  });

  it('clamps unknown detail parameters and opens person pages by click and keyboard', async () => {
    setup();
    const { router } = renderApp('/members?scope=demo&detail=bogus&member=unknown');
    await loaded();
    expect(screen.queryByTestId('members-detail')).toBeNull();
    const row = screen.getByTestId('member-row-person-taylor');
    fireEvent.keyDown(row, { key: 'Enter' });
    await waitFor(() => expect(router.state.location.pathname).toBe('/people/person-taylor'));
    expect(await screen.findByTestId('state-not-open')).toBeTruthy();
  });

  it('encodes task links and lets long titles wrap in the inline doing cell', async () => {
    setup();
    renderApp('/members?scope=demo');
    await loaded();
    const link = screen.getByTestId('member-doing-person-taylor');
    expect(link).toHaveAttribute('href', '/tasks/task%20%2F%20%3F?scope=demo');
    expect(link.className).toContain('members-cell-inline');
    expect(link).toHaveAttribute('title', expect.stringContaining('A very long task title'));
  });

  it('requests one organization directory without merging project rows in the page', async () => {
    const { reads, setDirectoryResponse, setOverviewResponse } = setup();
    setDirectoryResponse({ ok: true, data: { ...DIRECTORY, members: DIRECTORY.members.map((member, index) => ({
      ...member,
      duty: index === 0 ? '小组负责人' : undefined,
      projects: index === 0 ? [{ key: 'demo', name: 'Team mainline' }, { key: 'docs', name: 'Docs stream' }]
        : [{ key: 'demo', name: 'Team mainline' }],
    })) } });
    const model = overviewModel();
    model.project = '';
    setOverviewResponse(model);
    renderApp('/members');
    await loaded();
    expect(reads).toEqual(['{}']);
    expect(document.querySelectorAll('.members-table tbody tr.ant-table-row')).toHaveLength(3);
    const first = screen.getByTestId('member-row-person-taylor');
    expect(within(first).getByText('小组负责人')).toBeTruthy();
    expect(screen.getByTestId('member-projects-person-taylor')).toHaveTextContent('Team mainline、Docs stream');
    expect(screen.getAllByRole('link', { name: 'Team mainline' })[0]).toHaveAttribute('href', '/projects/demo');
    expect(screen.getByRole('link', { name: 'Docs stream' })).toHaveAttribute('href', '/projects/docs');
    const page = screen.getByTestId('page-members');
    expect(within(page).getByRole('link', { name: i18n.t('members.viewConnections') })).toHaveAttribute('href', '/account');
    expect(within(page).getByRole('link', { name: i18n.t('members.connectAgent') })).toHaveAttribute('href', '/agent-connect');
  });

  it('falls back to the existing project chooser when organization directories are unsupported', async () => {
    const { reads, fetch, setDirectoryResponse } = setup();
    setDirectoryResponse({ ok: false, error: { code: 'Unsupported' } });
    renderApp('/members');
    expect(await screen.findByText('Team mainline')).toBeTruthy();
    expect(screen.getByTestId('page-members')).toHaveTextContent(i18n.t('members.chooseProject'));
    expect(reads).toEqual(['{}']);
    // The organization scope exists only behind an organization directory; an unsupported one
    // leaves no empty-scope composed read behind (the shell's per-project indicators still read).
    const organizationModelReads = fetch.mock.calls
      .filter(([input]) => String(input).endsWith('/api/team/overview-model?project='));
    expect(organizationModelReads).toHaveLength(0);
    expect(fetch.mock.calls.filter(([input]) => String(input).includes('/api/team/connections/list'))).toHaveLength(0);
    setDirectoryResponse({ ok: true, data: DIRECTORY });
    await userEvent.setup().click(screen.getAllByRole('button', { name: '查看成员' })[0]);
    await loaded();
    expect(reads).toEqual(['{}', JSON.stringify({ project: 'demo' })]);
    expect(document.querySelectorAll('.members-table tbody tr.ant-table-row')).toHaveLength(3);
    expect(screen.queryByText('Docs stream')).toBeNull();
  });

  it('shows a recorded duty and keeps the existing missing-duty fallback', async () => {
    const { setDirectoryResponse } = setup();
    setDirectoryResponse({ ok: true, data: { ...DIRECTORY, members: DIRECTORY.members.map((member, index) => ({
      ...member, ...(index === 0 ? { duty: '小组负责人' } : {}),
    })) } });
    renderApp('/members?scope=demo');
    await loaded();
    expect(within(screen.getByTestId('member-row-person-taylor')).getByText('小组负责人')).toBeTruthy();
    expect(within(screen.getByTestId('member-row-person-riley')).getByText(i18n.t('members.roleMissing'))).toBeTruthy();
  });

  it.each([
    ['flat', 'directory-web-read-flat.json'],
    ['hierarchical', 'directory-web-read-hierarchy.json'],
    ['unconfigured', 'directory-web-read-unconfigured.json'],
  ])('renders the verbatim %s directory reply without raw fields', async (_name, file) => {
    const sample = JSON.parse(readFileSync(`test/fixtures/${file}`, 'utf8')) as { data: unknown };
    const projection = projectDirectory(sample.data);
    expect(projection).not.toBeNull();
    const { setDirectoryResponse } = setup();
    setDirectoryResponse({ ok: true, data: projection });
    renderApp('/members?scope=demo');
    expect(await screen.findByTestId('page-members')).toBeTruthy();
    const page = screen.getByTestId('page-members');
    expect(page.textContent).not.toMatch(/undefined|null|NaN|person_id|department_ids|updated_at_ms/i);
    expect(findHumanLanguageViolations(page)).toEqual([]);
  });

  it('retains confirmed directory facts after a failed refresh and replaces them after recovery', async () => {
    const { setDirectoryResponse } = setup();
    renderApp('/members?scope=demo');
    await loaded();
    setDirectoryResponse({ ok: false, error: { code: 'BadGateway', message: 'private-diagnostics' } });
    await userEvent.setup().click(screen.getByRole('button', { name: '刷新成员' }));
    expect(await screen.findByText(i18n.t('members.refreshFailed'))).toBeTruthy();
    expect(screen.getByTestId('member-row-person-taylor')).toBeTruthy();
    expect(screen.queryByText('private-diagnostics')).toBeNull();
    setDirectoryResponse({ ok: true, data: { ...DIRECTORY, organization: 'Updated company' } });
    await userEvent.setup().click(screen.getByRole('button', { name: '刷新成员' }));
    await waitFor(() => expect(screen.getByTestId('page-members')).toHaveTextContent('Updated company'));
    expect(screen.queryByText(i18n.t('members.refreshFailed'))).toBeNull();
  });

  it.each(['zh-CN', 'en-US'])('shows a designed loading state in %s', async (language) => {
    const { holdDirectory } = setup();
    await act(async () => { await i18n.changeLanguage(language); });
    holdDirectory();
    renderApp('/members?scope=demo');
    expect(await screen.findByTestId('members-loading')).toHaveTextContent(i18n.t('members.readLoading'));
    expect(findHumanLanguageViolations(screen.getByTestId('page-members'))).toEqual([]);
  });

  it.each(['zh-CN', 'en-US'])('shows designed empty and unregistered states in %s', async (language) => {
    const { setDirectoryResponse, setOverviewResponse } = setup();
    await act(async () => { await i18n.changeLanguage(language); });
    setDirectoryResponse({ ok: true, data: { ...DIRECTORY, members: [] } });
    const emptyModel = overviewModel();
    emptyModel.input.members = { coverage: 'complete', items: [] };
    emptyModel.input.tasks = { coverage: 'complete', items: [] };
    emptyModel.input.approvals = { coverage: 'complete', items: [] };
    setOverviewResponse(emptyModel);
    const empty = renderApp('/members?scope=demo');
    expect(await screen.findByTestId('members-empty')).toHaveTextContent(i18n.t('members.empty'));
    expect(findHumanLanguageViolations(screen.getByTestId('page-members'))).toEqual([]);
    empty.unmount();
    setDirectoryResponse({ ok: true, data: {
      configured: false, organization: null, departments: [], updated_at_ms: null, members: [],
    } });
    renderApp('/members?scope=demo');
    expect(await screen.findByTestId('members-empty')).toHaveTextContent(i18n.t('members.notRegistered'));
    expect(screen.getByTestId('members-empty')).toHaveTextContent(i18n.t('members.registrationHelp'));
    expect(findHumanLanguageViolations(screen.getByTestId('page-members'))).toEqual([]);
  });

  it.each(['zh-CN', 'en-US'])('shows designed failure states without diagnostics in %s', async (language) => {
    const { setDirectoryResponse } = setup();
    await act(async () => { await i18n.changeLanguage(language); });
    setDirectoryResponse({ ok: false, error: { code: 'BadGateway', message: 'PrivateError request_id=private' } });
    renderApp('/members?scope=demo');
    expect(await screen.findByTestId('members-read-error')).toHaveTextContent(i18n.t('members.readFailed'));
    expect(screen.queryByText('Taylor')).toBeNull();
    expect(document.body.textContent).not.toMatch(/PrivateError|request_id|private/);
    expect(findHumanLanguageViolations(screen.getByTestId('page-members'))).toEqual([]);
  });

  it.each(['zh-CN', 'en-US'])('keeps directory rows and marks unknown metrics after a partial failure in %s', async (language) => {
    const { setOverviewResponse, setConnectionsResponse } = setup();
    await act(async () => { await i18n.changeLanguage(language); });
    setOverviewResponse({ ok: false, error: { code: 'Unavailable' } });
    setConnectionsResponse({ ok: false, error: { code: 'Unavailable' } });
    renderApp('/members?scope=demo');
    await screen.findByTestId('members-partial');
    expect(screen.getByText('Taylor')).toBeTruthy();
    expect(screen.getByTestId('member-pending-unknown-0')).toHaveTextContent('—');
    expect(screen.getByTestId('members-partial')).toHaveTextContent(i18n.t('members.partialRead'));
    expect(findHumanLanguageViolations(screen.getByTestId('page-members'))).toEqual([]);
  });

  it.each(['zh-CN', 'en-US'])('clears member facts after permission loss in %s and can recover', async (language) => {
    const { setDirectoryResponse } = setup();
    await act(async () => { await i18n.changeLanguage(language); });
    setDirectoryResponse({ ok: false, error: { code: 'Forbidden' } });
    renderApp('/members?scope=demo');
    expect(await screen.findByTestId('state-forbidden')).toBeTruthy();
    expect(screen.queryByText('Taylor')).toBeNull();
    setDirectoryResponse({ ok: true, data: DIRECTORY });
    await userEvent.setup().click(screen.getByRole('button', { name: i18n.t('members.refresh') }));
    await loaded();
    expect(screen.queryByTestId('state-forbidden')).toBeNull();
  });

  it.each(['zh-CN', 'en-US'])('uses dashes and explicit coverage language for unread results in %s', async (language) => {
    const { setOverviewResponse } = setup();
    await act(async () => { await i18n.changeLanguage(language); });
    const model = overviewModel();
    for (const key of ['tasks', 'dependencies', 'decisions', 'reviewRounds', 'handoffs', 'approvals', 'members', 'activity'] as const) {
      model.input[key] = { coverage: 'unread', items: [], reason: 'read-failed' };
      model.coverage[key] = { status: 'unread', missing: [] };
    }
    setOverviewResponse(model);
    renderApp('/members?scope=demo');
    expect(await screen.findByTestId('members-unknown')).toHaveTextContent(i18n.t('members.unknownRead'));
    expect(screen.getByTestId('member-pending-unknown-0')).toHaveTextContent('—');
    expect(screen.getByTestId('member-pending-unknown-0').tagName).not.toBe('BUTTON');
    expect(screen.queryByTestId('member-pending-unknown-0')?.textContent).not.toBe('0');
    expect(findHumanLanguageViolations(screen.getByTestId('page-members'))).toEqual([]);
  });

  it('stops and removes every observation query on revocation', async () => {
    const { fetch } = setup();
    const { queryClient } = renderApp('/members?scope=demo');
    await loaded();
    act(() => revokeObservationReads(queryClient));
    expect(await screen.findByTestId('state-forbidden')).toBeTruthy();
    expect(screen.queryByText('Taylor')).toBeNull();
    expect(queryClient.getQueryCache().getAll().every((query) => query.state.data === undefined)).toBe(true);
    fetch.mockClear();
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 30)); });
    expect(fetch).not.toHaveBeenCalled();
  });

  it('uses only the recorded viewer person id for the viewer marker', async () => {
    const { setOverviewResponse } = setup();
    const model = overviewModel();
    model.input.options.views = null;
    setOverviewResponse(model);
    renderApp('/members?scope=demo');
    await loaded();
    expect(screen.getByTestId('member-who-person-taylor')).not.toHaveAttribute('data-viewer');
  });

  it('keeps the readable table on a phone and hides only prototype optional columns', async () => {
    const viewport = mockViewport(390);
    try {
      setup();
      renderApp('/members?scope=demo');
      await loaded();
      expect(screen.getByTestId('member-table')).toBeTruthy();
      expect(screen.queryByTestId('member-list')).toBeNull();
      expect(screen.queryByTestId('member-projects-person-taylor')).toBeNull();
      expect(screen.queryByTestId('member-agents-person-taylor')).toBeNull();
      expect(screen.getByTestId('member-pending-person-taylor')).toBeTruthy();
      expect(findHumanLanguageViolations(screen.getByTestId('page-members'))).toEqual([]);
    } finally {
      viewport.restore();
    }
  });

  it('joins recorded same-name and multi-group directory identities only by person ID', async () => {
    const { setDirectoryResponse, setOverviewResponse } = setup();
    setDirectoryResponse({ ok: true, data: recordedIdentityDirectory() });
    const model = identityOverviewModel();
    model.input.members.items![0].displayName = 'A different overview display label';
    model.input.members.items![0].groupId = 'an-unrelated-overview-group';
    setOverviewResponse(model);
    renderApp('/members?scope=demo');
    const first = await screen.findByTestId('member-row-directory-person');
    const second = await screen.findByTestId('member-row-directory-person-two');
    expect(first).toHaveAttribute('data-openable', 'true');
    expect(second).toHaveAttribute('data-openable', 'true');
    expect(screen.getAllByText('Project coordinator')).toHaveLength(2);
    expect(document.querySelectorAll('.members-table tbody tr.ant-table-row')).toHaveLength(2);
    expect(screen.getByTestId('member-doing-directory-person')).toHaveTextContent('First recorded task');
    expect(screen.getByTestId('member-doing-directory-person-two')).toHaveTextContent('Second recorded task');
    expect(screen.getByTestId('member-pending-directory-person')).toHaveTextContent('1');
    expect(screen.getByTestId('member-pending-directory-person-two')).toHaveTextContent('2');
    expect(first).not.toHaveTextContent('Second recorded task');
    expect(second).not.toHaveTextContent('First recorded task');
    expect(within(first).getByText('Engineering · Research')).toBeTruthy();
    expect(within(second).getByText('Research')).toBeTruthy();
  });

  it('keeps facts unverified when a directory person ID is absent instead of guessing by name and group', async () => {
    const { setDirectoryResponse, setOverviewResponse } = setup();
    setDirectoryResponse({ ok: true, data: recordedIdentityDirectory() });
    const model = identityOverviewModel();
    model.input.members = { coverage: 'complete',
      items: model.input.members.items!.filter((member) => member.personId !== 'directory-person') };
    setOverviewResponse(model);
    renderApp('/members?scope=demo');
    expect(await screen.findByTestId('member-row-directory-person-two')).toHaveAttribute('data-openable', 'true');
    const missing = screen.getByTestId('member-row-unknown-0');
    expect(missing).not.toHaveAttribute('data-openable');
    expect(missing).not.toHaveTextContent('First recorded task');
    expect(missing).not.toHaveTextContent('Second recorded task');
    expect(screen.getByTestId('member-pending-unknown-0')).toHaveTextContent('—');
    expect(screen.getByTestId('member-doing-unknown-0')).toHaveTextContent(i18n.t('members.doingUnknown'));
    expect(screen.queryByTestId('member-row-directory-person')).toBeNull();
    expect(screen.getByTestId('member-doing-directory-person-two')).toHaveTextContent('Second recorded task');
    expect(screen.getByTestId('member-pending-directory-person-two')).toHaveTextContent('2');
  });

  it('highlights exactly three pending items but not two', async () => {
    const { setOverviewResponse } = setup();
    const model = overviewModel();
    model.input.decisions = { coverage: 'complete', items: [
      ...[1, 2, 3].map((number) => ({ id: `taylor-${number}`, taskId: 'task / ?', state: 'open', current: true,
        question: `Question ${number}`, allowedResponderIds: ['person-taylor'], createdAtUnixMs: NOW })),
      ...[1, 2].map((number) => ({ id: `riley-${number}`, taskId: 'planned-task', state: 'open', current: true,
        question: `Question ${number}`, allowedResponderIds: ['person-riley'], createdAtUnixMs: NOW })),
    ] };
    setOverviewResponse(model);
    renderApp('/members?scope=demo');
    await loaded();
    expect(screen.getByTestId('member-pending-person-taylor')).toHaveTextContent('3');
    expect(screen.getByTestId('member-pending-person-riley')).toHaveTextContent('2');
    expect(screen.getByTestId('member-row-person-taylor')).toHaveAttribute('data-highlighted', 'true');
    expect(screen.getByTestId('member-row-person-riley')).not.toHaveAttribute('data-highlighted');
  });

  it('does not render revoked connections as Agent chips', async () => {
    const { setOverviewResponse, setConnectionsResponse } = setup();
    const model = overviewModel();
    model.input.tasks = { coverage: 'complete', items: [] };
    setOverviewResponse(model);
    setConnectionsResponse({ ok: true, data: { next_cursor: null, items: [
      { kind: 'agent', state: 'revoked', client_name: 'Riley · Retired Agent' },
    ] } });
    renderApp('/members?scope=demo');
    await loaded();
    const agents = screen.getByTestId('member-agents-person-riley');
    expect(agents.querySelector('.members-chip')).toBeNull();
    expect(agents).toHaveTextContent(i18n.t('members.noAgents'));
  });

  it('does not render browser connections as Agent chips', async () => {
    const { setOverviewResponse, setConnectionsResponse } = setup();
    const model = overviewModel();
    model.input.tasks = { coverage: 'complete', items: [] };
    setOverviewResponse(model);
    setConnectionsResponse({ ok: true, data: { next_cursor: null, items: [
      { kind: 'browser', state: 'active', client_name: 'Riley · Chrome' },
    ] } });
    renderApp('/members?scope=demo');
    await loaded();
    const agents = screen.getByTestId('member-agents-person-riley');
    expect(agents.querySelector('.members-chip')).toBeNull();
    expect(agents).toHaveTextContent(i18n.t('members.noAgents'));
  });

  it.each(['zh-CN', 'en-US'])('distinguishes unknown current work from known empty current work in %s', async (language) => {
    setup();
    await act(async () => { await i18n.changeLanguage(language); });
    renderApp('/members?scope=demo');
    await loaded();
    const knownEmpty = screen.getByTestId('member-doing-person-riley');
    const unknown = screen.getByTestId('member-doing-unknown-2');
    expect(knownEmpty).toHaveTextContent(i18n.t('members.noDoing'));
    expect(unknown).toHaveTextContent(i18n.t('members.doingUnknown'));
    expect(knownEmpty.textContent).not.toBe(unknown.textContent);
  });

  it('cycles name sorting through ascending descending and directory order', async () => {
    const { setDirectoryResponse, setOverviewResponse } = setup();
    const names = ['Zoe', 'Alpha', 'Morgan'];
    setDirectoryResponse({ ok: true, data: { ...DIRECTORY, members: names.map((name) => ({
      personId: `person-${name.toLowerCase()}`, name, available: true, departments: [{ key: 'engineering', name: 'Engineering' }],
    })) } });
    const model = overviewModel();
    model.input.members = { coverage: 'complete', items: names.map((name) => ({
      personId: `person-${name.toLowerCase()}`, displayName: name, groupId: 'engineering',
    })) };
    setOverviewResponse(model);
    renderApp('/members?scope=demo');
    await screen.findByTestId('member-row-person-zoe');
    const order = () => [...document.querySelectorAll('.members-table tbody .members-name')].map((element) => element.textContent);
    expect(order()).toEqual(['Zoe', 'Alpha', 'Morgan']);
    const user = userEvent.setup();
    const sort = screen.getByRole('button', { name: i18n.t('members.sortByName') });
    await user.click(sort);
    await waitFor(() => expect(order()).toEqual(['Alpha', 'Morgan', 'Zoe']));
    await user.click(sort);
    await waitFor(() => expect(order()).toEqual(['Zoe', 'Morgan', 'Alpha']));
    await user.click(sort);
    await waitFor(() => expect(order()).toEqual(['Zoe', 'Alpha', 'Morgan']));
  });

  it('matches member search case insensitively', async () => {
    const { setDirectoryResponse, setOverviewResponse } = setup();
    setDirectoryResponse({ ok: true, data: { ...DIRECTORY, members: [
      { personId: 'person-taylor', name: 'Owner A', available: true, departments: [{ key: 'engineering', name: 'Engineering' }] },
    ] } });
    const model = overviewModel();
    model.input.members = { coverage: 'complete', items: [
      { personId: 'person-taylor', displayName: 'Owner A', groupId: 'engineering' },
    ] };
    setOverviewResponse(model);
    renderApp('/members?scope=demo');
    await loaded();
    await userEvent.setup().type(screen.getByRole('textbox', { name: i18n.t('members.search') }), 'owner');
    expect(screen.getByTestId('member-row-person-taylor')).toBeTruthy();
    expect(screen.getByTestId('members-section-count')).toHaveTextContent('1');
  });

  it('keeps member details equal to the filtered visible rows', async () => {
    setup();
    renderApp('/members?scope=demo');
    await loaded();
    const user = userEvent.setup();
    await user.type(screen.getByRole('textbox', { name: i18n.t('members.search') }), 'Research');
    expect(screen.getByTestId('members-section-count')).toHaveTextContent('1');
    await user.click(screen.getByTestId('members-section-count'));
    expect(await screen.findByTestId('members-detail')).toBeTruthy();
    expect(document.querySelectorAll('.members-table tbody tr.ant-table-row')).toHaveLength(1);
    expect(screen.getAllByTestId('members-detail-row')).toHaveLength(1);
    expect(screen.getByTestId('members-detail-count')).toHaveTextContent('1');
    expect(screen.getByTestId('members-detail')).toHaveTextContent('Riley');
    expect(screen.getByTestId('members-detail')).not.toHaveTextContent('Taylor');
  });

  it('encodes person ids in row navigation and member detail links', async () => {
    const { setDirectoryResponse, setOverviewResponse } = setup();
    const personId = 'a/b?c d';
    setDirectoryResponse({ ok: true, data: { ...DIRECTORY, members: [
      { personId, name: 'Taylor', available: true, departments: [{ key: 'engineering', name: 'Engineering' }] },
    ] } });
    const model = overviewModel();
    model.input.tasks = { coverage: 'complete', items: [] };
    model.input.approvals = { coverage: 'complete', items: [] };
    model.input.members = { coverage: 'complete', items: [
      { personId, displayName: 'Taylor', groupId: 'engineering' },
    ] };
    setOverviewResponse(model);
    const { router } = renderApp('/members?scope=demo&detail=members');
    const detailLink = await screen.findByTestId('members-detail-row');
    expect(detailLink).toHaveAttribute('href', '/people/a%2Fb%3Fc%20d');
    await userEvent.setup().click(screen.getByRole('button', { name: i18n.t('members.closeDetail') }));
    fireEvent.keyDown(screen.getByTestId(`member-row-${personId}`), { key: 'Enter' });
    await waitFor(() => expect(router.state.location.pathname).toBe('/people/a%2Fb%3Fc%20d'));
  });

  it('does not reread the directory after revocation', async () => {
    const source = readFileSync('src/pages/MembersPage.tsx', 'utf8');
    const directoryOptions = source.slice(source.indexOf("queryKey: ['team', 'directory', project]"), source.indexOf('  const denied ='));
    // The parent also unmounts this page; pin the query gate itself as well as counting requests.
    expect(directoryOptions).toContain('enabled: !revoked,');
    const { fetch } = setup();
    const { queryClient } = renderApp('/members?scope=demo');
    await loaded();
    const directoryReads = () => fetch.mock.calls.filter(([input]) => String(input) === '/api/team/directory').length;
    const before = directoryReads();
    act(() => revokeObservationReads(queryClient));
    expect(await screen.findByTestId('state-forbidden')).toBeTruthy();
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 30)); });
    expect(directoryReads()).toBe(before);
  });

  it('rejects an overview model belonging to another project', async () => {
    const { setOverviewResponse } = setup();
    const model = overviewModel();
    model.project = 'docs';
    setOverviewResponse(model);
    renderApp('/members?scope=demo');
    expect(await screen.findByTestId('member-row-unknown-0')).not.toHaveAttribute('data-openable');
    expect(screen.getByTestId('member-pending-unknown-0')).toHaveTextContent('—');
    expect(screen.getByTestId('member-pending-unknown-1')).toHaveTextContent('—');
    expect(screen.queryByTestId('member-row-person-taylor')).toBeNull();
    expect(screen.getByTestId('page-members')).not.toHaveTextContent('3 件');
  });

  it('marks a connection list with another page as partial', async () => {
    const { setConnectionsResponse } = setup();
    setConnectionsResponse({ ok: true, data: { items: [], next_cursor: `agent_${'3'.repeat(64)}` } });
    renderApp('/members?scope=demo');
    await loaded();
    expect(await screen.findByTestId('members-partial')).toHaveTextContent(i18n.t('members.partialCoverage'));
    expect(screen.getByTestId('members-coverage')).toHaveTextContent(i18n.t('members.coveragePartial'));
  });

  it('distinguishes unreadable names from absent names without exposing references', async () => {
    const { setDirectoryResponse } = setup();
    setDirectoryResponse({ ok: true, data: {
      ...DIRECTORY,
      organization: 'client9x',
      departments: [{ key: 'private-department', name: 'status: custom failure' }],
      members: [{ personId: 'person-unreadable', name: 'agent7a', available: true, departments: [{ key: 'private-department', name: 'status: custom failure' }] }],
    } });
    renderApp('/members?scope=demo');
    await screen.findAllByText(humanText('nameUnavailable', 'zh-CN'));
    const page = screen.getByTestId('page-members');
    expect(page.textContent).not.toMatch(/client9x|agent7a|status: custom failure|private-department/);
    expect(findHumanLanguageViolations(page)).toEqual([]);
  });
});
