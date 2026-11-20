import { QueryClientProvider } from '@tanstack/react-query';
import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter, useLocation } from 'react-router';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { deriveOverview } from '../../core/overview';
import type { OverviewTask } from '../../core/overview';
import { approval, complete, reviewRound } from '../../core/overview/__tests__/factories';
import i18n from '../../i18n';
import { PerspectiveProvider } from '../../lib/perspective/PerspectiveProvider';
import { unknownPerspectiveFacts } from '../../lib/perspective/types';
import { createAppQueryClient } from '../../lib/queries';
import { RevocationProvider } from '../../lib/revocation';
import { CraftThemeProvider } from '../CraftThemeProvider';
import { WorkbenchFrame } from './WorkbenchFrame';
import { buildMyWorkModel, type WorkbenchPeriodUpdate } from './model';
import { myWorkInput, WORKBENCH_NOW } from '../../test-fixtures/workbench';

function LocationProbe() {
  const location = useLocation();
  return <output data-testid="location" data-path={location.pathname} data-search={location.search} />;
}

function renderFrame(
  input = myWorkInput(),
  initialEntry = '/workbench?scope=project-one',
  latestPeriodUpdate: WorkbenchPeriodUpdate | null = null,
) {
  const queryClient = createAppQueryClient({ retry: false });
  const model = buildMyWorkModel(deriveOverview(input), i18n.language, {
    personId: input.options.views?.me?.personId ?? null,
    latestPeriodUpdate,
    readAtUnixMs: WORKBENCH_NOW,
    referenceUnixMs: WORKBENCH_NOW,
    inputTasks: input.tasks.items,
  });
  return render(
    <QueryClientProvider client={queryClient}>
      <CraftThemeProvider>
        <RevocationProvider>
          <MemoryRouter initialEntries={[initialEntry]}>
            <PerspectiveProvider facts={unknownPerspectiveFacts()}>
              <WorkbenchFrame
                model={model}
                projectKey="project-one"
                projectName="Synthetic Project"
                projectTitles={{ 'project-one': 'Synthetic Project' }}
                readAtUnixMs={WORKBENCH_NOW}
              />
              <LocationProbe />
            </PerspectiveProvider>
          </MemoryRouter>
        </RevocationProvider>
      </CraftThemeProvider>
    </QueryClientProvider>,
  );
}

beforeEach(async () => {
  await i18n.changeLanguage('zh-CN');
});

