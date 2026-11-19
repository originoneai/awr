import { handbackVocabulary } from '../../i18n/handback';
import type { HandbackKind, HandbackSentenceKey } from '../../i18n/handback';
import { sanitizeUntrustedText, truncateCodePoints } from './sanitize';
import { resolveAgent } from './resolve';
import type { HandbackContext, HandbackItem, HandbackResult } from './types';

/** The whole copyable line never exceeds this many characters, counted by code point. */
export const HANDBACK_MAX_SENTENCE_LENGTH = 240;

/** A displayed Agent name is short by nature; anything longer is cut, not shown in full. */
const AGENT_NAME_BUDGET = 40;

const PLACEHOLDER = /\{\{(project|task|question|agent)\}\}/g;

/** The literal chunks between placeholders; every chunk survives any truncation. */
export function templateChunks(template: string): string[] {
  return template.split(PLACEHOLDER).filter((part, index) => index % 2 === 0 && part !== '');
}

/**
 * One pass over the template: a single global replace visits each placeholder once and
 * never rescans inserted text, so untrusted values containing placeholder syntax stay
 * inert instead of expanding into the sentence.
 */
function interpolate(template: string, values: Partial<Record<'project' | 'task' | 'question' | 'agent', string>>): string {
  return template.replace(PLACEHOLDER, (_match, key: 'project' | 'task' | 'question' | 'agent') => values[key] ?? '');
}

function sentenceKey(kind: HandbackKind, hasQuestion: boolean): HandbackSentenceKey {
  if (!hasQuestion) {
    if (kind === 'answer') return 'answerNoQuestion';
    if (kind === 'decision') return 'decisionNoQuestion';
  }
  return kind;
}

interface Field {
  value: string;
  cut: boolean;
}

/** Shorten the quoted values, never the template words, until the whole line fits. */
function fitValues(template: string, requested: Array<{ text: string; cut: boolean }>): Field[] {
  const skeletonLength = templateChunks(template).reduce((sum, chunk) => sum + Array.from(chunk).length, 0);
  const budget = HANDBACK_MAX_SENTENCE_LENGTH - skeletonLength;
  const lengths = requested.map((field) => Array.from(field.text).length);
  const total = lengths.reduce((sum, length) => sum + length, 0);
  if (total <= budget) return requested.map((field) => ({ value: field.text, cut: field.cut }));

  // The largest equal cap at which the kept lengths still fit; every field keeps at least one character.
  const keptAt = (cap: number): number => lengths.reduce((sum, length) => sum + Math.min(length, cap), 0);
  let cap = Math.max(...lengths, 1);
  while (cap > 1 && keptAt(cap) > budget) cap -= 1;
  const kept = lengths.map((length) => Math.min(length, cap));
  // Hand the remaining characters back in field order, so nothing is wasted and the result is stable.
  let leftover = budget - keptAt(cap);
  for (let index = 0; index < kept.length && leftover > 0; index += 1) {
    const give = Math.min(lengths[index] - kept[index], leftover);
    kept[index] += give;
    leftover -= give;
  }
  return requested.map((field, index) => {
    if (lengths[index] <= kept[index]) return { value: field.text, cut: field.cut };
    const truncated = truncateCodePoints(field.text, kept[index]);
    return { value: truncated.text, cut: true };
  });
}

/**
 * Build the one copyable line for a pending thing, plus which Agent it sits on. The
 * untrusted project, task and question text is cleaned (sanitizeUntrustedText) and, when
 * too long, shortened — only the quoted text is cut, the sentence's own words never are.
 * Kinds without a question slot in the prototype's sentence (plan approval, assignment,
 * lesson) follow the prototype and do not append the question.
 */
export function buildHandback(item: HandbackItem, context: HandbackContext, language: string): HandbackResult {
  const vocabulary = handbackVocabulary(language);

  const project = sanitizeUntrustedText(item.projectName, HANDBACK_MAX_SENTENCE_LENGTH);
  const task = sanitizeUntrustedText(item.taskName, HANDBACK_MAX_SENTENCE_LENGTH);
  // Only the answer and decision sentences quote the question; the prototype's other
  // three sentences do not carry one, so the text is not appended for those kinds.
  const kindQuotesQuestion = item.kind === 'answer' || item.kind === 'decision';
  const question = kindQuotesQuestion
    ? sanitizeUntrustedText(item.question, HANDBACK_MAX_SENTENCE_LENGTH)
    : { text: '', truncated: false };
  const hasQuestion = question.text !== '';

  const template = vocabulary.sentence[sentenceKey(item.kind, hasQuestion)];
  const requested = [
    { text: project.text !== '' ? project.text : vocabulary.fallback.project, cut: project.truncated },
    { text: task.text !== '' ? task.text : vocabulary.fallback.task, cut: task.truncated },
    ...(hasQuestion ? [{ text: question.text, cut: question.truncated }] : []),
  ];
  const fields = fitValues(template, requested);

  const sentence = interpolate(template, {
    project: fields[0].value,
    task: fields[1].value,
    question: hasQuestion ? fields[2].value : undefined,
  });
  const truncated = fields.some((field) => field.cut);

  const resolution = resolveAgent(context.person, context.task);
  let where: string;
  let agent = resolution;
  if (resolution.name === null) {
    where = vocabulary.where.anyAgent;
  } else {
    const shown = sanitizeUntrustedText(resolution.name, AGENT_NAME_BUDGET);
    if (shown.text === '') {
      agent = { name: null, basis: 'any-agent' };
      where = vocabulary.where.anyAgent;
    } else {
      agent = { name: shown.text, basis: resolution.basis };
      where = interpolate(vocabulary.where.known, { agent: shown.text });
    }
  }

  return { sentence, agent, where, truncated };
}
