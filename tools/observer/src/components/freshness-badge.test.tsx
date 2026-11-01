import { render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vitest';
import i18n from '../i18n';
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
  it('renders all three planes', () => {
    render(<FreshnessBadge freshness={freshness()} />);
    expect(screen.getByTestId('freshness-report_received')).toBeTruthy();
    expect(screen.getByTestId('freshness-authoritative')).toBeTruthy();
    expect(screen.getByTestId('freshness-page_read')).toBeTruthy();
  });

  it('renders measured with its time', () => {
    render(<FreshnessBadge freshness={freshness()} />);
    const plane = screen.getByTestId('freshness-report_received');
    expect(plane.textContent).toContain('已测量');
    expect(plane.textContent).toContain(String(new Date(1761840000000).getFullYear()));
  });

  it('renders stale distinctly from measured', () => {
    render(<FreshnessBadge freshness={freshness()} />);
    expect(screen.getByTestId('freshness-authoritative').textContent).toContain('已过期');
  });

  it('renders failed with the error code', () => {
    render(<FreshnessBadge freshness={freshness()} />);
    const plane = screen.getByTestId('freshness-page_read');
    expect(plane.textContent).toContain('读取失败');
    expect(plane.textContent).toContain('ReadTimeout');
  });

  it('renders not_measured without inventing a time', () => {
    render(
      <FreshnessBadge
        freshness={freshness({ report_received: { status: 'not_measured' } })}
      />,
    );
    const plane = screen.getByTestId('freshness-report_received');
    expect(plane.textContent).toContain('未测量');
    expect(plane.textContent).not.toContain(String(new Date(1761840000000).getFullYear()));
  });

  it('labels planes bilingually', async () => {
    render(<FreshnessBadge freshness={freshness()} />);
    expect(screen.getByTestId('freshness-page_read').textContent).toContain('页面读取');
    await i18n.changeLanguage('en-US');
    expect(screen.getByTestId('freshness-page_read').textContent).toContain('Page read');
    expect(screen.getByTestId('freshness-report_received').textContent).toContain('measured');
    expect(screen.getByTestId('freshness-authoritative').textContent).toContain('stale');
    expect(screen.getByTestId('freshness-page_read').textContent).toContain('failed');
  });
});
