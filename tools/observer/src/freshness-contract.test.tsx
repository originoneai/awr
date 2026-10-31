import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import './i18n';
import { FreshnessBadge } from './components/FreshnessBadge';
import { ObservationForbidden, PartialFailure } from './components/states';
import type { TeamFreshness } from './lib/types';

function freshnessWith(status: string): TeamFreshness {
  return {
    report_received: { status, at_unix_ms: 1761840000000 } as TeamFreshness['report_received'],
    authoritative: { status: 'not_measured' },
    page_read: { status: 'not_measured' },
  };
}

describe('freshness behavior contract', () => {
  it.each([
    ['measured', 'success'],
    ['stale', 'warning'],
    ['failed', 'error'],
    ['not_measured', 'default'],
  ])('renders %s with the %s state color', (status, color) => {
    render(<FreshnessBadge freshness={freshnessWith(status)} />);
    expect(screen.getByTestId('freshness-report_received').querySelector(`.ant-tag-${color}`)).not.toBeNull();
  });

  it.each([undefined, 'future-state'])('treats a missing or unknown plane (%s) as not measured', (status) => {
    const freshness = freshnessWith('measured');
    freshness.report_received = status === undefined
      ? undefined as unknown as TeamFreshness['report_received']
      : { status } as TeamFreshness['report_received'];
    const { container } = render(<FreshnessBadge freshness={freshness} />);
    const plane = screen.getByTestId('freshness-report_received');
    expect(plane.textContent).toContain('未测量');
    expect(plane.querySelector('.ant-tag-default')).not.toBeNull();
    expect(container.querySelector('.ant-tag-success')).toBeNull();
  });

  it('shows the measured time for a stale plane', () => {
    render(<FreshnessBadge freshness={freshnessWith('stale')} />);
    expect(screen.getByTestId('freshness-report_received').textContent).toContain(
      String(new Date(1761840000000).getFullYear()),
    );
  });

  it('keeps the contractual plane order', () => {
    render(<FreshnessBadge freshness={freshnessWith('measured')} />);
    const planes = Array.from(screen.getByTestId('freshness-badge').querySelectorAll('[data-testid^="freshness-"]')).map((node) => node.getAttribute('data-testid'));
    expect(planes).toEqual(['freshness-report_received', 'freshness-authoritative', 'freshness-page_read']);
  });

  it('uses the 403 Result status for revoked access', () => {
    const { container } = render(<ObservationForbidden />);
    expect(container.querySelector('.ant-result-403')).not.toBeNull();
  });

  it('prints the confirmation timestamp for a partial failure', () => {
    render(<PartialFailure errorCode="ReadTimeout" confirmedAtUnixMs={1761840000000}><span>facts</span></PartialFailure>);
    expect(screen.getByTestId('state-partial').textContent).toContain(
      String(new Date(1761840000000).getFullYear()),
    );
  });
});
