import { useEffect, useRef, useState, type CSSProperties, type KeyboardEvent } from 'react';
import { useTranslation } from 'react-i18next';
import { craftTokens } from '../../theme';
import { useCompactViewport, useMediaQuery } from '../viewport';
import { usePerspective } from './PerspectiveProvider';
import type { PerspectiveId } from './types';

interface PerspectiveSwitcherProps {
  /** Push right when the prototype hides its search field below 1240px. */
  alignEnd?: boolean;
}

export function PerspectiveSwitcher({ alignEnd = false }: PerspectiveSwitcherProps) {
  const { t } = useTranslation();
  const compactViewport = useCompactViewport();
  const hideCaption = useMediaQuery(`(max-width: ${craftTokens.perspective.captionMaxWidth}px)`);
  const { available, limitations, selected, select } = usePerspective();
  const [focused, setFocused] = useState<PerspectiveId>(selected);
  const buttonRefs = useRef<Array<HTMLButtonElement | null>>([]);

  useEffect(() => setFocused(selected), [selected]);

  const choose = (next: PerspectiveId) => select(next);

  const onKeyDown = (event: KeyboardEvent<HTMLButtonElement>, index: number) => {
    const id = available[index];
    if (event.key === 'Enter' || event.key === ' ' || event.key === 'Space' || event.key === 'Spacebar') {
      event.preventDefault();
      choose(id);
      return;
    }
    let nextIndex: number | null = null;
    if (event.key === 'ArrowRight' || event.key === 'ArrowDown') nextIndex = (index + 1) % available.length;
    if (event.key === 'ArrowLeft' || event.key === 'ArrowUp') nextIndex = (index - 1 + available.length) % available.length;
    if (event.key === 'Home') nextIndex = 0;
    if (event.key === 'End') nextIndex = available.length - 1;
    if (nextIndex === null) return;
    event.preventDefault();
    const next = available[nextIndex];
    setFocused(next);
    buttonRefs.current[nextIndex]?.focus();
  };

  const geometry = craftTokens.perspective;
  const rootStyle: CSSProperties = {
    display: 'flex',
    alignItems: 'center',
    flex: compactViewport ? '1 0 100%' : '0 0 auto',
    flexWrap: 'nowrap',
    gap: geometry.gapToCaption,
    minWidth: 0,
    marginLeft: compactViewport ? 0 : alignEnd ? 'auto' : 0,
  };
  const segmentStyle: CSSProperties = {
    display: 'inline-flex',
    flex: compactViewport ? '1 1 0' : '0 0 auto',
    gap: geometry.gap,
    minWidth: 0,
    padding: geometry.padding,
    overflowX: compactViewport ? 'auto' : 'visible',
    borderRadius: geometry.containerRadius,
    background: 'var(--craft-color-raised)',
    whiteSpace: 'nowrap',
  };
  const limitationTitle = limitations.map((limitation) => t(`shell.perspectiveLimitations.${limitation}`)).join(' · ');
  if (available.length === 1) return null;

  return (
    <div
      className="shell-perspective"
      data-testid="perspective-switcher"
      title={limitationTitle || undefined}
      style={rootStyle}
    >
      <span
        className="shell-perspective-caption"
        style={{
          display: hideCaption ? 'none' : undefined,
          flex: '0 0 auto',
          color: 'var(--craft-color-muted)',
          fontSize: geometry.captionFontSize,
          whiteSpace: 'nowrap',
        }}
      >
        {/* The inner span keeps the caption's text an inline box, like the prototype's
            .cap > .short, so the caption rides the body 20px line box without its own. */}
        <span>{t('shell.perspective')}</span>
      </span>
      <div
        className="shell-perspective-segment"
        role="group"
        aria-label={t('shell.perspective')}
        style={segmentStyle}
      >
        {available.map((id, index) => {
          const active = id === selected;
          const optionStyle: CSSProperties = {
            appearance: 'none',
            flex: '0 0 auto',
            height: geometry.optionHeight,
            margin: 0,
            padding: `0 ${geometry.optionPaddingInline}px`,
            border: 0,
            borderRadius: geometry.optionRadius,
            background: active ? 'var(--craft-color-surface)' : 'transparent',
            boxShadow: active ? 'var(--craft-perspective-selected-shadow)' : 'none',
            color: active ? 'var(--craft-color-ink)' : 'var(--craft-color-secondary)',
            cursor: 'pointer',
            fontFamily: 'inherit',
            fontSize: geometry.optionFontSize,
            fontWeight: active ? geometry.selectedFontWeight : geometry.fontWeight,
            // Prototype .seg button resets onto the body 20px line box.
            lineHeight: 'var(--craft-leading-body)',
            whiteSpace: 'nowrap',
            transition: `background-color ${craftTokens.motion.feedbackMs}ms ${craftTokens.motion.easing}, color ${craftTokens.motion.feedbackMs}ms ${craftTokens.motion.easing}`,
          };
          return (
            <button
              key={id}
              ref={(node) => { buttonRefs.current[index] = node; }}
              type="button"
              className="shell-perspective-option"
              aria-pressed={active}
              tabIndex={(focused === id || (!available.includes(focused) && active)) ? 0 : -1}
              onFocus={() => setFocused(id)}
              onClick={() => choose(id)}
              onKeyDown={(event) => onKeyDown(event, index)}
              style={optionStyle}
            >
              {t(`shell.perspectives.${id}.segment`)}
            </button>
          );
        })}
      </div>
    </div>
  );
}
