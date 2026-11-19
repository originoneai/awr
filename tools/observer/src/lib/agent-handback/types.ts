import type { HandbackAgentBasis, HandbackKind } from '../../i18n/handback';

/**
 * Minimal inputs for building a handback line. The read layer (a later task) provides
 * every field from already-read facts; this module performs no I/O and infers nothing
 * beyond the Agent-resolution rules in docs/dev/reference/agent-handback-rules.md.
 */

/** A pending thing addressed to the member: the five kinds the prototype's inbox lists. */
export interface HandbackItem {
  kind: HandbackKind;
  /** Project display name; free text from the records, treated as untrusted. */
  projectName: string | null;
  /** Task display name. For lesson items this is the lesson's short title. Untrusted. */
  taskName: string | null;
  /** The question or decision text. Untrusted. Null when the kind carries none. */
  question: string | null;
}

/** One Agent connection on the member's own account (their "Agent 连接" list). */
export interface HandbackAgentConnection {
  /** Agent display name, e.g. "Codex". Untrusted free text. */
  name: string;
  /** Connection kind (desktop client, CLI, API); shown by the page, not used here. */
  kind: string | null;
  /** When this connection last appeared; null is unknown. Never enters the sentence. */
  lastSeenUnixMs: number | null;
}

export interface HandbackPerson {
  agentConnections: readonly HandbackAgentConnection[];
}

/** What the task's execution records say about the Agent running it. */
export interface HandbackTaskFacts {
  /** Agent name recorded in the execution records; null when no record names one. */
  executionAgentName: string | null;
}

export interface HandbackContext {
  person: HandbackPerson;
  task: HandbackTaskFacts | null;
}

export interface HandbackAgentResolution {
  /** The Agent's name, or null when the rules cannot name one. Never invented. */
  name: string | null;
  /** Which rule produced the answer; see docs/dev/reference/agent-handback-rules.md. */
  basis: HandbackAgentBasis;
}

export interface HandbackResult {
  /** The one copyable line; at most 240 characters, counted by code point. */
  sentence: string;
  /** Which Agent this thing sits on, and why. */
  agent: HandbackAgentResolution;
  /** The page's "where" line naming the Agent, or the any-Agent phrase when unknown. */
  where: string;
  /** True when untrusted quoted text had to be shortened to keep the sentence readable. */
  truncated: boolean;
}
