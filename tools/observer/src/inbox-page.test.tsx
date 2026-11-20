import { act, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { existsSync, readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { afterEach, describe, expect, it, vi } from 'vitest';
import i18n from './i18n';
import { defaultVisibleText, findHumanLanguageViolations } from './lib/human-language-guard';
import { buildInboxModel } from './lib/inbox-model';
import { inboxProjectReadQueryOptions } from './lib/inbox-queries';
import { revokeObservationReads } from './lib/queries';
import { expectSvgDrawsPrototypeGlyph } from './components/icons/expect-prototype-glyph';
import type { PrototypeIconName } from './components/icons/prototype-icons';
import type { TeamOverviewModelResponse } from './lib/types';
import type { OverviewInput, OverviewPart } from './core/overview';
import {
  DAY,
  NOW,
  approval,
  baseTask,
  complete,
  decision,
  makeInput,
  reviewRound,
} from './core/overview/__tests__/factories';
import { DEMO_OVERVIEW, DEMO_PROJECTS, renderApp } from './test-utils';

const PARTS: OverviewPart[] = ['tasks', 'dependencies', 'decisions', 'reviewRounds', 'handoffs', 'approvals', 'members', 'activity'];
const PROJECTS = { ...DEMO_PROJECTS, projects: [{ key: 'demo', title: 'Customer Portal', role: 'developer' }] };

function response(input: OverviewInput): TeamOverviewModelResponse {
  return {
    ok: true,
    project: 'demo',
    // The render clock advances from this real answer moment to the fixture's explicit derivation clock.
    read_at_unix_ms: Date.now(),
    input,
    coverage: Object.fromEntries(PARTS.map((part) => [part, {
      status: input[part].coverage,
      ...(input[part].coverage === 'complete' ? {} : { reason: input[part].reason }),
      missing: [],
    }])) as unknown as TeamOverviewModelResponse['coverage'],
    freshness: {
      report_received: { status: 'not_measured' },
      authoritative: { status: 'measured', project_revision: '1', at_unix_ms: NOW },
      page_read: { status: 'measured', at_unix_ms: NOW },
    },
  };
}

function pageInput(): OverviewInput {
  return makeInput({
    tasks: complete([
      baseTask('answer-task', {
        projectId: 'demo', ownerPersonId: 'person-a', agentName: 'Desk Agent', started: true, agentActive: false,
      }),
      baseTask('other-task', {
        projectId: 'demo', ownerPersonId: 'person-b', agentName: null, started: true, agentActive: false,
      }),
      baseTask('assignment-task', {
        projectId: 'demo', ownerPersonId: 'person-a', agentName: null, started: false, agentActive: false,
      }),
      baseTask('review-task', {
        projectId: 'demo', ownerPersonId: 'person-b', started: true, agentActive: false,
        submittedResultAtUnixMs: NOW - DAY,
      }),
      baseTask('outside-task', {
        projectId: 'demo', ownerPersonId: 'person-a', agentName: null,
        externalWait: { party: 'Outside Example', sinceUnixMs: NOW - 8 * DAY },
      }),
    ]),
    decisions: complete([
      decision('answer-task', {
        id: 'answer-record', allowedResponderIds: ['person-a'], createdAtUnixMs: NOW - 2 * DAY,
        question: 'Should the empty state explain the next step?', impact: 'Three downstream checks can continue.',
      }),
      decision('other-task', {
        id: 'other-record', allowedResponderIds: ['person-b'], createdAtUnixMs: NOW - 9 * DAY,
        question: 'Should this result be reworked?',
      }),
    ]),
    reviewRounds: complete([reviewRound('review-task', 'open', {
      reviewerPersonId: 'person-a', authorPersonId: 'person-b',
    })]),
    handoffs: complete([{
      id: 'assignment-record', taskId: 'assignment-task', kind: 'responsibility', status: 'proposed', pending: true,
      fromPersonId: 'person-b', toPersonId: 'person-a', updatedAtUnixMs: NOW,
    }]),
    approvals: complete([
      approval('lesson-confirmation', 'person-a', {
        id: 'lesson-record', projectId: 'demo', title: 'Check the interface version first', sinceUnixMs: NOW - 6 * DAY,
      }),
      approval('plan-approval', 'person-a', {
        id: 'plan-record', projectId: 'demo', title: 'Recovery work breakdown', sinceUnixMs: NOW - DAY,
      }),
    ]),
    options: {
      nowUnixMs: NOW,
      stalledAfterDays: 3,
      views: { me: { personId: 'person-a', projectId: 'demo' } },
    },
  });
}

function json(payload: unknown) {
  return new Response(JSON.stringify(payload), { status: 200, headers: { 'content-type': 'application/json' } });
}

function setup({
  input = pageInput(),
  modelPayload,
  projectsPayload = PROJECTS,
  initialPath = '/inbox',
}: {
  input?: OverviewInput;
  modelPayload?: unknown;
  projectsPayload?: unknown;
  initialPath?: string;
} = {}) {
  let currentModel: unknown = modelPayload === undefined ? response(input) : modelPayload;
  const calls: Array<{ method: string; url: string }> = [];
  const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = String(input);
    calls.push({ method: init?.method ?? 'GET', url });
    if (url.includes('/api/team/projects')) return json(projectsPayload);
    if (url.includes('/api/team/overview-model')) return json(currentModel);
    if (url.includes('/api/team/overview')) return json(DEMO_OVERVIEW);
    return json({ ok: false, error: { code: 'NoRoute', message: 'Synthetic missing read' } });
  });
  vi.stubGlobal('fetch', fetchMock);
  const rendered = renderApp(initialPath);
  return {
    ...rendered,
    calls,
    fetchMock,
    setModel(value: unknown) { currentModel = value; },
  };
}

