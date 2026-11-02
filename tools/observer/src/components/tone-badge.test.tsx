import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import type { SemanticTone } from '../core/observation';
import { ClaimTag } from './ClaimTag';
import { ToneBadge, badgeStatusOf } from './ToneBadge';
import i18n from '../i18n';

describe('badgeStatusOf', () => {
  it('maps each semantic tone to one of the four antd status colours and nothing else', () => {
    const expected: Record<SemanticTone, string> = {
      neutral: 'default',
      accent: 'default',
      positive: 'success',
      warning: 'warning',
      negative: 'error',
    };
    for (const [tone, status] of Object.entries(expected)) expect(badgeStatusOf(tone as SemanticTone), tone).toBe(status);
  });
});

describe('ToneBadge', () => {
  it('shows the word with a dot of the tone\'s colour and keeps the test id on the whole', () => {
    render(<ToneBadge tone="negative" text="受阻" testId="word" />);
    expect(screen.getByTestId('word').textContent).toBe('受阻');
    expect(screen.getByTestId('word').querySelector('.ant-badge-status-dot')?.className).toContain('ant-badge-status-error');
  });
});

describe('ClaimTag', () => {
  it('colours only a claimed work and words a missing state as not provided, in both languages', async () => {
    const { rerender } = render(<ClaimTag value="claimed" />);
    expect(screen.getByText('已认领').closest('.ant-tag')?.className).toContain('ant-tag-processing');
    rerender(<ClaimTag value={null} />);
    expect(screen.getByText('未认领').closest('.ant-tag')?.className).not.toContain('ant-tag-processing');
    rerender(<ClaimTag value={undefined} />);
    expect(screen.getByText('未提供').closest('.ant-tag')?.className).not.toContain('ant-tag-processing');
    await i18n.changeLanguage('en-US');
    rerender(<ClaimTag value="claimed" />);
    expect(screen.getByText('Claimed')).toBeTruthy();
    await i18n.changeLanguage('zh-CN');
  });
});
