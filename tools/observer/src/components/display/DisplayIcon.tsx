import type { LucideIcon } from '../icons/prototype-icons';
import './display.css';

/** One icon box, size and stroke for the shared display components. */
export function DisplayIcon({ icon: Icon }: { icon: LucideIcon }) {
  return (
    <span className="obs-display-icon" aria-hidden="true">
      <Icon size="100%" />
    </span>
  );
}
