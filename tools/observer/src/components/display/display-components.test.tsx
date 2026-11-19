import { App as AntdApp } from 'antd';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import i18n from '../../i18n';
import { TASK_STATUSES } from '../../core/overview/types';
import { CraftThemeProvider } from '../CraftThemeProvider';
import { AcceptanceList } from './AcceptanceList';
import { Chain } from './Chain';
import { CompletionBar, CompletionLegend } from './CompletionBar';
import { DataTable } from './DataTable';
import { DetailPanel } from './DetailPanel';
import { EmptyState } from './EmptyState';
import { HealthTag, StatusLamp, TaskStatusTag } from './StatusTag';
import { KeyValueTable } from './KeyValueTable';
import { KpiStrip } from './KpiStrip';
import { Notice } from './Notice';
import { PageHeader } from './PageHeader';
import { RowList } from './RowList';
import { Section } from './Section';
import { Timeline } from './Timeline';
import { TodoRail } from './TodoRail';

declare module 'vitest' {
  interface Assertion {
    toBeInTheDocument(): void;
    toHaveAttribute(name: string, value?: string): void;
    toHaveFocus(): void;
  }
}

// Keep this suite dependency-free: these are the only DOM-specific matchers it needs.
expect.extend({
  toBeInTheDocument(received: Element | null) {
    const pass = received !== null && document.contains(received);
    return { pass, message: () => `Expected element ${pass ? 'not ' : ''}to be in the document` };
  },
  toHaveAttribute(received: Element | null, name: string, value?: string) {
    const pass = value === undefined
      ? received?.hasAttribute(name) === true
      : received?.getAttribute(name) === value;
    return { pass, message: () => `Expected attribute ${name} ${pass ? 'not ' : ''}to match ${value ?? 'presence'}` };
  },
  toHaveFocus(received: Element | null) {
    const pass = received !== null && document.activeElement === received;
    return { pass, message: () => `Expected element ${pass ? 'not ' : ''}to have focus` };
  },
});

function renderDisplay(ui: ReactNode) {
  return render(
    <CraftThemeProvider>
      <AntdApp>{ui}</AntdApp>
    </CraftThemeProvider>,
  );
}

beforeEach(async () => {
  await i18n.changeLanguage('zh-CN');
});

describe('Section', () => {
  it('renders title, count including zero, trailing content and body', () => {
    const view = renderDisplay(
      <Section title="交付记录" count={0} extra={<span>只读</span>} padded>
        <p>已经关联三份证据</p>
      </Section>,
    );
    expect(screen.getByRole('heading', { name: '交付记录' })).toBeInTheDocument();
    expect(screen.getByText('0')).toBeInTheDocument();
    expect(screen.getByText('只读')).toBeInTheDocument();
    expect(screen.getByText('已经关联三份证据')).toBeInTheDocument();
    expect(document.querySelector('.obs-section-content')).toHaveAttribute('data-padded', 'true');
    view.rerender(
      <CraftThemeProvider><AntdApp><Section title="交付记录">内容</Section></AntdApp></CraftThemeProvider>,
    );
    expect(screen.queryByText('0')).not.toBeInTheDocument();
    expect(document.querySelector('.obs-section-content')).not.toHaveAttribute('data-padded');
  });
});

describe('PageHeader', () => {
  it('renders compact and detail properties and opens the back destination', async () => {
    const user = userEvent.setup();
    const onBack = vi.fn();
    renderDisplay(
      <PageHeader
        variant="detail"
        title="星桥交付"
        subtitle="本周"
        summary="等待独立复核"
        meta={<span>负责人 林安</span>}
        actions={<button type="button">查看记录</button>}
        onBack={onBack}
      />,
    );
    expect(screen.getByRole('heading', { level: 1, name: '星桥交付' })).toBeInTheDocument();
    expect(screen.getByText('本周')).toBeInTheDocument();
    expect(screen.getByText('等待独立复核')).toBeInTheDocument();
    expect(screen.getByText('负责人 林安')).toBeInTheDocument();
    expect(document.querySelector('.obs-page-header')).toHaveAttribute('data-variant', 'detail');
    await user.click(screen.getByRole('button', { name: '返回' }));
    expect(onBack).toHaveBeenCalledTimes(1);
    expect(screen.getByRole('button', { name: '查看记录' })).toBeInTheDocument();
  });
});

