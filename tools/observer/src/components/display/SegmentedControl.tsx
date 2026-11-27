import { useEffect, useRef, useState, type CSSProperties, type KeyboardEvent } from 'react';
import './SegmentedControl.css';

export interface SegmentedControlProps<Value extends string> {
  label: string;
  value: Value;
  options: readonly { value: Value; label: string }[];
  onChange: (value: Value) => void;
  /** Preserve the shell's single Tab stop; page filters keep each button tabbable. */
  tabBehavior?: 'all' | 'roving';
  className?: string;
  optionClassName?: string;
  style?: CSSProperties;
  testId?: string;
}

/** Read-only view selection: navigation keys move focus; activation changes the view. */
export function SegmentedControl<Value extends string>({
  label, value, options, onChange, tabBehavior = 'all', className = '',
  optionClassName = '', style, testId,
}: SegmentedControlProps<Value>) {
  const [focused, setFocused] = useState(value);
  const buttons = useRef<Array<HTMLButtonElement | null>>([]);
  useEffect(() => setFocused(value), [value]);
  const focusValue = options.some((option) => option.value === focused) ? focused : value;

  function moveFocus(event: KeyboardEvent<HTMLButtonElement>, index: number) {
    if (['Enter', ' ', 'Space', 'Spacebar'].includes(event.key)) {
      event.preventDefault();
      onChange(options[index].value);
      return;
    }
    let next: number;
    switch (event.key) {
      case 'ArrowRight': case 'ArrowDown': next = (index + 1) % options.length; break;
      case 'ArrowLeft': case 'ArrowUp': next = (index - 1 + options.length) % options.length; break;
      case 'Home': next = 0; break;
      case 'End': next = options.length - 1; break;
      default: return;
    }
    event.preventDefault();
    buttons.current[next]?.focus();
  }

  return (
    <div className={`obs-segmented-control ${className}`.trim()} role="group" aria-label={label} style={style} data-testid={testId}>
      {options.map((option, index) => (
        <button
          key={option.value}
          ref={(node) => { buttons.current[index] = node; }}
          type="button"
          className={`obs-segmented-option ${optionClassName}`.trim()}
          aria-pressed={option.value === value}
          tabIndex={tabBehavior === 'all' || option.value === focusValue ? 0 : -1}
          onFocus={() => setFocused(option.value)}
          onClick={() => onChange(option.value)}
          onKeyDown={(event) => moveFocus(event, index)}
        >
          {option.label}
        </button>
      ))}
    </div>
  );
}
