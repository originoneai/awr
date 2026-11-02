import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import './i18n';
import { TechnicalDetails } from './components/TechnicalDetails';
import { humanText } from './i18n/human-language';
import { defaultVisibleText, findForbiddenVisibleText } from './lib/human-language-guard';

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
});