afterEach(async () => {
  vi.unstubAllGlobals();
  Reflect.deleteProperty(navigator, 'clipboard');
  await act(async () => { await i18n.changeLanguage('zh-CN'); });
});

describe('prototype inbox page', () => {
  it.each(['zh-CN', 'en-US'])('renders the queue, focus and shared shell count from one source in %s', async (language) => {
    await i18n.changeLanguage(language);
    setup();
    const page = await screen.findByTestId('page-inbox');
    const queue = await within(page).findByTestId('inbox-queue');
    expect(within(queue).getAllByTestId('inbox-queue-row')).toHaveLength(4);
    expect(within(queue).queryByText(/review-task|outside-task|Should this result be reworked/)).toBeNull();
    expect(within(page).getByTestId('inbox-queue-count').textContent).toBe('4');
    expect(screen.getByTestId('inbox-count').textContent).toBe('4');
    expect(document.querySelectorAll('.ant-badge-dot')).toHaveLength(1);
    expect(screen.getByTestId('inbox-bell').getAttribute('aria-label')).toBe(i18n.t('shell.pendingNotifications', { count: 4 }));
    expect(within(page).getByTestId('inbox-lead').textContent).toContain(language.startsWith('zh') ? '4 件事在等你' : '4 things are waiting for you');
    expect(within(page).getByTestId('inbox-lead').textContent).toContain(language.startsWith('zh') ? '最久的一件等了 2 天' : 'the oldest has been waiting 2 days');
    expect(within(page).getByTestId('inbox-focus')).toBeTruthy();
    expect(within(page).getByTestId('inbox-basis')).toBeTruthy();
    expect(within(page).getByTestId('inbox-impact')).toBeTruthy();
    expect(within(page).getByTestId('inbox-related')).toBeTruthy();
    expect(within(page).getByTestId('inbox-handback')).toBeTruthy();
    expect(within(page).getByTestId('freshness-badge')).toBeTruthy();
    expect(within(page).getByTestId('inbox-coverage')).toBeTruthy();
    // Read-only page: the only buttons are queue selection and the copy button.
    const buttons = within(page).getAllByRole('button');
    expect(buttons.map((button) => button.getAttribute('data-testid'))).toEqual(
      Array.from({ length: buttons.length }, (_, index) => (index === buttons.length - 1 ? 'inbox-copy' : 'inbox-queue-row')),
    );
    expect(findHumanLanguageViolations(page)).toEqual([]);
    expect(defaultVisibleText(page)).not.toMatch(/undefined|null|NaN/);
  });

  it('composes prototype titles and calendar ages from facts, including the focused Agent handback', async () => {
    const source = pageInput();
    if (source.tasks.coverage === 'complete') {
      const answer = source.tasks.items.find((task) => task.id === 'answer-task')!;
      // 47 hours elapsed, but the report and the reference are two calendar dates apart.
      answer.report = { completed: false, reportedAtUnixMs: NOW - 2 * DAY + 60 * 60 * 1000 };
    }
    if (source.decisions.coverage === 'complete') {
      Object.assign(source.decisions.items[0], {
        description: 'The recorded question detail explains the two empty-list choices.',
      });
    }
    setup({ input: source });
    const page = await screen.findByTestId('page-inbox');
    const queue = await within(page).findByTestId('inbox-queue');
    const rows = within(queue).getAllByTestId('inbox-queue-row');
    const assignment = rows.find((row) => row.textContent?.includes('Synthetic assignment-task'))!;
    const lesson = rows.find((row) => row.textContent?.includes('Check the interface version first'))!;

    expect(within(page).getByTestId('inbox-lead').textContent).toContain('最久的一件等了 2 天');
    expect(within(assignment).getByTestId('inbox-queue-title').textContent)
      .toBe('Reviewer Two把“Synthetic assignment-task”派给了你');
    expect(assignment.querySelector('.inbox-qrow-meta')?.textContent)
      .toBe('接任务 · Reviewer Two今天通过自己的 Agent 派的');
    expect(assignment.querySelector('.inbox-qrow-age')?.textContent).toBe('今天');
    expect(lesson.querySelector('.inbox-qrow-meta')?.textContent)
      .toBe('确认经验 · 你的 Agent 在 1 月 9 日的会话里记下的');
    expect(lesson.querySelector('.inbox-qrow-age')?.textContent).toBe('6 天前');

    await userEvent.click(rows.find((row) => row.textContent?.includes('Should the empty state explain the next step?'))!);
    expect(within(page).getByTestId('inbox-basis').textContent).toContain('Agent 最后一次报告是 2 天前');
    expect(within(page).getByTestId('inbox-handback').querySelector('p')?.textContent)
      .toBe('你的「Desk Agent」停在这件任务上。回到那个会话回答，它会接着做。');
    expect(within(page).getByTestId('inbox-focus').querySelector('.inbox-focus-ask')?.textContent)
      .toBe('The recorded question detail explains the two empty-list choices.');
  });

  it('keeps the existing honest fallbacks when a person, task, session date, description or Agent is missing', async () => {
    const source = pageInput();
    if (source.tasks.coverage === 'complete') {
      source.tasks.items.find((task) => task.id === 'answer-task')!.agentName = null;
    }
    if (source.handoffs.coverage === 'complete') source.handoffs.items[0].fromPersonId = 'person-not-in-directory';
    if (source.approvals.coverage === 'complete') {
      source.approvals.items.find((approval) => approval.kind === 'lesson-confirmation')!.sinceUnixMs = null;
    }
    const first = setup({ input: source });
    const page = await screen.findByTestId('page-inbox');
    const queue = await within(page).findByTestId('inbox-queue');
    const assignment = within(queue).getAllByTestId('inbox-queue-row')
      .find((row) => row.textContent?.includes('Synthetic assignment-task'))!;
    expect(within(assignment).getByTestId('inbox-queue-title').textContent).toBe('Synthetic assignment-task');
    expect(assignment.querySelector('.inbox-qrow-meta')?.textContent)
      .toBe('接任务 · 一位成员通过自己的 Agent 派的');
    const lesson = within(queue).getAllByTestId('inbox-queue-row')
      .find((row) => row.textContent?.includes('Check the interface version first'))!;
    expect(lesson.querySelector('.inbox-qrow-meta')?.textContent).toBe('确认经验 · 你的 Agent 记下的');
    expect(lesson.querySelector('.inbox-qrow-age')?.textContent).toBe('不知道等了多久');

    await userEvent.click(within(queue).getAllByTestId('inbox-queue-row')
      .find((row) => row.textContent?.includes('Should the empty state explain the next step?'))!);
    expect(within(page).getByTestId('inbox-handback').querySelector('p')?.textContent)
      .toBe('在你的任意一个 Agent 里说一句');
    expect(within(page).getByTestId('inbox-focus').querySelector('.inbox-focus-ask')?.textContent)
      .toBe('「Should the empty state explain the next step?」这件事不回答，Synthetic answer-task就停在这里。');
    first.unmount();

    const missingTask = pageInput();
    if (missingTask.tasks.coverage === 'complete') {
      missingTask.tasks.items.find((task) => task.id === 'assignment-task')!.title = null;
    }
    setup({ input: missingTask });
    const nextQueue = await screen.findByTestId('inbox-queue');
    const unnamed = within(nextQueue).getAllByTestId('inbox-queue-row')
      .find((row) => row.querySelector('.inbox-qrow-meta')?.textContent?.startsWith('接任务'))!;
    expect(within(unnamed).getByTestId('inbox-queue-title').textContent).toBe('这件事的名称还没有记录');
    expect(unnamed.textContent).not.toContain('派给了你');
  });

  it('words a waiting answer as how long it has waited and the other ages as how long ago', async () => {
    setup();
    const page = await screen.findByTestId('page-inbox');
    const queue = await within(page).findByTestId('inbox-queue');
    const rows = within(queue).getAllByTestId('inbox-queue-row');
    const age = (needle: string) => rows.find((row) => row.textContent?.includes(needle))!
      .querySelector('.inbox-qrow-age')?.textContent;
    expect(age('Should the empty state explain the next step?')).toBe('等了 2 天');
    expect(age('Recovery work breakdown')).toBe('1 天前');
  });

  it('shows the age and the report age as unknown, not a guess, when the read is stamped later than the derivation clock allows', async () => {
    const source = pageInput();
    if (source.tasks.coverage === 'complete') {
      source.tasks.items.find((task) => task.id === 'answer-task')!.report = { completed: false, reportedAtUnixMs: NOW - DAY };
    }
    setup({ modelPayload: { ...response(source), read_at_unix_ms: Date.now() + 40 * DAY } });
    const page = await screen.findByTestId('page-inbox');
    const queue = await within(page).findByTestId('inbox-queue');
    const answer = within(queue).getAllByTestId('inbox-queue-row')
      .find((row) => row.textContent?.includes('Should the empty state explain the next step?'))!;
    expect(answer.querySelector('.inbox-qrow-age')?.textContent).toBe('不知道等了多久');
    await userEvent.click(answer);
    expect(within(page).getByTestId('inbox-basis').textContent).toContain('Agent 最后一次报告的时间还没有读到');
  });

  it('uses the PPO-012 sentence and browser-only copy with a selection fallback', async () => {
    const writeText = vi.fn(async () => {});
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } });
    const { calls } = setup();
    // The queue opens on the oldest entry; the answer entry carries the question line.
    await userEvent.click(await screen.findByRole('button', { name: /Should the empty state explain the next step\?/ }));
    await screen.findByTestId('inbox-handback-line');
    const before = calls.length;
    await userEvent.click(screen.getByTestId('inbox-copy'));
    expect(writeText).toHaveBeenCalledWith(expect.stringContaining('Should the empty state explain the next step?'));
    expect(calls.length).toBe(before);
    expect(await screen.findByRole('status')).toHaveProperty('textContent', '已复制，贴到你的 Agent 里就行');

    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: undefined });
    await userEvent.click(screen.getByTestId('inbox-copy'));
    expect(await screen.findByRole('status')).toHaveProperty('textContent', '复制不了，请手动选中这句话');
    expect(calls.length).toBe(before);
    expect(screen.getByTestId('inbox-handback-line').textContent).not.toMatch(/answer-record|project_revision|source_snapshot_id/);
  });

  it('shows a linked item that is not waiting for the viewer without adding it to the queue', async () => {
    const source = response(pageInput());
    const built = buildInboxModel({ models: [source], projects: PROJECTS.projects });
    const other = built.allEntries.find((entry) => !entry.mine)!;
    setup({ initialPath: `/inbox?detail=${encodeURIComponent(other.key)}` });
    const focus = await screen.findByTestId('inbox-not-mine');
    expect(focus.textContent).toContain('这件事不在等你');
    expect(focus.textContent).toContain('Reviewer Two');
    expect(within(screen.getByTestId('inbox-queue')).getAllByTestId('inbox-queue-row')).toHaveLength(4);
    expect(screen.queryByTestId('inbox-copy')).toBeNull();
    expect(screen.queryByTestId('inbox-handback-line')).toBeNull();
  });

  it('clamps bogus URL state, preserves unrelated filters and encodes the related task link', async () => {
    const source = pageInput();
    if (source.tasks.coverage === 'complete') {
      const task = source.tasks.items.find((item) => item.id === 'answer-task');
      if (task) task.id = 'folder/task ?Which one';
    }
    if (source.decisions.coverage === 'complete') source.decisions.items[0].taskId = 'folder/task ?Which one';
    const { router } = setup({ input: source, initialPath: '/inbox?chain=99&chain=-1&tab=zzz&detail=bogus' });
    await screen.findByTestId('inbox-queue');
    // A bogus detail key resolves to the first queue entry instead of throwing.
    expect(within(screen.getByTestId('inbox-queue')).getAllByRole('button')[0]
      .getAttribute('aria-current')).toBe('true');
    const params = new URLSearchParams(router.state.location.search);
    expect(params.get('detail')).not.toBe('bogus');
    expect(params.get('tab')).toBe('zzz');
    // Every task link goes through the one encoder, /, ? and spaces included.
    await userEvent.click(screen.getByRole('button', { name: /Should the empty state explain the next step\?/ }));
    const link = await screen.findByTestId('inbox-task-link');
    expect(link.getAttribute('href')).toBe('/tasks/folder%2Ftask%20%3FWhich%20one');
    const assignment = screen.getByRole('button', { name: /Synthetic assignment-task/ });
    await userEvent.click(assignment);
    expect(new URLSearchParams(router.state.location.search).get('detail')).not.toBe(params.get('detail'));
  });

  it('marks long untrusted titles for truncation without rendering them as markup', async () => {
    const source = pageInput();
    const longTitle = `<strong>${'Very long title '.repeat(30)}</strong>`;
    if (source.decisions.coverage === 'complete') source.decisions.items[0].question = longTitle;
    setup({ input: source });
    const title = (await screen.findAllByTestId('inbox-queue-title')).find((element) => element.textContent === longTitle)!;
    expect(title.getAttribute('data-truncate')).toBe('true');
    expect(title.querySelector('strong')).toBeNull();
    expect(title.getAttribute('style')).toBeNull();
    expect(document.documentElement.scrollWidth).toBeLessThanOrEqual(document.documentElement.clientWidth);
  });

  it('removes a completed item on the next model refresh and updates all three indicators together', async () => {
    const original = pageInput();
    const { queryClient, setModel } = setup({ input: original });
    await screen.findByTestId('inbox-queue');
    expect(screen.getByTestId('inbox-count').textContent).toBe('4');
    const next = structuredClone(original);
    if (next.approvals.coverage === 'complete') {
      next.approvals.items = next.approvals.items.filter((item) => item.kind !== 'lesson-confirmation');
    }
    setModel(response(next));
    await act(async () => { await queryClient.refetchQueries({ queryKey: ['team', 'overview-model-inbox'] }); });
    await waitFor(() => expect(screen.getByTestId('inbox-queue-count').textContent).toBe('3'));
    expect(screen.getByTestId('inbox-count').textContent).toBe('3');
    expect(screen.getByTestId('inbox-bell').getAttribute('aria-label')).toBe(i18n.t('shell.pendingNotifications', { count: 3 }));
    expect(screen.queryByText('Check the interface version first')).toBeNull();
  });

  it('refuses a project read the moment observation access is revoked', async () => {
    const { queryClient } = setup();
    await screen.findByTestId('inbox-queue');
    const options = inboxProjectReadQueryOptions('demo', false);
    const before = await options.queryFn({
      signal: new AbortController().signal,
      client: queryClient,
    });
    expect(before.ok).toBe(true);
    await act(async () => { revokeObservationReads(queryClient); });
    await expect(options.queryFn({
      signal: new AbortController().signal,
      client: queryClient,
    })).rejects.toThrow('Observation access revoked');
  });

  it('clears the queue and stops composed reads immediately after revocation', async () => {
    const { queryClient, fetchMock } = setup();
    await screen.findByTestId('inbox-queue');
    await act(async () => { revokeObservationReads(queryClient); });
    await screen.findByTestId('state-forbidden');
    expect(screen.queryByTestId('inbox-queue')).toBeNull();
    expect(screen.queryByTestId('inbox-count')).toBeNull();
    expect(queryClient.getQueryData(['team', 'overview-model-inbox', 'demo'])).toBeUndefined();
    const calls = fetchMock.mock.calls.length;
    await new Promise((resolve) => setTimeout(resolve, 30));
    expect(fetchMock.mock.calls.length).toBe(calls);
  });

  it.each([
    ['data', 'inbox-queue'],
    ['empty', 'inbox-empty'],
    ['failed', 'inbox-failed'],
    ['partial', 'inbox-partial'],
    ['revoked', 'state-forbidden'],
    ['unknown', 'inbox-unknown'],
  ])('has a designed %s state in Chinese and English', async (state, testId) => {
    for (const language of ['zh-CN', 'en-US'] as const) {
      await i18n.changeLanguage(language);
      const source = pageInput();
      let modelPayload: unknown;
      if (state === 'empty') {
        const empty = makeInput({ options: source.options });
        modelPayload = response(empty);
      } else if (state === 'failed') {
        modelPayload = { ok: false, error: { code: 'InternalError', message: 'private-raw-diagnostic' } };
      } else if (state === 'revoked') {
        modelPayload = { ok: false, error: { code: 'Forbidden', message: 'private-raw-diagnostic' } };
      } else if (state === 'unknown') {
        source.options.views = {};
        modelPayload = response(source);
      } else if (state === 'partial') {
        source.handoffs = { coverage: 'unread', reason: 'read-failed' };
        modelPayload = response(source);
      }
      const rendered = setup({ input: source, modelPayload });
      await screen.findByTestId(testId);
      expect(defaultVisibleText(screen.getByTestId('page-inbox'))).not.toMatch(/undefined|null|NaN|private-raw-diagnostic/);
      expect(findHumanLanguageViolations(screen.getByTestId('page-inbox'))).toEqual([]);
      if (state === 'empty') {
        expect(screen.getByTestId('inbox-queue-count').textContent).toBe('0');
        // A known zero keeps the page count at 0; the shell pill and the bell dot stay hidden.
        expect(screen.queryByTestId('inbox-count')).toBeNull();
        expect(document.querySelector('.ant-badge-dot')).toBeNull();
      }
      if (state === 'unknown' || state === 'failed' || state === 'revoked') {
        expect(screen.queryByTestId('inbox-count')).toBeNull();
      }
      rendered.unmount();
    }
  });

  it('keeps the unread count as an em dash and exposes a non-interactive loading state', async () => {    const source = pageInput();
    source.tasks = { coverage: 'unread', reason: 'not-read' };
    source.decisions = { coverage: 'unread', reason: 'not-read' };
    source.handoffs = { coverage: 'unread', reason: 'not-read' };
    source.approvals = { coverage: 'unread', reason: 'not-read' };
    setup({ input: source });
    await screen.findByTestId('inbox-unknown');
    expect(screen.getByTestId('inbox-queue-count').textContent).toBe('—');
    expect(screen.queryByTestId('inbox-count')).toBeNull();
    expect(document.querySelector('.ant-badge-dot')).toBeNull();

    vi.unstubAllGlobals();
    vi.stubGlobal('fetch', vi.fn((input: RequestInfo | URL) => {
      if (String(input).includes('/api/team/projects')) return Promise.resolve(json(PROJECTS));
      return new Promise<Response>(() => {});
    }));
    renderApp('/inbox');
    expect((await screen.findByTestId('inbox-loading')).getAttribute('role')).toBe('status');
    // The freshly mounted page carries its own unread count while the read is in flight.
    expect(screen.getAllByTestId('inbox-queue-count').at(-1)?.textContent).toBe('—');
    expect(screen.queryByTestId('inbox-count')).toBeNull();
  });

  // The rehearsal run's verbatim overview reply: real works with summaries, empty member,
  // decision, handoff and approval planes, and no viewer record (live replies carry no
  // views). Skipped wherever the local sample pack is absent (CI).
  const liveSamplePath = path.resolve(
    path.dirname(fileURLToPath(import.meta.url)),
    '../../../../../../.local/kimi-frontend/review-444/live-samples/observer-overview.json',
  );
  describe.skipIf(!existsSync(liveSamplePath))('verbatim live sample render', () => {
    it('shows the member-unknown fallback without leaking raw fields from the real reply', async () => {
      const sample = JSON.parse(readFileSync(liveSamplePath, 'utf8')) as {
        works?: Array<Record<string, unknown>>;
        freshness?: unknown;
      };
      const complete = (items: unknown[]) => ({ coverage: 'complete' as const, items });
      const unread = { coverage: 'unread' as const, items: [], reason: 'not-read' as const };
      const modelPayload = {
        ok: true,
        project: 'demo',
        read_at_unix_ms: sample.freshness && typeof (sample.freshness as { page_read?: { at_unix_ms?: number } }).page_read?.at_unix_ms === 'number'
          ? (sample.freshness as { page_read: { at_unix_ms: number } }).page_read.at_unix_ms
          : 1790000000000,
        input: {
          // The bridge maps each work's id and title; the sample's own values pass through.
          tasks: complete((sample.works ?? []).map((work) => ({
            id: typeof work.work_id === 'string' ? work.work_id : null,
            title: typeof work.title === 'string' ? work.title : null,
            ownerPersonId: null,
            agentName: null,
            acceptance: [],
            report: null,
            submittedResultAtUnixMs: null,
            integrated: null,
            started: null,
            agentActive: null,
            externalWait: null,
          }))),
          dependencies: unread,
          decisions: unread,
          reviewRounds: unread,
          handoffs: unread,
          approvals: unread,
          members: complete([]),
          activity: unread,
          options: {
            nowUnixMs: 1790000000000,
            stalledAfterDays: 3,
            // Live replies have no views plane at all: the identity stays unknown.
          },
        },
        coverage: {
          tasks: { status: 'complete', missing: [] },
          dependencies: { status: 'unread', reason: 'not-read', missing: [] },
          decisions: { status: 'unread', reason: 'not-read', missing: [] },
          reviewRounds: { status: 'unread', reason: 'not-read', missing: [] },
          handoffs: { status: 'unread', reason: 'not-read', missing: [] },
          approvals: { status: 'unread', reason: 'not-read', missing: [] },
          members: { status: 'complete', missing: [] },
          activity: { status: 'unread', reason: 'not-read', missing: [] },
        },
        freshness: sample.freshness,
      };
      setup({ modelPayload });
      const page = await screen.findByTestId('page-inbox');
      await within(page).findByTestId('inbox-unknown');
      expect(within(page).getByTestId('inbox-queue-count').textContent).toBe('—');
      const text = defaultVisibleText(page);
      expect(text).not.toMatch(/undefined|null|NaN|work_id|interaction_mode|schema|mcp_url/);
      expect(findHumanLanguageViolations(page)).toEqual([]);
    });
  });
});

