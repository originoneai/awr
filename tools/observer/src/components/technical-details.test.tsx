import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import i18n from '../i18n';
import { TechnicalDetails, type TechnicalDetailItem } from './TechnicalDetails';

const SESSION = '01M69PHCX2R0Y5M588J945JDEH';

let written: string[] = [];

function installClipboard(fail = false) {
  Object.defineProperty(navigator, 'clipboard', {
    configurable: true,
    value: {
      writeText: vi.fn(async (text: string) => {
        if (fail) throw new Error('denied');
        written.push(text);
      }),
    },
  });
}

// user-event installs its own clipboard when it is set up, so the stub goes in after it.
async function open(items: TechnicalDetailItem[], failCopy = false) {
  const user = userEvent.setup();
  installClipboard(failCopy);
  render(<TechnicalDetails items={items} />);
  await user.click(screen.getByRole('button', { name: /技术细节|Technical details/ }));
  return user;
}

beforeEach(() => {
  written = [];
});

afterEach(async () => {
  await i18n.changeLanguage('zh-CN');
});

describe('technical details', () => {
  it('keeps every raw value out of the page until it is opened, and out again when it is closed', async () => {
    const user = userEvent.setup();
    render(<TechnicalDetails items={[{ label: 'session_id', value: SESSION }]} />);
    expect(screen.queryByText(SESSION)).toBeNull();
    await user.click(screen.getByRole('button', { name: '技术细节' }));
    expect(await screen.findByText(SESSION)).toBeTruthy();
    expect(screen.getByText('session_id:')).toBeTruthy();
    await user.click(screen.getByRole('button', { name: '技术细节' }));
    await waitFor(() => expect(screen.queryByText(SESSION)).toBeNull());
  });

  it('copies the exact raw value and says it copied', async () => {
    const user = await open([{ label: 'session_id', value: SESSION }, { label: 'work_id', value: 'issue-filter-api' }]);
    await user.click(screen.getByRole('button', { name: '复制 session_id' }));
    expect(written).toEqual([SESSION]);
    expect(within(screen.getByTestId('technical-detail-session_id')).getByRole('button').textContent).toContain('已复制');
    // only the value that was copied says so
    expect(within(screen.getByTestId('technical-detail-work_id')).getByRole('button').textContent).toContain('复制');
    expect(within(screen.getByTestId('technical-detail-work_id')).getByRole('button').textContent).not.toContain('已复制');
  });

  it('copies a number, a flag and a structure as they are written', async () => {
    const user = await open([
      { label: 'version', value: 3 },
      { label: 'client_stale', value: false },
      { label: 'ceiling', value: ['work.read', 'claim.manage_own'] },
    ]);
    await user.click(screen.getByRole('button', { name: '复制 version' }));
    await user.click(screen.getByRole('button', { name: '复制 client_stale' }));
    await user.click(screen.getByRole('button', { name: '复制 ceiling' }));
    expect(written).toEqual(['3', 'false', '["work.read","claim.manage_own"]']);
  });

  it('says a missing value is not provided and offers nothing to copy for it', async () => {
    await open([{ label: 'client_id', value: null }, { label: 'path', value: '' }, { label: 'session_id', value: SESSION }]);
    expect(within(screen.getByTestId('technical-detail-client_id')).getByText('未提供')).toBeTruthy();
    expect(within(screen.getByTestId('technical-detail-path')).getByText('未提供')).toBeTruthy();
    expect(screen.queryByRole('button', { name: '复制 client_id' })).toBeNull();
    expect(screen.queryByRole('button', { name: '复制 path' })).toBeNull();
    expect(screen.getByRole('button', { name: '复制 session_id' })).toBeTruthy();
  });

  it('copies a value shown as an element through the copy text it declares, and offers no copy without one', async () => {
    const user = await open([
      { label: 'link', value: <a href="/x">open it</a>, copyValue: 'https://example.test/x' },
      { label: 'badge', value: <span>shown only</span> },
    ]);
    await user.click(screen.getByRole('button', { name: '复制 link' }));
    expect(written).toEqual(['https://example.test/x']);
    expect(screen.getByText('shown only')).toBeTruthy();
    expect(screen.queryByRole('button', { name: '复制 badge' })).toBeNull();
  });

  it('does not say it copied when the copy was refused', async () => {
    const user = await open([{ label: 'session_id', value: SESSION }], true);
    await user.click(screen.getByRole('button', { name: '复制 session_id' }));
    expect(written).toEqual([]);
    expect(within(screen.getByTestId('technical-detail-session_id')).getByRole('button').textContent).not.toContain('已复制');
  });

  it('says the same in English', async () => {
    await i18n.changeLanguage('en-US');
    const user = await open([{ label: 'session_id', value: SESSION }, { label: 'client_id', value: undefined }]);
    await user.click(screen.getByRole('button', { name: 'Copy session_id' }));
    expect(written).toEqual([SESSION]);
    expect(within(screen.getByTestId('technical-detail-session_id')).getByRole('button').textContent).toContain('Copied');
    expect(within(screen.getByTestId('technical-detail-client_id')).getByText('Not provided')).toBeTruthy();
  });

  it('renders nothing when there is nothing to show', () => {
    const { container } = render(<TechnicalDetails items={[]} />);
    expect(container.textContent).toBe('');
  });
});
