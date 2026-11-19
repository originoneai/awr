# Agent handback: which Agent (AWR-PPO-012)

A pending item (a question to answer, a decision to make, a breakdown to approve, an
assignment to take, a candidate lesson to confirm) is handled in the member's own
Agent. The web page only shows a line to copy. This table says which Agent the line is
for; `resolveAgent` in `tools/observer/src/lib/agent-handback/resolve.ts` implements it,
and a test checks this file against the implementation row by row. An Agent name is
never invented.

| Rule | Condition | Resolution | Basis code |
| ---- | --------- | ---------- | ---------- |
| 1 | The task's execution records name an Agent, and that name is one of the member's Agent connections | That connection's Agent name | `task-execution` |
| 2 | Otherwise, the member has exactly one Agent connection | That single connection's Agent name | `single-connection` |
| 3 | Otherwise, the member has several Agent connections and none can be derived | No name; the page says the any-Agent phrase | `any-agent` |
| 4 | The member has no Agent connections at all | No name either; the same any-Agent phrase, never a fabricated name | `any-agent` |

Matching (rule 1) is exact after trimming whitespace. Case is not folded: Agent names
are display names, and a near-miss must not silently bind. When the recorded execution
Agent is not among the connections, rules 2–4 continue from the connections alone.

The any-Agent phrase lives in the vocabulary (`tools/observer/src/i18n/handback.ts`,
`where.anyAgent`) and must stay exactly `在你的任意一个 Agent 里说一句` in Chinese;
the English wording is `Say a line in any one of your Agents`.

## Sentence construction

The copyable line for each kind comes from the same vocabulary file, in both languages.
Untrusted text (project name, task name, question text, Agent display name) is cleaned
by `sanitizeUntrustedText` before it is quoted: whitespace is folded to single spaces,
control, invisible and bidi-override characters are removed, identifier- and
credential-shaped tokens are dropped, markdown structure (links, leading heading,
quote, list or command markers) is neutralized, quotes and backticks become fullwidth
look-alikes, and truncation is by code point with an ellipsis. The whole line is at
most 240 characters; only the quoted text is ever shortened, never the sentence's own
words. Copying uses only `navigator.clipboard.writeText` in the browser; when the
clipboard is unavailable or refuses, the text is returned so the page can select it.
