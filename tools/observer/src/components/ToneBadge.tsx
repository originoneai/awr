import { Badge } from 'antd';
import type { SemanticTone } from '../core/observation';

/** The antd status colour of a semantic tone: only the four semantic colours, nothing custom. */
export function badgeStatusOf(tone: SemanticTone): 'success' | 'error' | 'warning' | 'default' {
  if (tone === 'positive') return 'success';
  if (tone === 'negative') return 'error';
  if (tone === 'warning') return 'warning';
  return 'default';
}

/** A status word with its tone as a small dot. */
export function ToneBadge({ tone, text, testId }: { tone: SemanticTone; text: string; testId?: string }) {
  return (
    <span data-testid={testId}>
      <Badge status={badgeStatusOf(tone)} text={text} />
    </span>
  );
}
