// @vitest-environment node
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { describe, expect, it } from 'vitest';
import { HANDBACK_VOCABULARY } from '../../i18n/handback';
import { isHandbackAgentBasis, resolveAgent } from './resolve';
import type { HandbackAgentConnection, HandbackPerson, HandbackTaskFacts } from './types';

function connections(...names: string[]): HandbackPerson {
  const agentConnections: HandbackAgentConnection[] = names.map((name) => ({ name, kind: null, lastSeenUnixMs: null }));
  return { agentConnections };
}

const executedBy = (name: string | null): HandbackTaskFacts => ({ executionAgentName: name });

describe('resolveAgent', () => {
  it('rule 1: the execution Agent is one of the connections — that one', () => {
    const person = connections('Claude 桌面', 'Codex');
    expect(resolveAgent(person, executedBy('Codex'))).toEqual({ name: 'Codex', basis: 'task-execution' });
  });

  it('rule 1 does not fold case or match near-misses', () => {
    const person = connections('Claude 桌面', 'Codex');
    expect(resolveAgent(person, executedBy('codex')).basis).toBe('any-agent');
    expect(resolveAgent(person, executedBy('Codex 桌面')).basis).toBe('any-agent');
  });

  it('rule 1 matches after trimming whitespace', () => {
    expect(resolveAgent(connections('Codex'), executedBy('  Codex '))).toEqual({ name: 'Codex', basis: 'task-execution' });
  });

  it('rule 2: no execution fact, exactly one connection — that one', () => {
    expect(resolveAgent(connections('Codex'), null)).toEqual({ name: 'Codex', basis: 'single-connection' });
    expect(resolveAgent(connections('Codex'), executedBy(null))).toEqual({ name: 'Codex', basis: 'single-connection' });
  });

  it('rule 2 also applies when the recorded execution Agent is not a connection', () => {
    expect(resolveAgent(connections('Codex'), executedBy('别的会话'))).toEqual({ name: 'Codex', basis: 'single-connection' });
  });

  it('rule 3: several connections, none derivable — no name, any-Agent', () => {
    const person = connections('Claude 桌面', 'Codex');
    expect(resolveAgent(person, null)).toEqual({ name: null, basis: 'any-agent' });
    expect(resolveAgent(person, executedBy('不在名下'))).toEqual({ name: null, basis: 'any-agent' });
  });

  it('rule 4: no connections at all — still no name, never a fabricated one', () => {
    expect(resolveAgent({ agentConnections: [] }, null)).toEqual({ name: null, basis: 'any-agent' });
    expect(resolveAgent({ agentConnections: [] }, executedBy('Codex'))).toEqual({ name: null, basis: 'any-agent' });
  });

  it('deduplicates connections with the same name before counting', () => {
    const person = connections('Codex', 'Codex');
    expect(resolveAgent(person, null)).toEqual({ name: 'Codex', basis: 'single-connection' });
  });

  it('ignores blank connection names', () => {
    const person = connections('  ', '');
    expect(resolveAgent(person, null)).toEqual({ name: null, basis: 'any-agent' });
  });
});

describe('the rule table in docs/dev/reference/agent-handback-rules.md', () => {
  const docPath = path.resolve(process.cwd(), '../../docs/dev/reference/agent-handback-rules.md');
  const rows = readFileSync(docPath, 'utf8')
    .split('\n')
    .filter((line) => /^\| \d \|/.test(line))
    .map((line) => line.split('|').map((cell) => cell.trim().replace(/^`|`$/g, '')).slice(1, -1));

  it('is a four-row rule table whose codes are exactly the implemented bases', () => {
    expect(rows).toHaveLength(4);
    expect(rows.map((row) => row[0])).toEqual(['1', '2', '3', '4']);
    const codes = rows.map((row) => row[3] ?? '');
    for (const code of codes) expect(isHandbackAgentBasis(code), code).toBe(true);
    expect(new Set(codes)).toEqual(new Set(['task-execution', 'single-connection', 'any-agent']));
  });

  it('row 1 is the execution-Agent rule and the implementation agrees', () => {
    expect(rows[0][1]).toContain('execution records name an Agent');
    expect(rows[0][2]).toContain("That connection's Agent name");
    expect(resolveAgent(connections('Claude 桌面', 'Codex'), executedBy('Codex')).basis).toBe(rows[0][3]);
  });

  it('row 2 is the single-connection rule and the implementation agrees', () => {
    expect(rows[1][1]).toContain('exactly one Agent connection');
    expect(resolveAgent(connections('Codex'), null).basis).toBe(rows[1][3]);
  });

  it('row 3 is the several-connections rule and the implementation agrees', () => {
    expect(rows[2][1]).toContain('several Agent connections');
    expect(rows[2][2]).toContain('No name');
    expect(resolveAgent(connections('Claude 桌面', 'Codex'), null).basis).toBe(rows[2][3]);
  });

  it('row 4 is the no-connections rule and the implementation agrees, naming nobody', () => {
    expect(rows[3][1]).toContain('no Agent connections');
    expect(rows[3][2]).toContain('never a fabricated name');
    expect(resolveAgent({ agentConnections: [] }, null).name).toBeNull();
  });

  it('pins the exact any-Agent phrase from the acceptance wording in both languages', () => {
    expect(HANDBACK_VOCABULARY['zh-CN'].where.anyAgent).toBe('在你的任意一个 Agent 里说一句');
    expect(HANDBACK_VOCABULARY['en-US'].where.anyAgent).toBe('Say a line in any one of your Agents');
    expect(readFileSync(docPath, 'utf8')).toContain('在你的任意一个 Agent 里说一句');
  });
});
