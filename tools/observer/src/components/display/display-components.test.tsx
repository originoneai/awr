import { App as AntdApp } from 'antd';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import i18n from '../../i18n';
import { CraftThemeProvider } from '../CraftThemeProvider';
import { DataTable } from './DataTable';
import { DetailPanel } from './DetailPanel';
import { EmptyState } from './EmptyState';
import { KeyValueTable } from './KeyValueTable';
import { Notice } from './Notice';
import { PageHeader } from './PageHeader';
import { RowList } from './RowList';
import { Section } from './Section';
import { Timeline } from './Timeline';

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
