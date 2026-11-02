import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it } from 'vitest';
import i18n from '../i18n';
import { formatUnixMs } from '../lib/format';
import type { TeamFreshness } from '../lib/types';
import { FreshnessBadge } from './FreshnessBadge';

afterEach(async () => {
  await i18n.changeLanguage('zh-CN');
});

function freshness(overrides: Partial<TeamFreshness> = {}): TeamFreshness {
  return {
    report_received: { status: 'measured', at_unix_ms: 1761840000000 },
    authoritative: { status: 'stale', at_unix_ms: 1761830000000, stale_after_ms: 120000, age_ms: 130000 },
    page_read: { status: 'failed', error_code: 'ReadTimeout' },
    ...overrides,
  };
}

describe('FreshnessBadge (four-state vocabulary from observation-contract)', () => {
  it('renders all three plane states without engineering source names', () => {
    render(<FreshnessBadge freshness={freshness()} />);
    expect(screen.getByTestId('freshness-report_received').textContent).toContain('已更新');
    expect(screen.getByTestId('freshness-authoritative').textContent).toContain('可能已过期');
    expect(screen.getByTestId('freshness-page_read').textContent).toContain('读取失败');
    expect(screen.getByTestId('freshness-badge').textContent).not.toContain('上报接收');
    expect(screen.getByTestId('freshness-badge').textContent).not.toContain('权威测量');
    expect(screen.getByTestId('freshness-badge').textContent).not.toContain('页面读取');
  });

  it('labels every mixed plane state with its human source', () => {
    render(<FreshnessBadge freshness={freshness({
      report_received: { status: 'not_measured' },
      page_read: { status: 'measured', at_unix_ms: 1761850000000 },
    })} />);
    expect(screen.getByTestId('freshness-report_received').textContent).toBe('Agent 汇报：尚未读取');
    expect(screen.getByTestId('freshness-authoritative').textContent).toBe('系统核对：可能已过期');
    expect(screen.getByTestId('freshness-page_read').textContent).toBe('本页读取：已更新');
    expect(screen.getByTestId('freshness-badge').textContent).not.toContain('上报接收');
    expect(screen.getByTestId('freshness-badge').textContent).not.toContain('权威测量');
  });

  it('shows one latest-read sentence using the latest factual time', () => {
    render(<FreshnessBadge freshness={freshness({ page_read: { status: 'measured', at_unix_ms: 1761850000000 } })} />);
    const badge = screen.getByTestId('freshness-badge');
    expect(badge.textContent).toContain('最近一次读取：');
    expect(badge.textContent).toContain(formatUnixMs(1761850000000, 'zh-CN'));
    expect(badge.textContent).not.toContain(formatUnixMs(1761840000000, 'zh-CN'));
  });

  it('keeps the failed error code and plane names in technical details', async () => {
    render(<FreshnessBadge freshness={freshness()} />);
    const badge = screen.getByTestId('freshness-badge');
    expect(badge.textContent).not.toContain('ReadTimeout');
    await userEvent.setup().click(screen.getByText('技术细节'));
    expect(await screen.findByText(/ReadTimeout/)).toBeTruthy();
    expect(screen.getByText(/上报接收/)).toBeTruthy();
    expect(screen.getByText(/权威测量/)).toBeTruthy();
    expect(screen.getByText(/页面读取/)).toBeTruthy();
  });

  it('renders not_measured without inventing a read time', () => {
    render(<FreshnessBadge freshness={freshness({
      report_received: { status: 'not_measured' },
      authoritative: { status: 'not_measured' },
      page_read: { status: 'not_measured' },
    })} />);
    const badge = screen.getByTestId('freshness-badge');
    expect(badge.textContent).toContain('最近一次读取：尚未读取');
    expect(badge.textContent).not.toContain(String(new Date(1761840000000).getFullYear()));
  });

  it('uses human latest-read copy bilingually', async () => {
    render(<FreshnessBadge freshness={freshness()} />);
    await i18n.changeLanguage('en-US');
    expect(screen.getByTestId('freshness-badge').textContent).toContain('Latest read:');
    expect(screen.getByTestId('freshness-badge').textContent).not.toContain('Page read');
    expect(screen.getByTestId('freshness-report_received').textContent).toContain('Updated');
    expect(screen.getByTestId('freshness-authoritative').textContent).toContain('May be outdated');
    expect(screen.getByTestId('freshness-page_read').textContent).toContain('Read failed');
  });
});