describe('Notice', () => {
  it('renders both tones with title, content and trailing content', () => {
    renderDisplay(
      <>
        <Notice title="读取范围" extra={<span>今天</span>}>仅显示已读取记录</Notice>
        <Notice tone="warning">上游仍在等待复核</Notice>
      </>,
    );
    expect(screen.getByText('读取范围')).toBeInTheDocument();
    expect(screen.getByText('仅显示已读取记录')).toBeInTheDocument();
    expect(screen.getByText('今天')).toBeInTheDocument();
    expect(document.querySelectorAll('.obs-notice')[0]).toHaveAttribute('data-tone', 'info');
    expect(document.querySelectorAll('.obs-notice')[1]).toHaveAttribute('data-tone', 'warning');
  });

  it('gives each tone its own marker: data-tone, antd alert type class and icon', () => {
    renderDisplay(
      <>
        <Notice>常规说明</Notice>
        <Notice tone="warning">需要注意</Notice>
      </>,
    );
    const [info, warning] = document.querySelectorAll<HTMLElement>('.obs-notice');
    expect(info).toHaveAttribute('data-tone', 'info');
    expect(info.classList.contains('ant-alert-info')).toBe(true);
    expect(info.querySelector('.lucide-info')).not.toBeNull();
    expect(warning).toHaveAttribute('data-tone', 'warning');
    expect(warning.classList.contains('ant-alert-warning')).toBe(true);
    expect(warning.classList.contains('ant-alert-info')).toBe(false);
    expect(warning.querySelector('.lucide-triangle-alert')).not.toBeNull();
  });
});

describe('EmptyState', () => {
  it('uses localized defaults and accepts specific content', async () => {
    const chinese = renderDisplay(<EmptyState />);
    expect(screen.getByText('这里还没有内容')).toBeInTheDocument();
    chinese.unmount();
    await i18n.changeLanguage('en-US');
    const custom = renderDisplay(<EmptyState title="No updates" description="An update will appear here" />);
    expect(screen.getByText('No updates')).toBeInTheDocument();
    expect(screen.getByText('An update will appear here')).toBeInTheDocument();
    custom.unmount();
    renderDisplay(<EmptyState />);
    expect(screen.getByText('Nothing here yet')).toBeInTheDocument();
  });

  it('renders no description element when no description is given', async () => {
    await i18n.changeLanguage('zh-CN');
    const bare = renderDisplay(<EmptyState title="暂无更新" />);
    expect(screen.getByText('暂无更新')).toBeInTheDocument();
    expect(document.querySelector('.obs-empty-description')).toBeNull();
    bare.unmount();
    renderDisplay(<EmptyState title="暂无更新" description="负责人记录后会显示" />);
    expect(document.querySelector('.obs-empty-description')?.textContent).toBe('负责人记录后会显示');
  });
});

describe('KeyValueTable', () => {
  it('renders every label and value and only uses the empty state for an empty collection', () => {
    const items = [
      { key: 'owner', label: '负责人', value: '林安' },
      { key: 'next', label: '下一步', value: '查看复核结论' },
    ];
    const view = renderDisplay(<KeyValueTable items={items} />);
    expect(screen.getByText('负责人')).toBeInTheDocument();
    expect(screen.getByText('林安')).toBeInTheDocument();
    expect(screen.getByText('下一步')).toBeInTheDocument();
    expect(screen.getByText('查看复核结论')).toBeInTheDocument();
    expect(screen.queryByTestId('obs-empty-state')).not.toBeInTheDocument();
    view.rerender(
      <CraftThemeProvider><AntdApp><KeyValueTable items={[]} /></AntdApp></CraftThemeProvider>,
    );
    expect(screen.getByTestId('obs-empty-state')).toBeInTheDocument();
  });

  it('keeps each label in dt and each value in dd, not the other way round', () => {
    renderDisplay(
      <KeyValueTable items={[{ key: 'owner', label: '负责人', value: '林安' }]} />,
    );
    const terms = document.querySelectorAll('.obs-key-value dt');
    const descriptions = document.querySelectorAll('.obs-key-value dd');
    expect(terms).toHaveLength(1);
    expect(descriptions).toHaveLength(1);
    expect(terms[0].textContent).toBe('负责人');
    expect(descriptions[0].textContent).toBe('林安');
  });
});

describe('RowList', () => {
  it('renders row props, opens only destinations and never shows empty content alongside rows', async () => {
    const user = userEvent.setup();
    const onOpen = vi.fn();
    renderDisplay(
      <RowList
        items={[
          { key: 'one', title: '接口约定', meta: '今天更新', leading: <span>图标</span>, trailing: '2 天前', selected: true, onOpen },
          { key: 'two', title: '交付说明', meta: '只读资料' },
        ]}
      />,
    );
    const button = screen.getByRole('button', { name: /接口约定/ });
    expect(button).toHaveAttribute('aria-current', 'true');
    expect(screen.getByText('今天更新')).toBeInTheDocument();
    expect(screen.getByText('2 天前')).toBeInTheDocument();
    const staticRow = screen.getByText('交付说明').closest('.obs-row-control');
    expect(staticRow?.tagName).toBe('DIV');
    expect(staticRow?.querySelector('svg')).toBeNull();
    expect(screen.queryByTestId('obs-empty-state')).not.toBeInTheDocument();
    await user.click(button);
    expect(onOpen).toHaveBeenCalledTimes(1);
  });

  it('renders the empty state only when there are no rows', () => {
    renderDisplay(<RowList items={[]} empty={<span>没有资料</span>} />);
    expect(screen.getByText('没有资料')).toBeInTheDocument();
    expect(document.querySelector('.obs-row-list')).toBeNull();
  });
});

