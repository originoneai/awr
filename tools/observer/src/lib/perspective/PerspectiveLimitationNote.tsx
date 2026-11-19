import { useTranslation } from 'react-i18next';
import { craftTokens } from '../../theme';
import { usePerspective } from './PerspectiveProvider';

/** A page-owned explanation to place under its title; never mounted in the shell header. */
export function PerspectiveLimitationNote() {
  const { t } = useTranslation();
  const { limitations } = usePerspective();
  if (limitations.length === 0) return null;
  return (
    <p
      role="note"
      data-testid="perspective-limitation-note"
      style={{
        margin: 0,
        color: 'var(--craft-color-muted)',
        fontSize: craftTokens.text.small,
        lineHeight: `${craftTokens.leading.body}px`,
      }}
    >
      {limitations.map((limitation) => t(`shell.perspectiveLimitations.${limitation}`)).join(' · ')}
    </p>
  );
}
