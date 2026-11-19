// @vitest-environment node
import { describe, expect, it } from 'vitest';
import { buildHandback, HANDBACK_MAX_SENTENCE_LENGTH, templateChunks } from './build';
import { handbackVocabulary } from '../../i18n/handback';
import { MALICIOUS_TASK_NAMES, textInvariants } from './handback-test-utils';
import type { HandbackContext, HandbackItem } from './types';

const person = (names: string[]): HandbackContext => ({
  person: { agentConnections: names.map((name) => ({ name, kind: null, lastSeenUnixMs: null })) },
  task: { executionAgentName: names[0] ?? null },
});

const item = (overrides: Partial<HandbackItem> = {}): HandbackItem => ({
  kind: 'answer',
  projectName: '订单协作平台',
  taskName: '空状态检查',
  question: '空状态要不要显示引导文案',
  ...overrides,
});

const length = (text: string): number => Array.from(text).length;

/** Every literal chunk of the template must survive, in order, however long the input was. */
function skeletonPresent(sentence: string, template: string): boolean {
  let from = 0;
  for (const chunk of templateChunks(template)) {
    const at = sentence.indexOf(chunk, from);
    if (at === -1) return false;
    from = at + chunk.length;
  }
  return true;
}

describe('buildHandback produces the prototype sentence for each kind', () => {
  it('Chinese sentences match the prototype inbox copy verbatim', () => {
    expect(buildHandback(item(), person(['Codex']), 'zh-CN').sentence)
      .toBe('回答订单协作平台里“空状态检查”在等我的问题：空状态要不要显示引导文案');
    expect(buildHandback(item({ kind: 'decision', taskName: '登录异常处理', question: '要不要返工' }), person(['Codex']), 'zh-CN').sentence)
      .toBe('回答订单协作平台里“登录异常处理”在等我的决定：要不要返工');
    expect(buildHandback(item({ kind: 'plan-approval', taskName: '完善异常恢复', question: '超时后自动重试几次' }), person(['Codex']), 'zh-CN').sentence)
      .toBe('看一下订单协作平台里“完善异常恢复”的拆解，我来决定批不批');
    expect(buildHandback(item({ kind: 'assignment', taskName: '导出按钮的权限判断', question: null }), person([]), 'zh-CN').sentence)
      .toBe('接下订单协作平台派给我的任务：导出按钮的权限判断');
    expect(buildHandback(item({ kind: 'lesson', taskName: '读多写少的查询接口先加缓存层', question: null }), person([]), 'zh-CN').sentence)
      .toBe('确认我在订单协作平台的候选经验：读多写少的查询接口先加缓存层');
  });

  it('builds all five kinds in both languages within the length cap', () => {
    const kinds = ['answer', 'decision', 'plan-approval', 'assignment', 'lesson'] as const;
    for (const language of ['zh-CN', 'en-US']) {
      for (const kind of kinds) {
        const result = buildHandback(item({ kind }), person(['Codex']), language);
        expect(textInvariants(result.sentence), `${language} ${kind}: ${result.sentence}`).toEqual([]);
        expect(result.sentence).not.toContain('undefined');
        expect(result.sentence).not.toContain('null');
        expect(result.sentence).not.toContain('NaN');
      }
    }
  });

  it('omits the question clause when there is no question text', () => {
    expect(buildHandback(item({ question: null }), person(['Codex']), 'zh-CN').sentence)
      .toBe('回答订单协作平台里“空状态检查”在等我的问题');
    expect(buildHandback(item({ kind: 'decision', question: '   ' }), person(['Codex']), 'zh-CN').sentence)
      .toBe('回答订单协作平台里“空状态检查”在等我的决定');
  });

  it('substitutes fallback words when project or task names are missing or unreadable', () => {
    const zh = buildHandback(item({ projectName: null, taskName: null }), person(['Codex']), 'zh-CN');
    expect(zh.sentence).toBe('回答当前项目里“这件事”在等我的问题：空状态要不要显示引导文案');
    const invisibleOnly = buildHandback(item({ taskName: '\u200B\u200E' }), person(['Codex']), 'en-US');
    expect(invisibleOnly.sentence).toContain('this item');
  });
});

