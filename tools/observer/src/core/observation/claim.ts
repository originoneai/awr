import { humanText } from '../../i18n/human-language';
import { rawObservationText, statusWord } from './values';
import type { ClaimStateResult } from './types';

/**
 * Whether somebody has taken a work up. The service says 'claimed' or null for a work nobody holds;
 * a missing field, or anything else, is not a claim state and is never worded as one.
 */
export function claimState(value: unknown, options: { language: string }): ClaimStateResult {
  const { language } = options;
  if (value === 'claimed') {
    return { kind: 'claimed', text: statusWord('claimed', { language, field: 'state' }).text, technical: { raw: 'claimed' } };
  }
  if (value === null) {
    return { kind: 'unclaimed', text: statusWord('unclaimed', { language, field: 'state' }).text, technical: { raw: null } };
  }
  return { kind: 'unknown', text: humanText('notProvided', language), technical: { raw: rawObservationText(value).text } };
}