describe('DataTable', () => {
  const columns = [
    { key: 'work', title: '工作' },
    { key: 'owner', title: '负责人' },
  ];

  it('omits the complete arrow column when no row can open', () => {
    renderDisplay(
      <DataTable
        columns={columns}
        rows={[{ key: 'static', cells: ['交付说明', '林安'] }]}
      />,
    );
    expect(document.querySelector('.obs-table-arrow-column')).toBeNull();
    expect(screen.getByRole('columnheader', { name: '工作' })).toBeInTheDocument();
    expect(screen.getByRole('columnheader', { name: '负责人' })).toBeInTheDocument();
    expect(screen.getAllByRole('columnheader')).toHaveLength(2);
  });

  it('keeps a fixed trailing arrow column, leaves static rows without arrows, and opens with Enter', async () => {
    const user = userEvent.setup();
    const onOpen = vi.fn();
    renderDisplay(
      <DataTable
        columns={columns}
        rows={[
          { key: 'open', cells: ['核对页面', '林安'], onOpen },
          { key: 'static', cells: ['交付说明', '周宁'] },
        ]}
      />,
    );
    const openRow = screen.getByText('核对页面').closest('tr')!;
    const staticRow = screen.getByText('交付说明').closest('tr')!;
    expect(openRow).toHaveAttribute('data-openable', 'true');
    expect(openRow).toHaveAttribute('tabindex', '0');
    expect(staticRow).not.toHaveAttribute('tabindex');
    expect(document.querySelectorAll('.obs-table-arrow-column')).toHaveLength(3);
    expect(openRow.querySelector('.obs-table-arrow-column svg')).not.toBeNull();
    expect(staticRow.querySelector('.obs-table-arrow-column svg')).toBeNull();
    openRow.focus();
    expect(openRow).toHaveFocus();
    await user.keyboard('{Enter}');
    expect(onOpen).toHaveBeenCalledTimes(1);
    await user.keyboard('a');
    expect(onOpen).toHaveBeenCalledTimes(1);
  });

  it('does not open the row when a click lands on a nested interactive element', async () => {
    const user = userEvent.setup();
    const onOpen = vi.fn();
    const onNested = vi.fn();
    renderDisplay(
      <DataTable
        columns={columns}
        rows={[{
          key: 'open',
          cells: [<button key="nested" type="button" onClick={onNested}>查看记录</button>, '林安'],
          onOpen,
        }]}
      />,
    );
    await user.click(screen.getByRole('button', { name: '查看记录' }));
    expect(onNested).toHaveBeenCalledTimes(1);
    expect(onOpen).not.toHaveBeenCalled();
    await user.click(screen.getByText('林安'));
    expect(onOpen).toHaveBeenCalledTimes(1);
  });

  it('does not show an empty state beside populated rows and does show it for no rows', () => {
    const view = renderDisplay(
      <DataTable columns={columns} rows={[{ key: 'one', cells: ['核对页面', '林安'] }]} />,
    );
    expect(screen.queryByTestId('obs-empty-state')).not.toBeInTheDocument();
    view.rerender(
      <CraftThemeProvider><AntdApp><DataTable columns={columns} rows={[]} /></AntdApp></CraftThemeProvider>,
    );
    expect(screen.getByTestId('obs-empty-state')).toBeInTheDocument();
  });
});

