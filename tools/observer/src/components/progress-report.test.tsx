import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it } from 'vitest';
import { describeProgressReport } from '../core/observation';
import i18n from '../i18n';
import { ProgressReport } from './ProgressReport';

afterEach(async () => {
  await i18n.changeLanguage('zh-CN');
});

function show(progress: unknown, language = 'zh-CN') {
  return render(<ProgressReport report={describeProgressReport(progress, { language })} />);
}

describe('ProgressReport', () => {
  it('says the report was not given when there is nothing to show', () => {
    show({ next_action: 'only this' });
    expect(screen.queryByTestId('progress-report')).toBeNull();
    expect(screen.getByTestId('state-not-reported')).toBeTruthy();
  });

  it('shows five lines of a long list and says how many more there are, in both languages', async () => {
    const completed = Array.from({ length: 8 }, (_, index) => `Finished part ${index + 1}.`);
    const { unmount } = show({ completed });
    const list = screen.getByTestId('progress-report-completed');
    expect(within(list).getAllByText(/Finished part/)).toHaveLength(5);
    expect(list.textContent).toContain('还有 3 条');
    expect(list.textContent).not.toContain('Finished part 6.');
    unmount();
    await i18n.changeLanguage('en-US');
    show({ completed }, 'en-US');
    expect(screen.getByTestId('progress-report-completed').textContent).toContain('3 more');
    expect(screen.getByText('Done')).toBeTruthy();
  });

  it('lists blockers when there are some, says none were reported for an empty list, and says nothing when the list was not given', () => {
    const { unmount } = show({ summary: 'Working.', blockers: ['Waiting for the API.'] });
    expect(screen.getByTestId('progress-report-blockers').textContent).toContain('Waiting for the API.');
    unmount();
    const empty = show({ summary: 'Working.', blockers: [] });
    expect(screen.getByTestId('progress-report-blockers').textContent).toBe('未报告阻塞');
    empty.unmount();
    show({ summary: 'Working.' });
    expect(screen.queryByTestId('progress-report-blockers')).toBeNull();
  });

  it('shows each test with its outcome and the tone of that outcome, never its reference', () => {
    show({ tests: [{ name: 'unit', outcome: 'passed', reference: 'secret-ref-1' }, { name: 'e2e', outcome: 'failed', reference: 'secret-ref-2' }] });
    const tests = screen.getByTestId('progress-report-tests');
    expect(tests.textContent).toContain('unit');
    expect(tests.textContent).not.toContain('secret-ref');
    const dots = Array.from(tests.querySelectorAll('.ant-badge-status-dot')).map((dot) => dot.className);
    expect(dots[0]).toContain('ant-badge-status-success');
    expect(dots[1]).toContain('ant-badge-status-error');
  });

  it('shows the task-changed warning, the declaration and the raw fields only behind the technical details', async () => {
    show({ summary: 'Done.', provenance: 'caller_declared', contract_matches_current: false, checkpoint_id: 'cp-9', origin: { actor_name: 'Lin Agent' } });
    expect(screen.getByTestId('progress-report-contract').textContent).toContain('任务要求有变化');
    expect(screen.getByTestId('progress-report-declaration').textContent).toBe('Lin Agent 汇报 · 内容由汇报者自述，系统未核验');
    expect(screen.queryByText(/cp-9/)).toBeNull();
    await userEvent.setup().click(screen.getByText('技术细节'));
    expect((await screen.findByTestId('technical-detail-progress.checkpoint_id')).textContent).toContain('cp-9');
  });
});
