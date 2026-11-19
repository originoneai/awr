/**
 * Handback to the member's own Agent (AWR-PPO-012): the one copyable line per pending
 * kind, which of the member's Agents it sits on, and browser-only copying. Pure text
 * building — the platform itself performs no action and hosts no model; the line is for
 * a person to paste into their Agent.
 */

export { buildHandback, HANDBACK_MAX_SENTENCE_LENGTH, templateChunks } from './build';
export { copyText } from './copy';
export type { CopyTextResult } from './copy';
export { isHandbackAgentBasis, resolveAgent } from './resolve';
export { sanitizeUntrustedText, truncateCodePoints } from './sanitize';
export type { SanitizedText } from './sanitize';
export type {
  HandbackAgentConnection,
  HandbackContext,
  HandbackItem,
  HandbackPerson,
  HandbackResult,
  HandbackTaskFacts,
  HandbackAgentResolution,
} from './types';
export { HANDBACK_KINDS, HANDBACK_AGENT_BASES, handbackVocabulary } from '../../i18n/handback';
export type { HandbackKind, HandbackAgentBasis, HandbackSentenceKey, HandbackVocabulary } from '../../i18n/handback';