describe('DetailPanel', () => {
  it('renders counts and extra content, then updates content and footer after tab switching', async () => {
    const user = userEvent.setup();
    renderDisplay(
      <DetailPanel
        extra={<span>记录于今天</span>}
        tabs={[
          { key: 'progress', label: '进展', count: 3, content: <p>三种视口已完成</p>, footer: '进展页脚' },
          { key: 'evidence', label: '依据', count: 2, content: <p>浏览器检查记录</p>, footer: '依据页脚' },
        ]}
      />,
    );
    expect(screen.getByText('三种视口已完成')).toBeInTheDocument();
    expect(screen.queryByText('浏览器检查记录')).not.toBeInTheDocument();
    expect(screen.getByText('进展页脚')).toBeInTheDocument();
    expect(screen.getByText('记录于今天')).toBeInTheDocument();
    await user.click(screen.getByRole('tab', { name: '依据2' }));
    expect(screen.getByText('浏览器检查记录')).toBeInTheDocument();
    expect(screen.queryByText('三种视口已完成')).not.toBeInTheDocument();
    expect(screen.getByText('依据页脚')).toBeInTheDocument();
    expect(screen.queryByText('进展页脚')).not.toBeInTheDocument();
  });

  it('honors a controlled activeKey over internal state', async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    const tabs = [
      { key: 'progress', label: '进展', count: 3, content: <p>三种视口已完成</p>, footer: '进展页脚' },
      { key: 'evidence', label: '依据', count: 2, content: <p>浏览器检查记录</p>, footer: '依据页脚' },
    ];
    const view = renderDisplay(<DetailPanel activeKey="evidence" onChange={onChange} tabs={tabs} />);
    expect(screen.getByText('浏览器检查记录')).toBeInTheDocument();
    expect(screen.getByText('依据页脚')).toBeInTheDocument();
    expect(screen.queryByText('三种视口已完成')).not.toBeInTheDocument();
    await user.click(screen.getByRole('tab', { name: '进展3' }));
    expect(onChange).toHaveBeenCalledWith('progress');
    expect(screen.getByText('浏览器检查记录')).toBeInTheDocument();
    expect(screen.queryByText('三种视口已完成')).not.toBeInTheDocument();
    view.rerender(
      <CraftThemeProvider><AntdApp><DetailPanel activeKey="progress" onChange={onChange} tabs={tabs} /></AntdApp></CraftThemeProvider>,
    );
    expect(screen.getByText('三种视口已完成')).toBeInTheDocument();
    expect(screen.getByText('进展页脚')).toBeInTheDocument();
    expect(screen.queryByText('浏览器检查记录')).not.toBeInTheDocument();
  });
});

describe('Timeline', () => {
  it('renders time, title, detail and tone for every event', () => {
    renderDisplay(
      <Timeline
        items={[
          { key: 'one', time: '今天 10:20', title: '完成联调', detail: '下一步：复核', tone: 'positive' },
          { key: 'two', time: '今天 11:20', title: '开始复核', tone: 'current' },
        ]}
      />,
    );
    expect(screen.getByText('今天 10:20')).toBeInTheDocument();
    expect(screen.getByText('完成联调')).toBeInTheDocument();
    expect(screen.getByText('下一步：复核')).toBeInTheDocument();
    expect(document.querySelectorAll('.obs-timeline-item')[0]).toHaveAttribute('data-tone', 'positive');
    expect(document.querySelectorAll('.obs-timeline-item')[1]).toHaveAttribute('data-tone', 'current');
    expect(screen.queryByTestId('obs-empty-state')).not.toBeInTheDocument();
  });

  it('uses the empty state only when there are no events', () => {
    renderDisplay(<Timeline items={[]} />);
    expect(screen.getByTestId('obs-empty-state')).toBeInTheDocument();
  });
});

describe('KpiStrip', () => {
  it('renders label, value, unit and change for every cell and marks the emphasized one', () => {
    renderDisplay(
      <KpiStrip
        items={[
          {
            key: 'verified',
            label: '本周通过核验',
            value: '5',
            unit: '件',
            change: { text: '上周 3 件', tone: 'good', trend: 'up' },
          },
          { key: 'mine', label: '等你处理', value: '2', unit: '件', emphasized: true },
        ]}
      />,
    );
    expect(screen.getByText('本周通过核验')).toBeInTheDocument();
    expect(screen.getByText('5')).toBeInTheDocument();
    expect(screen.getAllByText('件', { selector: 'small' })).toHaveLength(2);
    expect(screen.getByText('上周 3 件')).toBeInTheDocument();
    const change = document.querySelector('.obs-kpi-change');
    expect(change).toHaveAttribute('data-tone', 'good');
    const cells = document.querySelectorAll('.obs-kpi');
    expect(cells).toHaveLength(2);
    expect(cells[0]).not.toHaveAttribute('data-emphasized');
    expect(cells[1]).toHaveAttribute('data-emphasized', 'true');
    expect(screen.getByRole('group', { name: '关键数字' })).toBeInTheDocument();
  });

  it('opens only the cells that carry a destination, with a readable title and hover chevron', async () => {
    const user = userEvent.setup();
    const onOpen = vi.fn();
    renderDisplay(
      <KpiStrip
        items={[
          { key: 'open', label: '本周通过核验', value: '5', onOpen },
          { key: 'static', label: '另一个数字', value: '9' },
        ]}
      />,
    );
    const button = screen.getByRole('button', { name: /本周通过核验/ });
    expect(button).toHaveAttribute('title', '点开看是哪几件');
    expect(button.querySelector('.obs-kpi-hint svg')).not.toBeNull();
    await user.click(button);
    expect(onOpen).toHaveBeenCalledTimes(1);
    const staticCell = screen.getByText('另一个数字').closest('.obs-kpi')!;
    expect(staticCell.tagName).toBe('DIV');
    expect(staticCell.querySelector('.obs-kpi-hint')).toBeNull();
  });

  it('opens a cell with the keyboard', async () => {
    const user = userEvent.setup();
    const onOpen = vi.fn();
    renderDisplay(
      <KpiStrip items={[{ key: 'open', label: '本周通过核验', value: '5', onOpen }]} />,
    );
    const button = screen.getByRole('button', { name: /本周通过核验/ });
    button.focus();
    await user.keyboard('{Enter}');
    expect(onOpen).toHaveBeenCalledTimes(1);
  });
});

