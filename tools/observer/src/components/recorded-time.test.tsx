import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it } from 'vitest';
import i18n from '../i18n';
import { formatUnixMs } from '../lib/format';
import { RecordedTime } from './RecordedTime';

afterEach(async () => {
  await i18n.changeLanguage('zh-CN');
});

describe('RecordedTime', () => {
  it('shows a numeric time on the person\'s own clock and the exact value on hover', async () => {
    const { container } = render(<RecordedTime value={1761839800000} />);
    expect(container.textContent).toBe(formatUnixMs(1761839800000, 'zh-CN'));
    await userEvent.setup().hover(screen.getByText(formatUnixMs(1761839800000, 'zh-CN')));
    expect(await screen.findByText('1761839800000')).toBeTruthy();
  });

  it('reads a numeric string the same way', () => {
    const { container } = render(<RecordedTime value="1761839800000" />);
    expect(container.textContent).toBe(formatUnixMs(1761839800000, 'zh-CN'));
  });

  it('says not provided for anything that is not a time, never the current clock', () => {
    // an empty or blank string is not zero milliseconds (it used to read as 1970)
    for (const value of [null, undefined, '', '   ', 'soon', {}, [], true, Number.NaN]) {
      const { container, unmount } = render(<RecordedTime value={value} />);
      expect(container.textContent, JSON.stringify(value)).toBe('未提供');
      // not a styled value with a hover of its own: the plain words only
      expect(container.querySelector('.craft-value'), JSON.stringify(value)).toBeNull();
      unmount();
    }
  });

  it('words English the same way', async () => {
    await i18n.changeLanguage('en-US');
    const { container } = render(<RecordedTime value={1761839800000} />);
    expect(container.textContent).toBe(formatUnixMs(1761839800000, 'en-US'));
    const missing = render(<RecordedTime value={null} />);
    expect(missing.container.textContent).toBe('Not provided');
  });
});
