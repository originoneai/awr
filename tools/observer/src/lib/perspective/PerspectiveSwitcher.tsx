import type { CSSProperties } from 'react';
import { useTranslation } from 'react-i18next';
import { SegmentedControl } from '../../components/display/SegmentedControl';
import { craftTokens } from '../../theme';
import { useCompactViewport, useMediaQuery } from '../viewport';
import { usePerspective } from './PerspectiveProvider';

interface PerspectiveSwitcherProps {
  /** Push right when the prototype hides its search field below 1240px. */
  alignEnd?: boolean;
}

export function PerspectiveSwitcher({ alignEnd = false }: PerspectiveSwitcherProps) {
  const { t } = useTranslation();
  const compactViewport = useCompactViewport();
  const hideCaption = useMediaQuery(`(max-width: ${craftTokens.perspective.captionMaxWidth}px)`);
  const { available, limitations, selected, select } = usePerspective();
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
    flex: compactViewport ? '1 1 0' : '0 0 auto',
    minWidth: 0,
    overflowX: compactViewport ? 'auto' : 'visible',
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
      <SegmentedControl
        className="shell-perspective-segment"
        optionClassName="shell-perspective-option"
        label={t('shell.perspective')}
        value={selected}
        options={available.map((id) => ({ value: id, label: t(`shell.perspectives.${id}.segment`) }))}
        onChange={select}
        tabBehavior="roving"
        style={segmentStyle}
      />
    </div>
  );
}