describe('StatusTag', () => {
  it('renders every health grade with its word and one-line meaning on hover', () => {
    renderDisplay(
      <>
        {(['ok', 'risk', 'off', 'none', 'info'] as const).map((status) => (
          <HealthTag key={status} status={status} />
        ))}
      </>,
    );
    const expected: Record<string, string> = {
      ok: '正常',
      risk: '有风险',
      off: '偏离',
      none: '没有更新',
      info: '刚接管',
    };
    for (const [status, word] of Object.entries(expected)) {
      const tag = screen.getByText(word).closest('.obs-tag')!;
      expect(tag).toBeInTheDocument();
      expect(tag.getAttribute('title')?.length).toBeGreaterThan(3);
      if (status === 'none') {
        expect(tag).not.toHaveAttribute('data-tone');
        expect(tag.querySelector('.obs-lamp')).toHaveAttribute('data-ring', 'true');
      } else {
        expect(tag).toHaveAttribute('data-tone', status);
      }
    }
  });

  it('renders all nine task statuses plus unknown with tone, lamp and hover meaning', () => {
    renderDisplay(
      <>
        {[...TASK_STATUSES, 'unknown' as const].map((status) => (
          <TaskStatusTag key={status} status={status} />
        ))}
      </>,
    );
    const words: Record<string, string> = {
      verified: '核验过',
      claimed: '标了完成，没核验',
      returned: '复核退回',
      review: '等复核',
      running: '在做',
      stalled: '停着',
      waiting: '在等',
      ready: '可以开始',
      planned: '没开始',
      unknown: '未知',
    };
    const tones: Record<string, string> = {
      verified: 'ok',
      returned: 'off',
      review: 'risk',
      stalled: 'risk',
      running: 'info',
    };
    // Review R8: the lamp colour is the point of the tag; pin it for all ten statuses.
    const lamps: Record<string, string> = {
      verified: 'done',
      claimed: 'record',
      returned: 'blocked',
      review: 'stalled',
      running: 'running',
      stalled: 'stalled',
      waiting: 'waiting',
      ready: 'ready',
      planned: 'waiting',
      unknown: 'none',
    };
    const rings = new Set(['claimed', 'waiting', 'ready', 'planned', 'unknown']);
    for (const [status, word] of Object.entries(words)) {
      const tag = screen.getByText(word).closest('.obs-tag')!;
      expect(tag, status).toBeInTheDocument();
      expect(tag.getAttribute('title'), `${status} hover meaning`).not.toBe(word);
      if (tones[status] === undefined) {
        expect(tag, status).not.toHaveAttribute('data-tone');
      } else {
        expect(tag, status).toHaveAttribute('data-tone', tones[status]);
      }
      const lamp = tag.querySelector('.obs-lamp')!;
      expect(lamp, `${status} lamp colour`).toHaveAttribute('data-lamp', lamps[status]);
      if (rings.has(status)) {
        expect(lamp, status).toHaveAttribute('data-ring', 'true');
      } else {
        expect(lamp, status).not.toHaveAttribute('data-ring');
      }
    }
  });

  it('pins the lamp colour for every health grade', () => {
    renderDisplay(
      <>
        {(['ok', 'risk', 'off', 'none', 'info'] as const).map((status) => (
          <HealthTag key={status} status={status} />
        ))}
      </>,
    );
    // Review R8: health lamps reuse the task lamp scale; none stays the quiet ring.
    const lamps: Record<string, string> = {
      ok: 'done',
      risk: 'stalled',
      off: 'blocked',
      none: 'none',
      info: 'running',
    };
    for (const [status, lampState] of Object.entries(lamps)) {
      const tag = document.querySelector(`.obs-lamp[data-lamp="${lampState}"]`)?.closest('.obs-tag');
      expect(tag, `${status} renders the ${lampState} lamp`).not.toBeNull();
      expect(tag!.querySelectorAll('.obs-lamp')).toHaveLength(1);
    }
    const allLamps = document.querySelectorAll('.obs-tag .obs-lamp');
    expect([...allLamps].map((lamp) => lamp.getAttribute('data-lamp')).sort())
      .toEqual(Object.values(lamps).sort());
  });

  it('keeps the live pulse to a running status and translates the words', async () => {
    renderDisplay(
      <>
        <TaskStatusTag status="running" live />
        <TaskStatusTag status="running" />
        <TaskStatusTag status="stalled" live />
      </>,
    );
    const lamps = document.querySelectorAll('.obs-lamp');
    expect(lamps[0]).toHaveAttribute('data-live', 'true');
    expect(lamps[1]).not.toHaveAttribute('data-live');
    expect(lamps[2]).not.toHaveAttribute('data-live');
    await i18n.changeLanguage('en-US');
    renderDisplay(<TaskStatusTag status="verified" />);
    expect(screen.getByText('Verified complete')).toBeInTheDocument();
    await i18n.changeLanguage('zh-CN');
  });

  it('exposes the lamp states so other components share one mapping', () => {
    renderDisplay(
      <>
        <StatusLamp lamp="done" />
        <StatusLamp lamp="waiting" />
      </>,
    );
    const lamps = document.querySelectorAll('.obs-lamp');
    expect(lamps[0]).toHaveAttribute('data-lamp', 'done');
    expect(lamps[1]).toHaveAttribute('data-lamp', 'waiting');
    expect(lamps[1]).toHaveAttribute('data-ring', 'true');
  });
});

