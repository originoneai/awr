import { useState } from 'react';
import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { SegmentedControl, type SegmentedControlProps } from './SegmentedControl';

const options = [
  { value: 'first', label: 'First view' },
  { value: 'second', label: 'Second view' },
  { value: 'third', label: 'Third view' },
] as const;

function Example({ tabBehavior }: Pick<SegmentedControlProps<string>, 'tabBehavior'>) {
  const [value, setValue] = useState('first');
  return <>
    <button>Before</button>
    <SegmentedControl label="Views" value={value} options={options} onChange={setValue} tabBehavior={tabBehavior} />
    <button>After</button>
  </>;
}

describe('SegmentedControl', () => {
  it('exposes one named group of pressed buttons with native activation and no form submission', async () => {
    const user = userEvent.setup();
    const submit = vi.fn((event) => event.preventDefault());
    render(<form onSubmit={submit}><Example /></form>);
    const buttons = within(screen.getByRole('group', { name: 'Views' })).getAllByRole('button');
    expect(buttons.map((button) => button.getAttribute('aria-pressed'))).toEqual(['true', 'false', 'false']);
    expect(buttons.every((button) => button.getAttribute('type') === 'button')).toBe(true);
    await user.click(buttons[1]);
    expect(buttons[1].getAttribute('aria-pressed')).toBe('true');
    buttons[2].focus();
    await user.keyboard('{Enter}');
    expect(buttons[2].getAttribute('aria-pressed')).toBe('true');
    buttons[0].focus();
    await user.keyboard(' ');
    expect(buttons[0].getAttribute('aria-pressed')).toBe('true');
    expect(submit).not.toHaveBeenCalled();
  });

  it.each(['all', 'roving'] as const)('moves focus without changing selection with %s Tab behavior', async (tabBehavior) => {
    const user = userEvent.setup();
    const change = vi.fn();
    render(<SegmentedControl label="Views" value="second" options={options} onChange={change} tabBehavior={tabBehavior} />);
    const buttons = screen.getAllByRole('button');
    buttons[1].focus();
    for (const [key, index] of [
      ['{ArrowRight}', 2], ['{ArrowRight}', 0], ['{ArrowLeft}', 2],
      ['{Home}', 0], ['{End}', 2], ['{ArrowDown}', 0], ['{ArrowUp}', 2],
    ] as const) {
      await user.keyboard(key);
      expect(document.activeElement).toBe(buttons[index]);
      expect(buttons.map((button) => button.getAttribute('aria-pressed'))).toEqual(['false', 'true', 'false']);
    }
    expect(change).not.toHaveBeenCalled();
  });

  it.each(['all', 'roving'] as const)('preserves Tab entry, exit and reverse entry for %s', async (tabBehavior) => {
    const user = userEvent.setup();
    render(<Example tabBehavior={tabBehavior} />);
    screen.getByRole('button', { name: 'Before' }).focus();
    await user.tab();
    expect(document.activeElement).toBe(screen.getByRole('button', { name: 'First view' }));
    if (tabBehavior === 'all') {
      await user.tab();
      expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Second view' }));
      await user.tab();
      expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Third view' }));
    } else {
      await user.keyboard('{End}');
      expect(screen.getAllByRole('button').filter((button) => button.tabIndex === 0).map((button) => button.textContent))
        .toEqual(['Before', 'Third view', 'After']);
    }
    await user.tab();
    expect(document.activeElement).toBe(screen.getByRole('button', { name: 'After' }));
    await user.tab({ shift: true });
    expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Third view' }));
  });

  it('restores the roving entry after controlled selection and option availability change', () => {
    const change = vi.fn();
    const { rerender } = render(<SegmentedControl label="Views" value="first" options={options} onChange={change} tabBehavior="roving" />);
    rerender(<SegmentedControl label="Views" value="third" options={options} onChange={change} tabBehavior="roving" />);
    expect(screen.getAllByRole('button').map((button) => button.tabIndex)).toEqual([-1, -1, 0]);
    rerender(<SegmentedControl label="Views" value="second" options={options.slice(0, 2)} onChange={change} tabBehavior="roving" />);
    expect(screen.getAllByRole('button').map((button) => button.tabIndex)).toEqual([-1, 0]);
    expect(change).not.toHaveBeenCalled();
  });
});
