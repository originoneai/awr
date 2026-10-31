import { render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vitest';
import i18n from '../i18n';
import {
  NotReportedYet,
  ObservationEmpty,
  ObservationForbidden,
  PartialFailure,
} from './states';

afterEach(async () => {
  await i18n.changeLanguage('zh-CN');
});

describe('observation state presentations', () => {
  it('empty: tied to a read, never "no data ever"', () => {
    render(<ObservationEmpty readAtUnixMs={1761840000000} />);
    expect(screen.getAllByText(/未观察到相关记录/).length).toBeGreaterThan(0);
    expect(screen.getByTestId('state-empty').textContent).toContain(
      String(new Date(1761840000000).getFullYear()),
    );
    expect(screen.queryByText(/从未/)).toBeNull();
  });

  it('empty: bilingual copy', async () => {
    render(<ObservationEmpty />);
    expect(screen.getAllByText(/未观察到相关记录/).length).toBeGreaterThan(0);
    await i18n.changeLanguage('en-US');
    expect(screen.getAllByText(/no related records were observed/i).length).toBeGreaterThan(0);
  });

  it('forbidden: antd Result 403', async () => {
    render(<ObservationForbidden />);
    expect(screen.getByText('403')).toBeTruthy();
    expect(screen.getByText(/没有查看该范围的权限/)).toBeTruthy();
    await i18n.changeLanguage('en-US');
    expect(screen.getByText(/not authorized/i)).toBeTruthy();
  });

  it('not reported yet: distinct from empty and failed', async () => {
    render(<NotReportedYet />);
    expect(screen.getByText(/尚未收到上报/)).toBeTruthy();
    await i18n.changeLanguage('en-US');
    expect(screen.getByText(/not reported yet/i)).toBeTruthy();
  });

  it('partial failure keeps confirmed facts and declares the failed part', async () => {
    render(
      <PartialFailure errorCode="ReadTimeout" confirmedAtUnixMs={1761840000000}>
        <div>已确认的事实</div>
      </PartialFailure>,
    );
    // Confirmed facts stay on screen...
    expect(screen.getByText('已确认的事实')).toBeTruthy();
    // ...next to a warning that names the failed read.
    expect(screen.getByRole('alert')).toBeTruthy();
    expect(screen.getByText(/ReadTimeout/)).toBeTruthy();
    await i18n.changeLanguage('en-US');
    expect(screen.getByText(/partial read failure/i)).toBeTruthy();
  });
});
