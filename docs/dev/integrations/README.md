# Host integration layers

AWR is a host-agnostic work runtime. It owns project facts, claims, checkpoints
and evidence. A coding agent owns its model, UI and native conversation. The
product boundary is the shared CLI/MCP contract, not any one editor or agent.

Use three layers. Do not treat a deeper layer as a new product identity.

| Layer | Promise | Form | When to add |
| --- | --- | --- | --- |
| **L0 — generic contract** | Any host that can run a CLI or speak MCP uses the same project state | [Session workflow](session-workflow.md), [MCP tools](../../../crates/awr-mcp/README.md), `awr client bind --client generic` | Default. This is the supported product. |
| **L1 — host note** | A named host was checked against L0 | Short page: merge paths, native IDs, dated verification, limits | After a real host check. Record diffs only. |
| **L2 — optional adapter** | Native lifecycle events can drive AWR checkpoints | `awr client install` and a documented hook dialect | Only when hooks are stable and someone maintains them. Today: Codex. |

L0 is complete enough to observe work, start/resume sessions and persist
checkpoints. L1 does not add runtime features. L2 does not make AWR that host's
runtime.

## What AWR owns

- Source-backed goals, work, acceptance and evidence
- Sessions, claims, checkpoints and operation receipts
- MCP tools and the CLI, including shared Streamable HTTP

## What the host owns

- Models, agent UI, native conversation IDs and compaction
- Whether MCP is stdio, HTTP or absent
- Whether native hooks exist. If they do not, operators checkpoint manually.

Provider and model strings stored by AWR are labels. They do not select, start
or configure a coding agent.

## Identity

`--client generic` is the L0 identity. Pair it with the host's conversation ID
(`--external-session`). `--session` attaches that conversation to an **active**
AWR session. `--from-session` **resumes** a predecessor and binds the new
conversation to the successor.

`--client` is **required** on every client subcommand. No dialect is a default,
so omitting it fails loudly with a usage error instead of silently assuming
one. `codex` and `kimi` are documented native dialects for hook field shapes.
They are not the centre of the product. Unknown `--client` values are rejected;
do not add a new enum for every editor.

Compatibility: hook commands written by `awr client install` already pin
`--client codex` explicitly, so installed adapters are unaffected. Operator
scripts that omitted `--client` must now name their dialect; existing bindings,
dedup keys and provider metadata are unchanged because explicit
`--client codex` resolves to the same identity as before.

Host names travel inside the external session ID, not the client enum:
`--client generic --external-session cursor:8f3a…`. The `<host>:` prefix keeps
identities distinct per host without a schema change. Binding records are
append-only and reject unknown fields, so a new `host` column would make new
events unreadable by older binaries; the namespaced ID avoids that. Use
`--provider` and `--model` on session start/resume for display labels.

`awr client install` is L2. Unsupported hosts are expected to stay on L0.

## Current host notes

| Host | Layer | Page | What it adds beyond L0 |
| --- | --- | --- | --- |
| Any CLI or MCP client | L0 | [Session workflow](session-workflow.md) | Shared commands and bind rules |
| Grok Build | L1 | [Grok note](grok.md) | Project MCP add, native `--continue` / `--resume`, dated check |
| Kimi Code | L1 | [Kimi note](kimi.md) | `.kimi-code/mcp.json`, native session flags, dated check |
| Codex | L1 + L2 | [Codex note and adapter](codex.md) | `.codex` grouped MCP merge, manual identity, `client install` hooks, AGENTS snippet |
| Named adapters (WS-024) | controlled | [Named agent host](named-agent-host.md) | Capability negotiation, L0 report, Codex + Claude Code, subtask parallelism |
| Claude Code (WS-024) | named | [Claude Code adapter](claude-code-agent.md) | Status/reconnect/forensics; not auto-startable |
| Team MCP · Codex (TMCP-041) | remote Team | [team-mcp-codex-cli.md](team-mcp-codex-cli.md) | HTTPS Team MCP natural workflow for `codex_cli` |
| Team MCP · Claude Code (TMCP-041) | remote Team | [team-mcp-claude-code.md](team-mcp-claude-code.md) | HTTPS Team MCP natural workflow for `claude_code` |
| Team Web entry (WS-044) | browser / Inspector | [team-web-entry.md](team-web-entry.md) | Cookie Web entry distinct from MCP bearer; Team collaboration loop |
| Cursor | L1 | [Cursor note](cursor.md) | `.cursor/mcp.json`, `type: stdio`, source-built grouped MCP, Cloud Agent HTTPS |
| Claude Code, Windsurf, … | L0 | [Session workflow](session-workflow.md) | None needed: `--client generic` with a `host:`-prefixed conversation ID |

Application hosts that invoke `awr` over argv follow the
[host application contract](../reference/host-contract.md) and
[host workflow](host-workflow.md). That is a program integration, not a
coding-agent L1 note.

## Verification

Configuration, a generated hooks file and a local CLI fixture are not native
activation, MCP tool invocation or business acceptance. An L1 note must say
what was actually called and on which host version. An L2 adapter must keep
`activation_verified: false` until a live trigger and checkpoint receipt exist.

## Adding another host

1. Run the [L0 session workflow](session-workflow.md) against a disposable
   project.
2. Connect MCP if the host has it; call `awr_project_status` and confirm the
   project identity before any mutation.
3. If that is enough for operators, stop. No new page is required.
4. If merge paths or native flags are easy to get wrong, add an **L1** page
   that links here and records only the diff plus a dated check.
5. Do not copy the session tutorial into the host page.
6. Do not add L2 unless native hooks have a stable envelope and a maintainer.

Cursor, Claude Code, Windsurf and similar hosts should enter at L0 or L1.
They do not need a Codex-shaped installer to “count” as supported.
Cursor MCP merge paths are an [L1 note](cursor.md).

- [Delegation ∩ execution authority (TMCP-030)](delegation-execution-auth.md)

## Team MCP authority notes

| Topic | Page |
| --- | --- |
| Delegation ∩ execution auth (TMCP-030) | [delegation-execution-auth.md](delegation-execution-auth.md) |
| PR delivery ∩ review/complete (TMCP-031) | [pr-delivery-review.md](pr-delivery-review.md) |
| Independent deploy + member handoff (TMCP-041) | [team-deploy-pack.md](../reference/team-deploy-pack.md) |
| Team MCP · Codex `codex_cli` (TMCP-041) | [team-mcp-codex-cli.md](team-mcp-codex-cli.md) |
| Team MCP · Claude Code `claude_code` (TMCP-041) | [team-mcp-claude-code.md](team-mcp-claude-code.md) |
| Team MCP BIZ acceptance (TMCP-051) | [../reference/team-mcp-biz-acceptance.md](../reference/team-mcp-biz-acceptance.md) |
