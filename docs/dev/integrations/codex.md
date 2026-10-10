# Using AWR from Codex

> **Layers: L1 host note + L2 optional adapter.** Session start, claim, compile,
> checkpoint and resume stay on the [L0 session workflow](session-workflow.md).
> This page records Codex merge paths, grouped MCP discovery, identity and the
> optional lifecycle adapter. See [host integration layers](README.md).

Checked against **Codex CLI 0.155.0** and the official
[MCP](https://learn.chatgpt.com/docs/extend/mcp) and
[hooks](https://learn.chatgpt.com/docs/hooks) documentation on **2026-09-20**.
The published package for this contract is **AWR 0.5.1**, not 0.4.0. This
source tree uses that same grouped default. Codex owns its model, UI, native
conversation IDs and compaction. AWR owns project facts, claims, checkpoints
and evidence.

This page is the personal stdio path. The remote Team service uses a different
server and tool family; see [Team MCP · Codex](team-mcp-codex-cli.md). The
WS-024 named adapter id is `codex_cli`; see [named agent host](named-agent-host.md).

## Why this is L1 plus L2

L0 is enough to run AWR from any terminal or MCP client. Codex still has merge
paths and lifecycle behavior that L0 cannot name:

- stdio configuration is merged into `.codex/config.toml` or
  `~/.codex/config.toml`; project configuration requires a trusted project
- the published AWR 0.5.1 catalog, and this source tree, default to grouped
  domain tools, so a flat child name is not the advertised top-level tool
- Codex has a documented lifecycle hook envelope and an optional AWR installer

L1 covers the MCP merge path and manual identity. L2 adds `awr client install`
hooks and native lifecycle checkpoints. Do not copy the L0 session tutorial into
this page. Do not add a second AWR runtime or client-private history access.

## MCP merge paths

Install published AWR 0.5.1, which includes the grouped MCP catalog, or build
both executables from this checkout:

```sh
npm install -g @originoneai/agent-work-runtime@0.5.1
# or: python -m pip install agent-work-runtime==0.5.1
command -v awr-mcp

# Contributor/source path. This tree uses the same grouped default as 0.5.1.
cargo build --locked -p awr-cli -p awr-mcp
# binaries: target/debug/awr and target/debug/awr-mcp
```

Use absolute executable and project paths. Initialize the target project with a
reviewed source manifest as described in [the basic example](../../../examples/basic/README.md).
Starting `awr-mcp` does not create or migrate a database.

Merge [the grouped stdio template](../../../examples/codex/config.toml.example)
into the trusted project's `.codex/config.toml`, or use the user-level
registration command:

```sh
codex mcp add awr -- /absolute/path/to/awr-mcp \
  --project /absolute/path/to/initialized/project
codex mcp get awr --json
```

Choose either project configuration or user-level registration for this server.
`codex mcp add` changes shared user configuration; the template changes nothing.
The local CLI, desktop and IDE clients share MCP configuration on the same host.
Give servers for different projects distinct names and explicit roots.

`codex mcp get` checks configured values. In the receiving client's `/mcp` view,
confirm a connected `awr` server, then request its project status and confirm the
project identity before work. Restart or reconnect the client after configuration
changes. A configured server is not proof of an active connection.

## Grouped MCP discovery

Published AWR 0.5.1 and this source tree default to eight top-level domain tools:

`awr_query`, `awr_context`, `awr_work`, `awr_evidence`, `awr_session`,
`awr_continuity`, `awr_change`, `awr_compaction`.

Call `awr_query` with no arguments to discover its child schemas, then pass a
child name and its arguments through the same domain. The status probe is:

```json
{"child_tool": "awr_project_status", "arguments": {}}
```

Flat names remain callable for existing integrations but are not in the default
catalog. A host that builds its tool list from `tools/list` must use the domain
route. Set `AWR_MCP_TOOL_EXPOSURE_MODE=flat` only when the receiving host cannot
route through domains.

Codex's `enabled_tools` is an allow list for top-level names. Do not populate it
with flat child names while AWR is in the default grouped mode: those names do
not match the advertised domain entries. The grouped template selects the
`awr_query`, `awr_context`, `awr_work` and `awr_evidence` domains. Add
`awr_session`, `awr_continuity`, `awr_change` or `awr_compaction` only when the
host needs those domains. A domain allow list permits every child inside that
domain; it does not preserve child-level filtering.

For an exact flat-tool allowlist, use
[the compatibility template](../../../examples/codex/config.flat.toml.example),
which sets `AWR_MCP_TOOL_EXPOSURE_MODE=flat` and selects the eight original flat
tools. Reconnect Codex after changing exposure mode so `tools/list` refreshes.
See [the MCP reference](../../../crates/awr-mcp/README.md) for argument contracts.
That reference still describes the older flat catalog in places; the domain list
above is the default `tools/list` for 0.5.1 and this source tree.

## Identity

`--client` is required on every `client` subcommand. Use the L0 generic identity
for a manual binding, prefixing the native Codex conversation UUID so unrelated
hosts cannot collide:

```sh
: "${CODEX_SESSION_ID:?set the Codex session UUID first}"
AWR_EXTERNAL="codex:${CODEX_SESSION_ID}"
awr --project /absolute/path/to/initialized/project client bind \
  --client generic --external-session "$AWR_EXTERNAL" \
  --work EXAMPLE-001 --session AWR_SESSION_ID
```

`AWR_SESSION_ID` comes from the [L0 session workflow](session-workflow.md).
`--session` attaches to an active AWR session. For a handoff, follow the L0
two-step path (`session resume`, then `bind --session`); `--from-session` creates
a successor and does not carry the claim. Never invent a Codex session ID or bind
the literal `codex:`.

`--client codex` is the documented native dialect for the L2 hook envelope, not
a prerequisite for manual MCP use. The native dialect receives the bare hook
`session_id`; the manual generic identity prefixes that same UUID with `codex:`.
`--provider openai` and `--model` on session start/resume are recorded labels;
they do not configure or invoke Codex.

## Optional L2 lifecycle adapter

Codex is the only host with a built-in `awr client install` adapter. Preview the
exact configuration, then accept it after review:

```sh
awr --project /absolute/path/to/initialized/project client install \
  --client codex --work EXAMPLE-001
awr --project /absolute/path/to/initialized/project client install \
  --client codex --work EXAMPLE-001 --accept
```

The installer merges `SessionStart`, `PreCompact`, `PostCompact`, `Stop`,
`SessionEnd` and `Interrupt` handlers into the project's `.codex/hooks.json`,
preserving existing handlers. Project hooks load only for a trusted project.
Review and trust the exact definitions in a fresh client's `/hooks` view.
Installation reports `activation_verified: false`; a generated file or synthetic
receiver call is not proof that the receiving client activated the hooks.

The receiver binds the native Codex `session_id` to a work-bound AWR session.
`SessionStart` and `PostCompact` return recovery context.
`Stop`, `PreCompact`, `SessionEnd` and `Interrupt` checkpoint the persisted next
action, open loops and observed AWR event/source delta. Shutdown hooks are
advisory: they do not end the AWR session, release claims or kill processes.
Explicit session end and handoff keep those responsibilities.

Record a changed next action before it is lost from the client:

```sh
awr --project /absolute/path/to/initialized/project client progress \
  --client codex --external-session "$CODEX_SESSION_ID" \
  --next-action "Apply the reviewer corrections" \
  --open-loop "Independent review remains"
awr --project /absolute/path/to/initialized/project client show \
  --client codex --external-session "$CODEX_SESSION_ID"
```

The adapter does not read transcript bodies, infer facts from assistant prose or
invoke a model. A checkpoint completed before a lost receipt is recovered by its
delivery key; incomplete saves are never recovery checkpoints. See
[client checkpoints](../TAKEOVER.md#client-checkpoints) for installation and
recovery details.

If the project root holds a `remote_workspace.toml`, the `SessionStart` receiver
also takes what other machines published before it renders context and names the
paths it moved. That step never fails the session, never publishes local work and
only re-registers index entries this host already published:
see [session-start exchange](../reference/workspace-exchange.md#会话开始时的自动取回).

Copying [the AGENTS snippet](../../../examples/codex/AGENTS.snippet.md) provides
agent instructions only; it does not install hooks. The
[executable lifecycle example](../../../examples/codex/README.md) checks AWR
behavior on a disposable project but does not invoke Codex or activate hooks.

## Dated check

On 2026-09-20, Codex CLI `0.155.0` and AWR `0.5.0` (commit `7f5301a`) were
checked under an isolated `CODEX_HOME`; no model turn was invoked. That check
predates the 0.5.1 package and is not evidence for the current source tree:

1. `codex mcp get awr --json` parsed both the grouped and flat templates.
2. The Codex app-server returned the four selected grouped domains through
   `mcpServerStatus/list`: `awr_query`, `awr_context`, `awr_work`, `awr_evidence`.
3. The flat compatibility template returned the eight selected flat tools.
4. `mcpServer/tool/call` routed `awr_query` with
   `{"child_tool":"awr_project_status","arguments":{}}` and reported project
   `AWR example`, `EXAMPLE-001` ready,
   `freshness_basis: source_verified_readonly`.
5. An independent stdio probe observed the full default catalog of eight domain
   tools and the same status result; flat mode exposed 32 callable flat tools.

That verifies Codex configuration parsing, live MCP startup and read routing
through the grouped and flat paths against AWR 0.5.0. AWR 0.5.1 publishes the
same grouped contract ([#66](https://github.com/originoneai/awr/pull/66),
[#68](https://github.com/originoneai/awr/pull/68)). It is not lifecycle-hook
activation, a model turn, Cloud execution or business acceptance, and it is not
a fresh Codex CLI check of this later source tree.
