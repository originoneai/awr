import { QueryClientProvider } from '@tanstack/react-query';
import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter, useLocation } from 'react-router';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { deriveOverview } from '../../core/overview';
import type { OverviewTask } from '../../core/overview';
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
  });
  return render(
    <QueryClientProvider client={queryClient}>
      <CraftThemeProvider>
        <RevocationProvider>
          <MemoryRouter initialEntries={[initialEntry]}>
            <PerspectiveProvider facts={unknownPerspectiveFacts()}>
              <WorkbenchFrame model={model} projectKey="project-one" projectName="Synthetic Project" />
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
    expect(screen.getByText('没有更新')).toBeTruthy();
    expect(screen.getByText(/程序按事实生成/)).toBeTruthy();
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

  it.each([
    ['本周通过核验', 1],
    ['Agent 在跑', 1],
    ['我在等', 3],
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

  it('removes the recorded owner suffix from an Agent display name', () => {
    const base = myWorkInput();
    const input = {
      ...base,
      tasks: {
        ...base.tasks,
        items: (base.tasks.items ?? []).map((task: OverviewTask) => task.id === 'mine-running'
          ? { ...task, agentName: 'Claude 桌面（林悦的 Agent）' }
          : task),
      },
    };
    const { container } = renderFrame(input);
    const agentCell = container.querySelector('.obs-data-table tbody tr td:first-child')!;
    expect(agentCell.textContent).toBe('Claude 桌面');
    expect(container.querySelector('thead th:nth-child(3) span')?.getAttribute('title')).toBe('报告正文尚未读取');
    expect(container.querySelector('thead th:nth-child(5) span')?.getAttribute('title')).toBe('报告时间尚未读取');
    expect(container.querySelector('tbody tr td:nth-child(3)')?.textContent).toBe('—');
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
    expect(screen.getByText('No update')).toBeTruthy();
    expect(screen.getByText(/Generated from recorded facts/)).toBeTruthy();
    expect(container.querySelectorAll('.obs-kpi')).toHaveLength(4);
    expect(screen.getByRole('heading', { name: 'Where work is stuck' })).toBeTruthy();
    expect(screen.getByRole('heading', { name: 'Waiting for you' })).toBeTruthy();
    expect(document.body.textContent).not.toMatch(/\b(?:undefined|null|NaN)\b/);
  });
});
