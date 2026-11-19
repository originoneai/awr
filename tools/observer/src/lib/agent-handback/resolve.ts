import type { HandbackAgentBasis } from '../../i18n/handback';
import { HANDBACK_AGENT_BASES } from '../../i18n/handback';
import type { HandbackAgentResolution, HandbackPerson, HandbackTaskFacts } from './types';

/**
 * Which of the member's Agents a pending thing sits on. The rules are the table in
 * docs/dev/reference/agent-handback-rules.md; a test checks that file line by line
 * against this implementation. An Agent name is never invented: when no rule names
 * one, the answer is null with the any-Agent basis.
 */

function normalized(name: string | null | undefined): string | null {
  if (typeof name !== 'string') return null;
  const trimmed = name.trim();
  return trimmed === '' ? null : trimmed;
}

/** Distinct connection names, in listed order, keeping each name exactly as registered. */
function connectionNames(person: HandbackPerson): string[] {
  const names: string[] = [];
  for (const connection of person.agentConnections ?? []) {
    const name = normalized(connection?.name);
    if (name !== null && !names.includes(name)) names.push(name);
  }
  return names;
}

export function isHandbackAgentBasis(value: string): value is HandbackAgentBasis {
  return (HANDBACK_AGENT_BASES as readonly string[]).includes(value);
}

/**
 * Rule 1: the task's execution Agent is one of the member's connections — that one.
 * Rule 2: otherwise a single connection — that one.
 * Rule 3: several connections, none derivable — no name (the any-Agent phrase).
 * Rule 4: no connections at all — no name either; a name is never fabricated.
 * Matching is exact after trimming whitespace; case is not folded, because Agent names
 * are display names and near-misses must not silently bind.
 */
export function resolveAgent(person: HandbackPerson, task: HandbackTaskFacts | null): HandbackAgentResolution {
  const names = connectionNames(person);
  const executionAgent = normalized(task?.executionAgentName);
  if (executionAgent !== null) {
    const recorded = names.find((name) => name === executionAgent);
    if (recorded !== undefined) return { name: recorded, basis: 'task-execution' };
  }
  if (names.length === 1) return { name: names[0], basis: 'single-connection' };
  return { name: null, basis: 'any-agent' };
}
