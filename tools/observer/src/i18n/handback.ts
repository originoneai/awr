/**
 * Handback vocabulary: the one copyable line per pending kind, and the words that say
 * which Agent to say it in (AWR-PPO-012). Pure data, no UI imports; the sentence builder
 * in src/lib/agent-handback interpolates {{placeholders}} itself so it stays deterministic.
 *
 * Chinese sentences follow the prototype's "在你的 Agent 里处理" box verbatim
 * (prototype/src3/data.js, ASK items). Every key must exist in both languages with the
 * same placeholders; a parity test pins this.
 */

export const HANDBACK_KINDS = ['answer', 'decision', 'plan-approval', 'assignment', 'lesson'] as const;
export type HandbackKind = (typeof HANDBACK_KINDS)[number];

/** Answer and decision sentences have a variant that omits the question text. */
export type HandbackSentenceKey = HandbackKind | 'answerNoQuestion' | 'decisionNoQuestion';

/** Structured basis codes for the Agent resolution; wording lives only in this vocabulary. */
export const HANDBACK_AGENT_BASES = ['task-execution', 'single-connection', 'any-agent'] as const;
export type HandbackAgentBasis = (typeof HANDBACK_AGENT_BASES)[number];

export interface HandbackVocabulary {
  kind: Record<HandbackKind, string>;
  sentence: Record<HandbackSentenceKey, string>;
  where: { known: string; anyAgent: string };
  fallback: { project: string; task: string };
}

export const HANDBACK_VOCABULARY: Record<'zh-CN' | 'en-US', HandbackVocabulary> = {
  'zh-CN': {
    kind: {
      answer: '回答问题',
      decision: '做决定',
      'plan-approval': '批准拆解',
      assignment: '接下派工',
      lesson: '确认经验',
    },
    sentence: {
      answer: '回答{{project}}里“{{task}}”在等我的问题：{{question}}',
      answerNoQuestion: '回答{{project}}里“{{task}}”在等我的问题',
      decision: '回答{{project}}里“{{task}}”在等我的决定：{{question}}',
      decisionNoQuestion: '回答{{project}}里“{{task}}”在等我的决定',
      'plan-approval': '看一下{{project}}里“{{task}}”的拆解，我来决定批不批',
      assignment: '接下{{project}}派给我的任务：{{task}}',
      lesson: '确认我在{{project}}的候选经验：{{task}}',
    },
    where: {
      known: '在你的「{{agent}}」里说这一句',
      anyAgent: '在你的任意一个 Agent 里说一句',
    },
    fallback: {
      project: '当前项目',
      task: '这件事',
    },
  },
  'en-US': {
    kind: {
      answer: 'Answer a question',
      decision: 'Make a decision',
      'plan-approval': 'Approve a breakdown',
      assignment: 'Take an assignment',
      lesson: 'Confirm a lesson',
    },
    sentence: {
      answer: 'Answer the question “{{question}}” waiting for me on “{{task}}” in {{project}}',
      answerNoQuestion: 'Answer the question waiting for me on “{{task}}” in {{project}}',
      decision: 'Answer the decision “{{question}}” waiting for me on “{{task}}” in {{project}}',
      decisionNoQuestion: 'Answer the decision waiting for me on “{{task}}” in {{project}}',
      'plan-approval': 'Look at the breakdown of “{{task}}” in {{project}} and decide whether to approve it',
      assignment: 'Take the task “{{task}}” assigned to me in {{project}}',
      lesson: 'Confirm my candidate lesson in {{project}}: “{{task}}”',
    },
    where: {
      known: 'Say this line in your “{{agent}}”',
      anyAgent: 'Say a line in any one of your Agents',
    },
    fallback: {
      project: 'the project',
      task: 'this item',
    },
  },
};

export type HandbackLanguage = keyof typeof HANDBACK_VOCABULARY;

export function handbackVocabulary(language: string): HandbackVocabulary {
  return HANDBACK_VOCABULARY[language.toLowerCase().startsWith('zh') ? 'zh-CN' : 'en-US'];
}