describe('CompletionBar', () => {
  it('draws one segment per non-zero grade, sized by its count, with a spoken label', () => {
    renderDisplay(<CompletionBar counts={{ verified: 6, reported: 0, inProgress: 2, notStarted: 1 }} />);
    const bar = document.querySelector('.obs-seg4')!;
    expect(bar).toHaveAttribute('role', 'img');
    expect(bar.getAttribute('aria-label')).toBe('核验过 6，标了完成 0，在做 2，没开始 1');
    const segments = bar.querySelectorAll('.obs-seg4-part');
    expect(segments).toHaveLength(3);
    expect(segments[0]).toHaveAttribute('data-seg', 'verified');
    expect((segments[0] as HTMLElement).style.flexGrow).toBe('6');
    expect(segments[0]).not.toHaveAttribute('data-seg', 'reported');
  });

  it('renders the quiet summary line unless it is turned off', () => {
    const view = renderDisplay(<CompletionBar counts={{ verified: 3, reported: 1, inProgress: 0, notStarted: 0 }} />);
    expect(screen.getByText('核验过 3 · 标了完成 1 · 共 4')).toBeInTheDocument();
    view.unmount();
    renderDisplay(<CompletionBar showSummary={false} counts={{ verified: 3, reported: 1, inProgress: 0, notStarted: 0 }} />);
    expect(document.querySelector('.obs-completion-summary')).toBeNull();
  });

  it('keeps the prototype segment order and every number in the summary with all counts non-zero', () => {
    // Review R8: the order is verified, reported, in progress, not started; the total
    // includes the not-started count.
    renderDisplay(<CompletionBar counts={{ verified: 6, reported: 2, inProgress: 1, notStarted: 1 }} />);
    const segments = [...document.querySelectorAll('.obs-seg4-part')].map((part) => part.getAttribute('data-seg'));
    expect(segments).toEqual(['verified', 'reported', 'inProgress', 'notStarted']);
    const summary = document.querySelector('.obs-completion-summary')!.textContent;
    expect(summary).toBe('核验过 6 · 标了完成 2 · 共 10');
    expect(summary).toContain('6');
    expect(summary).toContain('2');
    expect(summary).toContain('10');
  });

  it('renders the four-word legend beside the bar', () => {
    renderDisplay(
      <>
        <CompletionLegend />
        <CompletionLegend variant="wrapped" />
      </>,
    );
    for (const word of ['核验过', '标了完成，没核验', '在做', '没开始']) {
      expect(screen.getAllByText(word).length).toBe(2);
    }
    const swatches = document.querySelectorAll('.obs-legend-swatch');
    expect(swatches).toHaveLength(8);
    expect(document.querySelectorAll('.obs-legend')).toHaveLength(2);
  });
});