describe('buildHandback truncation', () => {
  it('keeps the whole line within 240 characters whatever the input length', () => {
    for (const language of ['zh-CN', 'en-US']) {
      for (const fill of ['空状态检查', '👨‍👩‍👧🧑‍💻订单', 'Export button permission check']) {
        const result = buildHandback(
          item({ taskName: fill.repeat(120), question: fill.repeat(120) }),
          person(['Codex']),
          language,
        );
        expect(length(result.sentence)).toBeLessThanOrEqual(HANDBACK_MAX_SENTENCE_LENGTH);
        expect(result.truncated).toBe(true);
        expect(textInvariants(result.sentence, HANDBACK_MAX_SENTENCE_LENGTH)).toEqual([]);
      }
    }
  });

  it('shortens the quoted text and never the sentence\u2019s own words', () => {
    const template = handbackVocabulary('zh-CN').sentence.answer;
    const result = buildHandback(item({ taskName: '空状态检查'.repeat(80), question: '要不要加引导'.repeat(40) }), person(['Codex']), 'zh-CN');
    expect(result.truncated).toBe(true);
    expect(skeletonPresent(result.sentence, template)).toBe(true);
    expect(result.sentence.endsWith('…')).toBe(true);
    for (const chunk of templateChunks(template)) expect(result.sentence).toContain(chunk);
  });

  it('does not claim truncation for short input', () => {
    expect(buildHandback(item(), person(['Codex']), 'zh-CN').truncated).toBe(false);
  });

  it('a body-cutting implementation is caught: tail-slicing the sentence loses template words', () => {
    const template = handbackVocabulary('en-US').sentence.answer;
    const result = buildHandback(
      item({ taskName: 'task name '.repeat(40), question: 'question text '.repeat(40) }),
      person(['Codex']),
      'en-US',
    );
    // What a broken builder would do: build overlong text, then slice the whole line.
    const broken = `Answer the question “${'question text '.repeat(40)}” waiting for me on “${'task name '.repeat(40)}” in 订单协作平台`
      .slice(0, HANDBACK_MAX_SENTENCE_LENGTH);
    expect(length(broken)).toBe(HANDBACK_MAX_SENTENCE_LENGTH);
    expect(skeletonPresent(broken, template)).toBe(false);
    expect(skeletonPresent(result.sentence, template)).toBe(true);
  });
});

describe('buildHandback Agent resolution in the result', () => {
  it('names the Agent the execution records point to and cleans its name in the where line', () => {
    const result = buildHandback(item(), {
      person: { agentConnections: [{ name: 'Claude 桌面', kind: null, lastSeenUnixMs: null }, { name: 'Codex', kind: null, lastSeenUnixMs: null }] },
      task: { executionAgentName: 'Codex' },
    }, 'zh-CN');
    expect(result.agent).toEqual({ name: 'Codex', basis: 'task-execution' });
    expect(result.where).toBe('在你的「Codex」里说这一句');
  });

  it('uses the any-Agent phrase when several connections exist and none can be derived', () => {
    const result = buildHandback(item(), { person: { agentConnections: [{ name: 'Claude 桌面', kind: null, lastSeenUnixMs: 1 }, { name: 'Codex', kind: null, lastSeenUnixMs: 2 }] }, task: null }, 'zh-CN');
    expect(result.agent).toEqual({ name: null, basis: 'any-agent' });
    expect(result.where).toBe('在你的任意一个 Agent 里说一句');
    expect(buildHandback(item(), { person: { agentConnections: [] }, task: null }, 'en-US').where)
      .toBe('Say a line in any one of your Agents');
  });

  it('falls back to the any-Agent phrase when the Agent name scrubs to nothing', () => {
    const result = buildHandback(item(), person(['T-104']), 'zh-CN');
    expect(result.agent).toEqual({ name: null, basis: 'any-agent' });
    expect(result.where).toBe('在你的任意一个 Agent 里说一句');
  });
});

describe('buildHandback on malicious names', () => {
  for (const { input, keeps } of MALICIOUS_TASK_NAMES) {
    it(`keeps the line clean and readable for ${JSON.stringify(input.slice(0, 30))}`, () => {
      for (const language of ['zh-CN', 'en-US']) {
        const result = buildHandback(item({ taskName: input }), person(['Codex']), language);
        expect(textInvariants(result.sentence, HANDBACK_MAX_SENTENCE_LENGTH), `${language}: ${result.sentence}`).toEqual([]);
        expect(result.sentence).toContain(keeps);
      }
    });
  }
});
