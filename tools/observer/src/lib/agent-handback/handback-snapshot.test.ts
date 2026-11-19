// @vitest-environment node
import { describe, expect, it } from 'vitest';
import { buildHandback } from './build';
import { resolveAgent } from './resolve';
import type { HandbackAgentConnection, HandbackContext, HandbackItem } from './types';

/**
 * Snapshot of the handback output for a fixed synthetic set: every kind in both
 * languages, plus one case where no Agent can be derived and one truncated case.
 * Nothing time-dependent enters the output, so any snapshot change is a wording change
 * that must be reviewed in the PR.
 */

const CONNECTIONS: HandbackAgentConnection[] = [
  { name: 'Claude 桌面', kind: 'desktop', lastSeenUnixMs: 1762982400000 },
  { name: 'Codex', kind: 'cli', lastSeenUnixMs: 1763068800000 },
];

const context = (executionAgentName: string | null, connections: HandbackAgentConnection[] = CONNECTIONS): HandbackContext => ({
  person: { agentConnections: connections },
  task: { executionAgentName },
});

const BASE: HandbackItem = {
  kind: 'answer',
  projectName: '订单协作平台',
  taskName: '空状态检查',
  question: '空状态要不要显示引导文案',
};

describe('handback snapshots', () => {
  describe('Chinese, Agent resolved from the task execution records', () => {
    for (const kind of ['answer', 'decision', 'plan-approval', 'assignment', 'lesson'] as const) {
      it(kind, () => {
        expect(buildHandback(
          { ...BASE, kind, taskName: kind === 'lesson' ? '读多写少的查询接口先加缓存层' : BASE.taskName, question: kind === 'answer' || kind === 'decision' ? BASE.question : null },
          context('Codex'),
          'zh-CN',
        )).toMatchSnapshot();
      });
    }
  });

  describe('English, Agent resolved from the task execution records', () => {
    for (const kind of ['answer', 'decision', 'plan-approval', 'assignment', 'lesson'] as const) {
      it(kind, () => {
        expect(buildHandback(
          { ...BASE, kind, taskName: kind === 'lesson' ? '读多写少的查询接口先加缓存层' : BASE.taskName, question: kind === 'answer' || kind === 'decision' ? BASE.question : null },
          context('Codex'),
          'en-US',
        )).toMatchSnapshot();
      });
    }
  });

  it('no Agent can be derived: several connections, none executing the task', () => {
    expect(buildHandback(BASE, context('不在名下'), 'zh-CN')).toMatchSnapshot();
    expect(buildHandback(BASE, context('不在名下'), 'en-US')).toMatchSnapshot();
    expect(resolveAgent(context('不在名下').person, context('不在名下').task)).toMatchSnapshot();
  });

  it('no Agent can be derived: no connections at all', () => {
    expect(buildHandback(BASE, context(null, []), 'zh-CN')).toMatchSnapshot();
  });

  it('truncated: overlong task name and question keep the sentence within 240 characters', () => {
    const result = buildHandback(
      { ...BASE, taskName: '空状态检查与引导文案设计'.repeat(30), question: '空状态要不要显示引导文案，加多少字合适'.repeat(20) },
      context('Codex'),
      'zh-CN',
    );
    expect(result.truncated).toBe(true);
    expect(Array.from(result.sentence).length).toBeLessThanOrEqual(240);
    expect(result).toMatchSnapshot();
  });
});
