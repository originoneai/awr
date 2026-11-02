import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import './i18n';
import { TechnicalDetails } from './components/TechnicalDetails';
import { humanText } from './i18n/human-language';
import { defaultVisibleText, findForbiddenVisibleText } from './lib/human-language-guard';
import { DEMO_PROJECTS, renderApp, stubFetch } from './test-utils';

afterEach(() => vi.unstubAllGlobals());

describe('human-language visible-text guard', () => {
  it.each([
    ['work_snapshot', 'snake_case'],
    ['work.snapshot', 'operation_name'],
    ['a'.repeat(40), 'long_hex'],
    ['ActivityReadError', 'error_class'],
    ['合同', 'engineering_copy'],
    ['claim', 'engineering_word'],
    ['切片', 'engineering_copy'],
    ['未测量', 'engineering_copy'],
  ])('rejects %s as %s', (text, rule) => {
    expect(findForbiddenVisibleText(text).map((violation) => violation.id)).toContain(rule);
  });

  it('accepts business language and the human freshness vocabulary', () => {
    expect(findForbiddenVisibleText('任务正在推进，最近活动在 3 分钟前；内容未被改动')).toEqual([]);
    expect(humanText('notMeasured', 'zh-CN')).toBe('尚未读取');
    expect(humanText('notMeasured', 'en-US')).toBe('Not read yet');
  });

  it('keeps raw values out of default text even after technical details are expanded', async () => {
    const digest = 'a'.repeat(40);
    render(<TechnicalDetails items={[{ label: 'work_id', value: 'TW-201' }, { label: 'digest', value: digest }]} />);
    expect(document.body.textContent).not.toContain('TW-201');
    expect(document.body.textContent).not.toContain(digest);
    expect(defaultVisibleText(document.body)).not.toContain('work_id');

    const user = userEvent.setup();
    await user.click(screen.getByText('技术细节'));
    expect(document.body.textContent).toContain('TW-201');
    expect(document.body.textContent).toContain(digest);
    expect(defaultVisibleText(document.body)).not.toContain('TW-201');
    expect(defaultVisibleText(document.body)).not.toContain(digest);
    expect(findForbiddenVisibleText(defaultVisibleText(document.body))).toEqual([]);
  });

  it.each(['/knowledge', '/members', '/settings', '/advanced'])('uses human language on the %s placeholder', async (path) => {
    stubFetch({ '/api/team/projects': DEMO_PROJECTS });
    renderApp(path);
    expect(await screen.findByTestId('state-not-open')).toBeTruthy();
    expect(defaultVisibleText(document.body)).toContain('该功能尚未开放');
    expect(defaultVisibleText(document.body)).not.toContain('未观察到相关记录');
    expect(findForbiddenVisibleText(defaultVisibleText(document.body))).toEqual([]);
  });
});
