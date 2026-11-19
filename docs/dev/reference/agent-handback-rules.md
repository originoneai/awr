# Agent handback: which Agent (AWR-PPO-012)

A pending item — a question to answer, a decision to make, a breakdown to approve, an
assignment to take, a candidate lesson to confirm — is handled in the member's own
Agent. The web page shows a line to copy; this table says which Agent that line is for.

`resolveAgent` in `tools/observer/src/lib/agent-handback/resolve.ts` implements the
table, and a test checks this file against the implementation row by row. Every Agent
name comes from a rule below; the code names an Agent only through these rules.

| Rule | Condition | Resolution | Basis code |
| ---- | --------- | ---------- | ---------- |
| 1 | The task's execution records name an Agent, and that name is one of the member's Agent connections | That connection's Agent name | `task-execution` |
| 2 | Otherwise, the member has exactly one Agent connection | That single connection's Agent name | `single-connection` |
| 3 | Otherwise, the member has several Agent connections and the read leaves the Agent unknown | No name; the page shows the any-Agent phrase | `any-agent` |
| 4 | The member has no Agent connections at all | No name either; the same any-Agent phrase, never a fabricated name | `any-agent` |

Matching under rule 1 compares the trimmed names exactly. Case stays significant,
because Agent names are display names and a near-miss must stay unmatched. When the
recorded execution Agent is missing from the connections, rules 2 through 4 continue
from the connections alone.

The any-Agent phrase lives in the vocabulary (`tools/observer/src/i18n/handback.ts`,
`where.anyAgent`) and reads `在你的任意一个 Agent 里说一句` in Chinese; the English
line is `Say a line in any one of your Agents`.

## Sentence construction

The copyable line for each kind comes from the same vocabulary file, in both languages,
and the Chinese lines follow the prototype's inbox copy. Untrusted text (project name,
task name, question text, Agent display name) is cleaned by `sanitizeUntrustedText`
before it is quoted:

- Control characters become separators; invisible characters, bidi overrides and
  hidden marks (variation selectors other than the two presentation selectors, and
  the combining grapheme joiner) are deleted.
- Whitespace folds to single spaces.
- Identifier- and credential-shaped tokens are dropped: hashes, long hex, UUID and
  ULID, `sha256:` digests, work-item keys, snake_case fields, dotted machine paths
  of three or more segments, and two-segment dotted names whose first segment is a
  service namespace (`work`, `task`, `session`, `claim`, and the rest of the list in
  the source). Ordinary names such as `server.js`, `README.md` or `example.com`
  stay as written.
- Leading markdown and command markers (`#`, `>`, `-`, `*`, `+`, backtick, `/`, `!`)
  are stripped; every quoting, bracketing and brace character becomes a fullwidth
  look-alike, so code spans, links, tags and placeholder syntax all die.
- Truncation is by code point with an ellipsis. The whole line stays within 240
  characters, and only the quoted text is shortened — the sentence's own words are
  kept whole.

The raw input is cut to a few thousand code points before any pattern runs, and every
pattern repetition is bounded, so a long hostile name costs linear time.

Copying uses only `navigator.clipboard.writeText` in the browser; when the clipboard
is unavailable or refuses, the text is returned so the page can select it for the
person to copy by hand.