describe('Chain', () => {
  const nodes = [
    { id: 'demo:root', kind: 'root' as const, root: true, tone: 'negative' as const, task: null, commitment: null },
    { id: 'demo:task:a', kind: 'task' as const, root: false, tone: null, task: null, commitment: null },
    { id: 'demo:commitment:c', kind: 'commitment' as const, root: false, tone: null, task: null, commitment: { kind: 'goal' as const } },
  ];
  const connectors = [
    { fromNodeId: 'demo:root', toNodeId: 'demo:task:a', kind: 'blocks' as const },
    { fromNodeId: 'demo:task:a', toNodeId: 'demo:commitment:c', kind: 'affects' as const },
  ];

  function present(id: string) {
    switch (id) {
      case 'demo:root':
        return {
          kicker: '卡在这里',
          title: '复核队列',
          ownerName: '周宁',
          queue: [{ title: '接口第 3 版', days: 2 }, { title: '提交校验', days: null }],
          queueNote: { queued: 2, oldestDays: 2 },
        };
      case 'demo:task:a':
        return { kicker: '任务', title: '接口第 3 版', meta: '陈禾 · 等复核 2 天', status: 'waiting' as const };
      default:
        return { kicker: '目标', title: '本周要交的', progress: { done: 4, total: 6 } };
    }
  }

  it('marks the root red, renders connectors between nodes and speaks queue and progress', () => {
    renderDisplay(
      <Chain nodes={nodes} connectors={connectors} present={(node) => present(node.id)} footer={<span>后面 3 件事可以接着走</span>} />,
    );
    const root = screen.getByText('复核队列').closest('.obs-chain-node')!;
    expect(root.classList.contains('obs-chain-node-root')).toBe(true);
    const taskNode = screen.getByText('接口第 3 版', { selector: '.obs-chain-title' }).closest('.obs-chain-node')!;
    expect(taskNode.querySelector('.obs-lamp')).toHaveAttribute('data-lamp', 'waiting');
    const connectorsRendered = document.querySelectorAll('.obs-chain-connector');
    expect(connectorsRendered).toHaveLength(2);
    expect(connectorsRendered[0]).toHaveAttribute('data-hot', 'true');
    expect(connectorsRendered[0]).toHaveAttribute('data-kind', 'blocks');
    expect(connectorsRendered[1]).not.toHaveAttribute('data-hot');
    expect(screen.getAllByText('卡住')).toHaveLength(2);
    expect(screen.getByText('2 件在排队，最久 2 天')).toBeInTheDocument();
    expect(screen.getByText('2 天', { selector: 'em' })).toBeInTheDocument();
    expect(screen.getByText('已核验 4 / 6')).toBeInTheDocument();
    expect(screen.getByText('后面 3 件事可以接着走')).toBeInTheDocument();
    expect(screen.getByRole('group', { name: '卡点链路' })).toBeInTheDocument();
  });

  it('renders openable nodes as buttons that report their core node, and quiet nodes as divs', async () => {
    const user = userEvent.setup();
    const onOpen = vi.fn();
    renderDisplay(
      <Chain
        nodes={nodes}
        connectors={connectors}
        present={(node) => ({ ...present(node.id), open: node.kind !== 'commitment' })}
        onOpen={onOpen}
      />,
    );
    const rootNode = screen.getByText('复核队列').closest('.obs-chain-node')!;
    expect(rootNode.tagName).toBe('BUTTON');
    await user.click(rootNode);
    expect(onOpen).toHaveBeenCalledWith(nodes[0]);
    const endNode = screen.getByText('本周要交的').closest('.obs-chain-node')!;
    expect(endNode.tagName).toBe('DIV');
  });

  it('falls back to a queue footer without an age when the oldest wait is unknown', () => {
    renderDisplay(
      <Chain
        nodes={nodes.slice(0, 1)}
        connectors={[]}
        present={() => ({ title: '复核队列', queueNote: { queued: 3, oldestDays: null } })}
      />,
    );
    expect(screen.getByText('3 件在排队')).toBeInTheDocument();
  });

  it('renders the you badge and the hot outline only when the view asks for them', () => {
    const view = renderDisplay(
      <Chain
        nodes={nodes}
        connectors={connectors}
        present={(node) => ({
          ...present(node.id),
          you: node.id === 'demo:task:a',
          highlighted: node.id === 'demo:task:a',
        })}
      />,
    );
    const taskNode = screen.getByText('接口第 3 版', { selector: '.obs-chain-title' }).closest('.obs-chain-node')!;
    expect(taskNode.querySelector('.obs-chain-you')?.textContent).toBe('你在这里');
    expect(taskNode).toHaveAttribute('data-highlighted', 'true');
    const rootNode = screen.getByText('复核队列').closest('.obs-chain-node')!;
    expect(rootNode.querySelector('.obs-chain-you')).toBeNull();
    expect(rootNode).not.toHaveAttribute('data-highlighted');
    view.rerender(
      <CraftThemeProvider>
        <AntdApp>
          <Chain nodes={nodes} connectors={connectors} present={(node) => present(node.id)} />
        </AntdApp>
      </CraftThemeProvider>,
    );
    expect(document.querySelector('.obs-chain-you')).toBeNull();
    expect(document.querySelector('.obs-chain-node[data-highlighted]')).toBeNull();
  });
});

