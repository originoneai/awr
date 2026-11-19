import type { LucideIcon } from 'lucide-react';
import './display.css';

/** One icon box, size and stroke for the shared display components. */
export function DisplayIcon({ icon: Icon }: { icon: LucideIcon }) {
  return (
    <span className="obs-display-icon" aria-hidden="true">
      <Icon size="100%" />
    </span>
  );
}
