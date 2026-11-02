import type { TFunction } from 'i18next';
import type { PlaneState } from './contract';

/** The only call site for translating a freshness plane state. */
export function freshnessWord(t: TFunction, status: PlaneState): string {
  return t(`freshness.state.${status}`);
}