describe('TodoRail', () => {
  const items = ['第一件', '第二件', '第三件', '第四件', '第五件'].map((title, index) => ({
    key: `item-${index}`,
    kind: index % 2 === 0 ? '回答' : '决定',
    title,
    from: index === 0 ? '来自「页面联调」' : undefined,
    age: index === 0 ? '2 天' : '今天',
    detail: index === 0 ? '回答之后才能继续做文案部分' : undefined,
    onOpen: vi.fn(),
  }));

  it('expands only the first card, collapses the rest and counts everything', () => {
    renderDisplay(<TodoRail title="等你的事" items={items} onViewAll={() => {}} />);
    expect(screen.getByText('等你的事')).toBeInTheDocument();
    expect(document.querySelector('.obs-todo-count')?.textContent).toBe('5');
    expect(screen.getByText('来自「页面联调」')).toBeInTheDocument();
    expect(screen.getByText('回答之后才能继续做文案部分')).toBeInTheDocument();
    expect(screen.getByText('在你的 Agent 里处理')).toBeInTheDocument();
    expect(screen.getByText('看这件事')).toBeInTheDocument();
    expect(screen.getAllByText('第二件', { selector: '.obs-todo-min-title' })).toHaveLength(1);
    expect(screen.queryByText('第四件')).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: /还有 2 件/ })).toBeInTheDocument();
  });

  it('opens a card, the full list and hides the more line within the limit', async () => {
    const user = userEvent.setup();
    const onViewAll = vi.fn();
    const view = renderDisplay(<TodoRail title="等你的事" items={items} onViewAll={onViewAll} />);
    await user.click(screen.getByRole('button', { name: /还有 2 件/ }));
    expect(onViewAll).toHaveBeenCalledTimes(1);
    await user.click(screen.getByText('第二件'));
    expect(items[1].onOpen).toHaveBeenCalledTimes(1);
    view.unmount();
    renderDisplay(<TodoRail title="等你的事" items={items.slice(0, 3)} />);
    expect(screen.queryByRole('button', { name: /还有/ })).not.toBeInTheDocument();
    expect(screen.getByText('第三件')).toBeInTheDocument();
  });

  it('shows the shared empty state when nothing is pending', () => {
    renderDisplay(<TodoRail title="等你的事" items={[]} />);
    expect(screen.getByTestId('obs-empty-state')).toBeInTheDocument();
  });
});

describe('AcceptanceList', () => {
  it('renders every criterion with its evidence, icon and status word', () => {
    renderDisplay(
      <AcceptanceList
        items={[
          { key: 'lookup', title: '按编号和手机号都能查到', evidence: '测试 9 条通过', status: 'supported' },
          { key: 'repeat', title: '重复提交不会重复下单', evidence: '复核时发现没有验证', status: 'failed' },
          { key: 'keep', title: '提交失败时保留已填内容', evidence: '要等接口第 3 版', status: 'waiting' },
          { key: 'scenes', title: '走完 8 个场景', status: 'notStarted' },
        ]}
      />,
    );
    const words: Record<string, string> = {
      supported: '有依据',
      failed: '没通过',
      waiting: '在等',
      notStarted: '还没有开始',
    };
    // Review R8: one distinct lucide glyph per criterion status.
    const icons: Record<string, string> = {
      supported: 'lucide-circle-check',
      failed: 'lucide-circle-x',
      waiting: 'lucide-clock',
      notStarted: 'lucide-circle',
    };
    for (const [status, word] of Object.entries(words)) {
      const row = screen.getByText(word).closest('.obs-acceptance-item')!;
      expect(row).toHaveAttribute('data-status', status);
      expect(row.querySelector(`.${icons[status]}`), `${status} icon`).not.toBeNull();
    }
    expect(screen.getByText('测试 9 条通过')).toBeInTheDocument();
    expect(screen.getByRole('list', { name: '完成标准' })).toBeInTheDocument();
  });

  it('uses the empty state only when there are no criteria', () => {
    renderDisplay(<AcceptanceList items={[]} />);
    expect(screen.getByTestId('obs-empty-state')).toBeInTheDocument();
  });
});