describe('shared workbench frame and My Work', () => {
  it('keeps the prototype block order, verdict, four numbers, chain, rail and tabs', () => {
    const { container } = renderFrame();
    const page = container.querySelector('[data-testid="workbench-frame"]')!;
    const blocks = [...page.children].map((child) => child.getAttribute('data-block'));
    expect(blocks).toEqual(['heading', 'freshness', 'instrument', 'detail']);
    expect(screen.getByRole('heading', { level: 1, name: '我的工作' })).toBeTruthy();
    expect(screen.getByText('等你')).toBeTruthy();
    expect(container.querySelector('.obs-workbench-verdict-top')?.getAttribute('data-health')).toBe('risk');
    expect(screen.getByText('按你名下的任务和 Agent 的报告算出 · 刚刚')).toBeTruthy();
    expect(screen.getByRole('button', { name: /这句话的依据/ })).toBeTruthy();
    expect(container.querySelectorAll('.obs-kpi')).toHaveLength(4);
    expect(screen.getByRole('heading', { name: '卡在哪' })).toBeTruthy();
    expect(screen.getByRole('heading', { name: '等你的事' })).toBeTruthy();
    const tabs = container.querySelector('.obs-detail-tabs') as HTMLElement;
    expect(within(tabs).getByRole('tab', { name: /我的 Agent/ })).toBeTruthy();
    expect(within(tabs).getByRole('tab', { name: /我在等/ })).toBeTruthy();
    expect(within(tabs).getByRole('tab', { name: /经验/ })).toBeTruthy();
    expect(container.querySelector('.obs-chain-node-root .obs-lamp[data-lamp="blocked"]')).not.toBeNull();
    expect(container.querySelector('.obs-todo-card:not(.obs-todo-min) .obs-todo-detail')).not.toBeNull();
    expect(container.querySelectorAll('.obs-todo-min')).toHaveLength(2);
    expect(screen.getByRole('button', { name: /还有 1 件/ })).toBeTruthy();
  });

  it('selects the first viewer-waiting chain by default after applying My Work order', () => {
    const input = myWorkInput({
      reviewRounds: complete([reviewRound('other-review', 'open', { reviewerPersonId: 'person-b' })]),
    });
    const { container } = renderFrame(input);
    const buttons = within(screen.getByRole('group', { name: '链路' })).getAllByRole('button');
    expect(buttons[0].textContent).toBe('等你回答');
    expect(buttons[0].getAttribute('aria-pressed')).toBe('true');
    expect(container.querySelector('.obs-chain-node-root .obs-chain-title')?.textContent).toBe('等你回答一个问题');
  });

  it('uses the question and prototype wording for answer, assignment and lesson cards', () => {
    const input = myWorkInput({
      reviewRounds: complete([reviewRound('mine-verified', 'approved')]),
      approvals: complete([approval('lesson-confirmation', 'person-a', {
        title: 'Synthetic lesson title', sinceUnixMs: WORKBENCH_NOW - 6 * 86_400_000,
      })]),
    });
    const { container } = renderFrame(input);
    expect(container.querySelector('.obs-page-subtitle')?.textContent).toBe('Owner One · 今天，11 月 19 日');
    const cards = Array.from(container.querySelectorAll('.obs-todo-card'));
    expect(cards).toHaveLength(3);
    expect(cards.map((card) => card.querySelector('.obs-tag')?.textContent)).toEqual(['回答', '接任务', '确认经验']);
    expect(cards[0].querySelector('.obs-todo-title')?.textContent).toBe('Which synthetic option should be used?');
    expect(cards[0].querySelector('.obs-todo-first-meta')?.textContent).toContain('你的 Agent「Agent Two」在问');
    expect(cards[0].querySelector('.obs-todo-age')?.textContent).toBe('等了 2 天');
    expect(cards[1].querySelector('.obs-todo-age')?.textContent).toBe('1 天前');
    expect(cards[2].querySelector('.obs-todo-age')?.textContent).toBe('6 天前');
  });

  it('omits the viewer name from the subtitle when the member record has no name', () => {
    const base = myWorkInput();
    const input = {
      ...base,
      members: complete((base.members.items ?? []).map((member) => member.personId === 'person-a'
        ? { ...member, displayName: null }
        : member)),
    };
    const { container } = renderFrame(input);
    expect(container.querySelector('.obs-page-subtitle')?.textContent).toBe('今天，11 月 19 日');
    expect(container.querySelector('.obs-page-subtitle')?.textContent).not.toContain('·');
  });

  it('uses decision and breakdown wording without replacing the recorded title', () => {
    const base = myWorkInput({
      decisions: complete([]), handoffs: complete([]), reviewRounds: complete([]),
      approvals: complete([approval('plan-approval', 'person-a', { title: 'Synthetic breakdown title' })]),
    });
    const plan = renderFrame(base);
    expect(plan.container.querySelector('.obs-todo-card .obs-tag')?.textContent).toBe('批准拆解');
    expect(plan.container.querySelector('.obs-todo-title')?.textContent).toBe('Synthetic breakdown title');
    plan.unmount();

    const withDecision = myWorkInput({ handoffs: complete([]), reviewRounds: complete([]), approvals: complete([]) });
    const input = {
      ...withDecision,
      tasks: complete((withDecision.tasks.items ?? []).map((task) => task.id === 'mine-decision'
        ? { ...task, ownerPersonId: 'person-c' }
        : task)),
    };
    const decision = renderFrame(input);
    expect(decision.container.querySelector('.obs-todo-card .obs-tag')?.textContent).toBe('决定');
    expect(decision.container.querySelector('.obs-todo-title')?.textContent).toBe('Which synthetic option should be used?');
  });

  it('uses only supported footer facts for each My Work chain kind', async () => {
    const user = userEvent.setup();
    const { container } = renderFrame();
    const cases = [
      ['等你回答', '你回答之后，后面 1 件事可以接着走。'],
      ['等复核', '这一环不在你手里。'],
      ['等Example Client', '这一环在项目外。谁去催，可以请小组负责人定。'],
    ];
    for (const [label, sentence] of cases) {
      await user.click(screen.getByRole('button', { name: label }));
      const footer = container.querySelector('.obs-chain-footer')!;
      expect(footer.textContent).toBe(sentence);
      expect(footer.children).toHaveLength(1);
    }
  });

  it('keeps the no-update verdict when no fact waits for the viewer', () => {
    const input = myWorkInput({
      decisions: complete([]), handoffs: complete([]), approvals: complete([]),
      reviewRounds: complete([reviewRound('other-review', 'open', { reviewerPersonId: 'person-b' })]),
    });
    const { container } = renderFrame(input);
    expect(screen.getByText('没有更新', { selector: '.obs-workbench-health-word' })).toBeTruthy();
    expect(container.querySelector('.obs-workbench-verdict-top')?.getAttribute('data-health')).toBe('none');
  });

  it('marks an empty chain with the flat plain-state attribute', () => {
    const { container } = renderFrame(myWorkInput({ tasks: { coverage: 'complete', items: [] } }));
    expect(container.querySelector('.obs-chain-block')?.getAttribute('data-empty')).toBe('true');
  });

  it('switches between multiple blocker chains and keeps exactly one red root', async () => {
    const user = userEvent.setup();
    const { container } = renderFrame();
    const group = screen.getByRole('group', { name: '链路' });
    const buttons = within(group).getAllByRole('button');
    expect(buttons.length).toBeGreaterThan(1);
    const firstTitle = container.querySelector('.obs-chain-node-root .obs-chain-title')?.textContent;
    await user.click(buttons[1]);
    expect(container.querySelector('.obs-chain-node-root .obs-chain-title')?.textContent).not.toBe(firstTitle);
    expect(container.querySelectorAll('.obs-chain-node-root')).toHaveLength(1);
    expect(container.querySelector('.obs-chain-node-root .obs-lamp[data-lamp="blocked"]')).not.toBeNull();
  });

  it('clamps invalid chain, tab and detail URL parameters to the first safe view', () => {
    const { container } = renderFrame(myWorkInput(), '/workbench?scope=project-one&chain=99&tab=zzz&detail=bogus');
    const chainButtons = within(screen.getByRole('group', { name: '链路' })).getAllByRole('button');
    expect(chainButtons[0].getAttribute('aria-pressed')).toBe('true');
    expect(container.querySelector('.obs-detail-tabs .ant-tabs-tab-active')?.textContent).toContain('我的 Agent');
    expect(screen.queryByTestId('workbench-metric-detail')).toBeNull();
  });

  it.each(['99', '-1', 'abc'])('clamps chain=%s onto the first chain', (value) => {
    renderFrame(myWorkInput(), `/workbench?scope=project-one&chain=${value}`);
    const chainButtons = within(screen.getByRole('group', { name: '链路' })).getAllByRole('button');
    expect(chainButtons[0].getAttribute('aria-pressed')).toBe('true');
  });

  it.each(['pending', 'verified'])('shows the tabs when detail=%s names a metric whose total is unknown', (metric) => {
    const base = myWorkInput();
    const input = metric === 'pending'
      ? myWorkInput({ approvals: { coverage: 'unread', reason: 'not-read' } })
      : myWorkInput({ options: { ...base.options, views: { ...base.options.views!, currentWeek: undefined } } });
    const { container } = renderFrame(input, `/workbench?scope=project-one&detail=${metric}`);
    expect(container.querySelector('.obs-detail-tabs')).not.toBeNull();
    expect(screen.queryByTestId('workbench-metric-detail')).toBeNull();
  });

  it('prints an unknown KPI total as an em dash in a plain cell and a known zero as a clickable 0', () => {
    const unread = renderFrame(myWorkInput({ approvals: { coverage: 'unread', reason: 'not-read' } }));
    const unreadCells = Array.from(unread.container.querySelectorAll('.obs-kpis .obs-kpi'));
    const pendingCell = unreadCells.find((cell) => cell.textContent?.includes('等你处理'));
    expect(pendingCell?.textContent).toContain('—');
    expect(pendingCell?.textContent).not.toContain('0');
    // a cell with an unknown total is a plain div, not a button
    expect(pendingCell?.tagName).toBe('DIV');
    const railCount = unread.container.querySelector('.obs-todo-count');
    expect(railCount?.textContent).toBe('—');
    expect(railCount?.getAttribute('data-unknown')).toBe('true');
    // the rail still lists the confirmed items while the total stays unknown
    expect(unread.container.querySelectorAll('.obs-todo-card').length).toBeGreaterThan(0);
    unread.unmount();

    const zero = renderFrame(myWorkInput({
      approvals: { coverage: 'complete', items: [] },
      handoffs: { coverage: 'complete', items: [] },
      decisions: { coverage: 'complete', items: [] },
      reviewRounds: { coverage: 'complete', items: [] },
    }));
    const zeroCells = Array.from(zero.container.querySelectorAll('.obs-kpis .obs-kpi'));
    const zeroPending = zeroCells.find((cell) => cell.textContent?.includes('等你处理'));
    expect(zeroPending?.querySelector('.obs-kpi-value')?.textContent).toContain('0');
    // a known total opens its items, also at zero
    expect(zeroPending?.tagName).toBe('BUTTON');
  });

  it('encodes a task link whose id contains a space, slash and question mark', async () => {
    const user = userEvent.setup();
    const base = myWorkInput();
    const input = {
      ...base,
      tasks: {
        ...base.tasks,
        items: (base.tasks.items ?? []).map((task: OverviewTask) => task.id === 'mine-external' ? { ...task, id: 'external /?' } : task),
      },
    };
    const { container } = renderFrame(input);
    await user.click(screen.getByRole('button', { name: '等Example Client' }));
    const taskNode = container.querySelector('.obs-chain-node[data-node-kind="task"]')!;
    await user.click(taskNode);
    expect(screen.getByTestId('location').getAttribute('data-path')).toBe('/tasks/external%20%2F%3F');
  });

  it('encodes the same hostile work id from a table row and a pending card', async () => {
    const user = userEvent.setup();
    const base = myWorkInput();
    const input = {
      ...base,
      tasks: {
        ...base.tasks,
        items: (base.tasks.items ?? []).map((task: OverviewTask) => task.id === 'mine-running' ? { ...task, id: 'row /?' } : task),
      },
      decisions: {
        ...base.decisions,
        items: (base.decisions.items ?? []).map((decision) => ({ ...decision, id: 'decision /?' })),
      },
    };
    const { container } = renderFrame(input);
    const row = Array.from(container.querySelectorAll('tr[data-openable="true"]'))
      .find((element) => element.textContent?.includes('Synthetic mine-running'));
    expect(row).toBeTruthy();
    await user.click(row!);
    expect(screen.getByTestId('location').getAttribute('data-path')).toBe('/tasks/row%20%2F%3F');

    const decisionCard = Array.from(container.querySelectorAll('.obs-todo-card'))
      .find((element) => element.textContent?.includes('Which synthetic option should be used?'));
    expect(decisionCard).toBeTruthy();
    await user.click(decisionCard!);
    expect(screen.getByTestId('location').getAttribute('data-path')).toBe('/inbox');
    expect(screen.getByTestId('location').getAttribute('data-search')).toContain('item=decision%20%2F%3F');
  });

  it('truncates a hostile title in chain nodes and pending cards and renders it as text', () => {
    const longTitle = `${'外部依赖的长标题 '.repeat(20)}<img src=x onerror=alert(1)>`;
    const base = myWorkInput();
    const input = {
      ...base,
      tasks: {
        ...base.tasks,
        items: (base.tasks.items ?? []).map((task: OverviewTask) => task.id === 'other-review' ? { ...task, title: longTitle } : task),
      },
    };
    const { container } = renderFrame(input);
    const chainTitles = Array.from(container.querySelectorAll('.obs-chain-title'));
    expect(chainTitles.length).toBeGreaterThan(0);
    for (const title of chainTitles) {
      expect(title.textContent!.length).toBeLessThanOrEqual(120);
    }
    expect(container.querySelector('.obs-chain-block img')).toBeNull();
    const cardTitles = Array.from(container.querySelectorAll('.obs-todo-title'));
    expect(cardTitles.length).toBeGreaterThan(0);
    for (const title of cardTitles) {
      expect(title.textContent!.length).toBeLessThanOrEqual(120);
    }
    expect(cardTitles.some((title) => title.textContent?.includes('…'))).toBe(true);
    expect(container.querySelector('.obs-todo-rail img')).toBeNull();
    expect(container.textContent).not.toContain(longTitle);
  });

  it('toggles the basis of a generated verdict in place instead of navigating', async () => {
    const user = userEvent.setup();
    renderFrame();
    await user.click(screen.getByRole('button', { name: '这句话的依据' }));
    expect(screen.getByTestId('workbench-verdict-basis')).toBeTruthy();
    expect(screen.getByTestId('workbench-verdict-basis').textContent).toContain('没有一期更新可读');
    const location = screen.getByTestId('location');
    // a generated sentence toggles its basis instead of navigating
    expect(location.getAttribute('data-path')).toBe('/workbench');
    expect(location.getAttribute('data-search')).toContain('basis=1');
    await user.click(screen.getByRole('button', { name: '收起依据' }));
    expect(screen.queryByTestId('workbench-verdict-basis')).toBeNull();
    expect(screen.getByTestId('location').getAttribute('data-path')).toBe('/workbench');
  });

  it.each([
    ['本周通过核验', 1],
    ['Agent 在跑', 1],
    ['我在等', 2],
    ['等你处理', 4],
  ])('opens %s into exactly %i detail rows from the same array', async (label, count) => {
    const user = userEvent.setup();
    const { container } = renderFrame();
    await user.click(screen.getByRole('button', { name: new RegExp(label) }));
    const detail = await screen.findByTestId('workbench-metric-detail');
    expect(detail.querySelectorAll('.obs-row-item')).toHaveLength(count);
    expect(detail.getAttribute('data-count')).toBe(String(count));
    await user.click(screen.getByRole('button', { name: '返回工作台' }));
    expect(screen.queryByTestId('workbench-metric-detail')).toBeNull();
    expect(container.querySelector('.obs-detail-panel')).not.toBeNull();
  });

  it('uses a quiet info note for unread-only coverage and warning for partial reads', () => {
    const unread = renderFrame(myWorkInput({ approvals: { coverage: 'unread', reason: 'not-read' } }));
    expect(unread.container.querySelector('[data-testid="workbench-partial"] .obs-notice')?.getAttribute('data-tone')).toBe('info');
    unread.unmount();
    const partial = renderFrame(myWorkInput({ dependencies: { coverage: 'partial', reason: 'read-failed', items: [] } }));
    expect(partial.container.querySelector('[data-testid="workbench-partial"] .obs-notice')?.getAttribute('data-tone')).toBe('warning');
  });

  it('shows the generic review-responsibility role instead of guessing a person', () => {
    const baseInput = myWorkInput({
      reviewRounds: { coverage: 'complete', items: [{
        id: 'review-unknown', taskId: 'other-review', roundIndex: 1, state: 'open', current: true,
        independent: true, authorPersonId: 'person-c', reviewerPersonId: null,
        authorActorId: 'actor-c', reviewerActorId: null, openedAtUnixMs: Date.parse('2026-11-17T12:00:00Z'),
      }] },
    });
    const input = {
      ...baseInput,
      tasks: {
        ...baseInput.tasks,
        items: (baseInput.tasks.items ?? []).map((task: OverviewTask) => task.id === 'other-review' ? { ...task, ownerPersonId: 'person-a' } : task),
      },
    };
    const { container } = renderFrame(input, '/workbench?scope=project-one&tab=waits');
    const panel = container.querySelector('.obs-detail-panel')!;
    expect(panel.textContent).toContain('有复核职责的成员');
    expect(panel.textContent).not.toContain('Owner Three → Owner Three');
  });

  it('shows an unread lessons state and never offers a business mutation', async () => {
    const user = userEvent.setup();
    renderFrame();
    await user.click(screen.getByText('在你的 Agent 里处理', { selector: '.obs-todo-agent-control' }));
    await user.click(screen.getByRole('tab', { name: /经验/ }));
    expect(screen.getByText('经验还没读取')).toBeTruthy();
    for (const action of ['派发', '批准', '提交复核', '回答问题', '合入', '提交']) {
      expect(screen.queryByRole('button', { name: new RegExp(`^${action}$`) })).toBeNull();
    }
    expect(screen.getByRole('button', { name: '复制这句话' })).toBeTruthy();
  });

  it('opens the item from the whole first card and toggles its handback with H', async () => {
    const user = userEvent.setup();
    const first = renderFrame();
    await user.click(first.container.querySelector('.obs-todo-title')!);
    expect(screen.getByTestId('location').getAttribute('data-path')).toBe('/inbox');
    first.unmount();

    const second = renderFrame();
    const card = second.container.querySelector('.obs-todo-card') as HTMLElement;
    card.focus();
    await user.keyboard('h');
    expect(screen.getByTestId('agent-handback')).toBeTruthy();
    expect(screen.getByTestId('location').getAttribute('data-path')).toBe('/workbench');
  });

  it('copies the handback locally without a request', async () => {
    const user = userEvent.setup();
    const writeText = vi.fn(async (_text: string) => undefined);
    Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true });
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    renderFrame();
    await user.click(screen.getByText('在你的 Agent 里处理', { selector: '.obs-todo-agent-control' }));
    await user.click(screen.getByRole('button', { name: '复制这句话' }));
    expect(writeText).toHaveBeenCalledTimes(1);
    const sentence = writeText.mock.calls[0]?.[0] ?? '';
    expect(sentence.length).toBeLessThanOrEqual(240);
    expect(fetchSpy).not.toHaveBeenCalled();
  });

  it('shows a structured title as stored plain text, not as markup', () => {
    const base = myWorkInput();
    const title = 'client[secret]=synthetic-work-title';
    const input = {
      ...base,
      tasks: {
        ...base.tasks,
        items: (base.tasks.items ?? []).map((task: OverviewTask) => task.id === 'mine-running' ? { ...task, title } : task),
      },
    };
    const { container } = renderFrame(input);
    const taskCell = container.querySelector('.obs-data-table tbody tr td:nth-child(2)')!;
    expect(taskCell.textContent).toBe(title);
    expect(taskCell.querySelector('img, script, iframe, object')).toBeNull();
  });

  it('reads the recorded report fact into the latest-report and age columns', () => {
    const base = myWorkInput();
    const input = {
      ...base,
      tasks: {
        ...base.tasks,
        items: (base.tasks.items ?? []).map((task: OverviewTask) => task.id === 'mine-running'
          ? { ...task, agentName: 'Claude 桌面（林悦的 Agent）', report: { completed: true, reportedAtUnixMs: WORKBENCH_NOW - 60_000 } }
          : task),
      },
    };
    const { container } = renderFrame(input);
    const agentCell = container.querySelector('.obs-data-table tbody tr td:first-child')!;
    expect(agentCell.textContent).toBe('Claude 桌面');
    // The unread-reason tooltips only explain the em dashes; a recorded report prints its
    // completion declaration, and the age column counts from the read's fixed "now".
    expect(container.querySelector('thead th:nth-child(3) span')?.getAttribute('title')).toBe('报告还没有读到');
    expect(container.querySelector('thead th:nth-child(5) span')?.getAttribute('title')).toBe('报告时间还没有读到');
    expect(container.querySelector('tbody tr td:nth-child(3)')?.textContent).toBe('报告说做完了');
    expect(container.querySelector('tbody tr td:nth-child(5)')?.textContent).toBe('1 分钟前');
    const twoDays = container.querySelectorAll('.obs-data-table tbody tr')[1] as HTMLElement;
    expect(within(twoDays).getAllByText('报告说还在做').length).toBeGreaterThan(0);
    expect(within(twoDays).getByText('2 天前')).toBeTruthy();
    const withoutReport = container.querySelectorAll('.obs-data-table tbody tr')[2] as HTMLElement;
    expect(within(withoutReport).getAllByText('—')).toHaveLength(2);
  });

  it('never prints a report that is newer than the read it belongs to', () => {
    const base = myWorkInput();
    const input = {
      ...base,
      tasks: {
        ...base.tasks,
        items: (base.tasks.items ?? []).map((task: OverviewTask) => task.id === 'mine-running'
          ? { ...task, report: { completed: false, reportedAtUnixMs: WORKBENCH_NOW + 60_000 } }
          : task),
      },
    };
    const { container } = renderFrame(input);
    expect(container.querySelector('tbody tr td:nth-child(5)')?.textContent).toBe('—');
  });

  it('truncates untrusted long text and never treats markup as an element', () => {
    const baseInput = myWorkInput();
    const longTitle = `${'Long synthetic name '.repeat(40)}<img src=x onerror=alert(1)>`;
    const input = {
      ...baseInput,
      tasks: {
        ...baseInput.tasks,
        items: (baseInput.tasks.items ?? []).map((task: OverviewTask) => task.id === 'mine-running' ? { ...task, title: longTitle } : task),
      },
    };
    const { container } = renderFrame(input);
    const taskCell = container.querySelector('.obs-data-table tbody tr td:nth-child(2)')!;
    expect(taskCell.textContent!.length).toBeLessThanOrEqual(120);
    expect(taskCell.textContent).toContain('…');
    expect(taskCell.querySelector('img')).toBeNull();
    expect(container.textContent).not.toContain(longTitle);
  });

  it('opens task nodes at the corresponding read-only destination', async () => {
    const user = userEvent.setup();
    const { container } = renderFrame();
    const taskNode = container.querySelector('.obs-chain-node[data-node-kind="task"]')!;
    await user.click(taskNode);
    expect(screen.getByTestId('location').getAttribute('data-path')).toMatch(/^\/tasks\//);
  });

  it('names commitment nodes from the projects read and neutral goal wording, never raw keys', async () => {
    const user = userEvent.setup();
    const { container } = renderFrame();
    const titles: string[] = [];
    const buttons = within(screen.getByRole('group', { name: '链路' })).getAllByRole('button');
    for (const button of buttons) {
      await user.click(button);
      container.querySelectorAll('.obs-chain-node[data-node-kind="commitment"]').forEach((node) => {
        titles.push(node.querySelector('.obs-chain-title')?.textContent ?? '');
      });
    }
    expect(titles).toContain('Synthetic Project');
    expect(titles).toContain('目标（还没有名称）');
    expect(titles).not.toContain('project-one');
    expect(titles).not.toContain('goal-one');
    expect(titles).not.toContain('Week 47 commitment');
    expect(container.textContent).not.toMatch(/\bgoal-one\b/);
  });

  it('falls back to neutral project wording when the projects read carries no name', async () => {
    const user = userEvent.setup();
    const queryClient = createAppQueryClient({ retry: false });
    const input = myWorkInput();
    const model = buildMyWorkModel(deriveOverview(input), i18n.language, {
      personId: 'person-a',
      readAtUnixMs: WORKBENCH_NOW,
      inputTasks: input.tasks.items,
    });
    const { container } = render(
      <QueryClientProvider client={queryClient}>
        <CraftThemeProvider>
          <RevocationProvider>
            <MemoryRouter initialEntries={['/workbench?scope=project-one']}>
              <PerspectiveProvider facts={unknownPerspectiveFacts()}>
                <WorkbenchFrame model={model} projectKey="project-one" projectName="Synthetic Project" />
              </PerspectiveProvider>
            </MemoryRouter>
          </RevocationProvider>
        </CraftThemeProvider>
      </QueryClientProvider>,
    );
    const titles: string[] = [];
    const buttons = within(screen.getByRole('group', { name: '链路' })).getAllByRole('button');
    for (const button of buttons) {
      await user.click(button);
      container.querySelectorAll('.obs-chain-node[data-node-kind="commitment"]').forEach((node) => {
        titles.push(node.querySelector('.obs-chain-title')?.textContent ?? '');
      });
    }
    expect(titles).toContain('项目（还没有名称）');
    expect(titles).not.toContain('project-one');
  });

  it('renders a recorded period update with author, time, health and evidence navigation', async () => {
    const user = userEvent.setup();
    const update: WorkbenchPeriodUpdate = {
      id: 'update-one', health: 'risk', sentence: 'A synthetic lead wrote this period update.',
      authorName: 'Project Lead', writtenAtUnixMs: WORKBENCH_NOW,
    };
    renderFrame(myWorkInput(), '/workbench?scope=project-one', update);
    expect(screen.getByText('有风险')).toBeTruthy();
    expect(screen.getByText(update.sentence)).toBeTruthy();
    expect(document.querySelector('.obs-workbench-byline')?.textContent).toContain('Project Lead');
    expect(document.querySelector('.obs-workbench-byline')?.textContent).toContain('2026');
    await user.click(screen.getByRole('button', { name: /这句话的依据/ }));
    expect(screen.getByTestId('location').getAttribute('data-path')).toBe('/updates/update-one');
  });
});

describe('English rendering', () => {
  it('uses the same structure with product-language labels', async () => {
    await i18n.changeLanguage('en-US');
    const { container } = renderFrame();
    expect(screen.getByRole('heading', { level: 1, name: 'My work' })).toBeTruthy();
    expect(screen.getByText('Waiting for you', { selector: '.obs-workbench-health-word' })).toBeTruthy();
    expect(screen.getByText('Calculated from your tasks and Agent reports · Just now')).toBeTruthy();
    expect(container.querySelectorAll('.obs-kpi')).toHaveLength(4);
    expect(screen.getByRole('heading', { name: 'Where work is stuck' })).toBeTruthy();
    expect(screen.getByRole('heading', { name: 'Waiting for you' })).toBeTruthy();
    expect(container.textContent).toContain('Report says the work is ongoing');
    expect(container.textContent).toContain('1 min ago');
    expect(container.textContent).toContain('Synthetic Project');
    expect(document.body.textContent).not.toMatch(/\b(?:undefined|null|NaN)\b/);
  });
});