/*
 * Every icon slot of the inbox page draws its prototype glyph. The expectation is
 * always PROTOTYPE_ICONS[name] — never this page's own imports — so a swapped glyph
 * (kind, evidence, "not waiting on you", jump arrow, terminal, copy) turns red.
 */
describe('inbox page icons (prototype glyphs)', () => {
  const KIND_GLYPH = {
    answer: 'help',
    decision: 'scale',
    'plan-approval': 'split',
    assignment: 'inbox',
    lesson: 'book',
  } as const satisfies Record<string, PrototypeIconName>;

  /** The queue fixture plus a decision-kind row (a decision on someone else's task). */
  function iconInput(): OverviewInput {
    const source = pageInput();
    if (source.tasks.coverage === 'complete') {
      // A missing report turns the answer entry's report evidence "bad" deterministically.
      source.tasks.items[0].report = null;
    }
    if (source.decisions.coverage === 'complete') {
      source.decisions.items = [...source.decisions.items, decision('other-task', {
        id: 'decide-record',
        allowedResponderIds: ['person-a'],
        question: 'Pick the storage engine',
        createdAtUnixMs: NOW - DAY,
      })];
    }
    return source;
  }

  it('draws every queue kind icon from the prototype glyphs', async () => {
    await i18n.changeLanguage('zh-CN');
    setup({ input: iconInput() });
    const page = await screen.findByTestId('page-inbox');
    await within(page).findAllByTestId('inbox-queue-row');
    const labelToGlyph = new Map((Object.keys(KIND_GLYPH) as Array<keyof typeof KIND_GLYPH>).map((kind) => [
      i18n.t(`inbox.page.kind.${kind}` as `inbox.page.kind.${keyof typeof KIND_GLYPH}`),
      KIND_GLYPH[kind],
    ]));
    const seen = new Set<string>();
    for (const row of within(page).getAllByTestId('inbox-queue-row')) {
      const meta = row.querySelector('.inbox-qrow-meta')?.textContent ?? '';
      const glyph = labelToGlyph.get(meta.split(' · ')[0]);
      if (glyph === undefined) throw new Error(`unmapped kind label in queue row: ${meta}`);
      seen.add(glyph);
      expectSvgDrawsPrototypeGlyph(row.querySelector('.inbox-ki svg'), `inbox queue row (${meta})`, glyph);
    }
    expect([...seen].sort()).toEqual(['book', 'help', 'inbox', 'scale', 'split']);
  });

  it('draws the three evidence icons per tone from the prototype glyphs', async () => {
    await i18n.changeLanguage('zh-CN');
    setup({ input: iconInput() });
    const page = await screen.findByTestId('page-inbox');
    // The answer entry: task-fact evidence (info) plus a missing report (bad).
    await userEvent.click(await within(page).findByRole('button', { name: /Should the empty state explain the next step\?/ }));
    await within(page).findByTestId('inbox-basis');
    expectSvgDrawsPrototypeGlyph(
      within(page).getByTestId('inbox-basis').querySelector('li[data-s="info"] svg'),
      'inbox evidence info',
      'dot',
    );
    expectSvgDrawsPrototypeGlyph(
      within(page).getByTestId('inbox-basis').querySelector('li[data-s="bad"] svg'),
      'inbox evidence bad',
      'badc',
    );
    // The assignment entry starts independently: its second evidence line is "ok".
    await userEvent.click(await within(page).findByRole('button', { name: /Synthetic assignment-task/ }));
    await within(page).findByTestId('inbox-basis');
    expectSvgDrawsPrototypeGlyph(
      within(page).getByTestId('inbox-basis').querySelector('li[data-s="ok"] svg'),
      'inbox evidence ok',
      'okc',
    );
    // The related-task link carries the jump arrow; the mine block carries terminal and copy.
    expectSvgDrawsPrototypeGlyph(within(page).getByTestId('inbox-task-link').querySelector('svg'), 'inbox task link arrow', 'arrow');
    const handback = within(page).getByTestId('inbox-handback');
    expectSvgDrawsPrototypeGlyph(handback.querySelector('.inbox-where-icon svg'), 'inbox handback marker', 'terminal');
    expectSvgDrawsPrototypeGlyph(handback.querySelector('.inbox-copy svg'), 'inbox copy button', 'copy');
  });

  it('draws the not-waiting-on-you marker as the prototype dot glyph', async () => {
    await i18n.changeLanguage('zh-CN');
    // A linked, not-mine entry: the question owed by person-b on their own task
    // (viewer person-a linked to it, so it resolves through allEntries, not the queue).
    setup({ input: iconInput(), initialPath: '/inbox?detail=answer:other-record:person-b' });
    const page = await screen.findByTestId('page-inbox');
    const notMine = await within(page).findByTestId('inbox-not-mine');
    expectSvgDrawsPrototypeGlyph(notMine.querySelector('.inbox-where-icon svg'), 'inbox not-waiting marker', 'dot');
  });
});
