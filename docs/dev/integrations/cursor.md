# Using AWR from Cursor

> **Layer: L1 host note.** Session start, claim, compile, checkpoint and resume
> stay on the [L0 session workflow](session-workflow.md). This page records only
> Cursor merge paths, identity, transport limits, and a dated check. It does not
> add an L2 installer. See [host integration layers](README.md).

Checked against **Cursor 3.18.9** and the [official MCP docs](https://cursor.com/docs/mcp)
on **2026-09-17**. Cursor owns the Agent Window, native conversation IDs and
compaction. AWR owns project facts, claims and checkpoints.

## Why this is L1

L0 is enough to run AWR from any terminal. Cursor still has merge-path and
transport mistakes that L0 cannot name:

- stdio config lives in `.cursor/mcp.json` or `~/.cursor/mcp.json`, not a host CLI
- Cursor's STDIO field table requires `"type": "stdio"`; `cwd` is not in that set
- Cloud Agents cannot reach a laptop `awr-mcp` or `127.0.0.1`
- default MCP `tools/list` is **grouped**; a host that only offers listed tools
  must call domain tools, not assume flat `awr_project_status`

Do not copy the L0 tutorial here. Do not add `--client cursor`. Do not run
`awr client install` for this host.

## MCP merge paths

This note describes the **AWR 0.6.0** integration baseline and the current source tree. Both
expose grouped `awr_query`. The **0.4.0** package does not. **0.5.1** already
grouped the catalog; install 0.6.0 for schema 14 and
`session start --client/--conversation`. Install 0.7.0, or build `awr` and
`awr-mcp` from this repository using
[the repository instructions](../../../README.md), then point `command` at that
binary (absolute path):

Installation examples use the canonical 0.7.0 package names. Existing users should
follow the [package migration steps](../release/DISTRIBUTIONS.md#migrate-from-the-previous-package-names) first.

```sh
npm install -g @originoneai/awr@0.7.0
# or: python -m pip install originoneai-awr==0.7.0
# or from this tree:
cargo build --locked -p awr-cli -p awr-mcp
# binaries: target/debug/awr and target/debug/awr-mcp
```

0.5.1 projects use SQLite schema 7. 0.6.0 migrates **7 → 14** on the first
explicit database write, normally `awr source reindex`. Back up `.awr/state.db`
first. Older binaries refuse schema 14. See
[AWR 0.6.0](../release/0.6.0.md) and
[Upgrading from 0.5.x](../release/DISTRIBUTIONS.md#upgrading-from-05x-schema-7--14).
0.6.0 packages compile Team code but leave it off unless enabled; this Cursor
note stays on personal CLI/MCP.

Initialize the project before starting the server. Merge
[the stdio template](../../../examples/cursor/mcp.json.example) into **one** of:

| Scope | Path |
| --- | --- |
| Project | `.cursor/mcp.json` |
| User | `~/.cursor/mcp.json` |

Prefer a single `awr` server. When both files exist, confirm in **Customize**
which one the Agent Window attached. Do not commit a project file that contains a
machine-local root or a token. `${workspaceFolder}` is the folder that contains
`.cursor/mcp.json`, not automatically the AWR `--project` root.

```json
{
  "mcpServers": {
    "awr": {
      "type": "stdio",
      "command": "/absolute/path/to/awr-mcp",
      "args": ["--project", "/absolute/path/to/initialized/project"]
    }
  }
}
```

AWR only needs absolute `command` plus `--project`. Reload **Customize** or the
window; a file on disk is not a live connection. Use **Output → MCP Logs** on
startup failure.

Then follow L0 MCP discovery: default `tools/list` returns eight domain tools
(`awr_query`, `awr_context`, `awr_work`, `awr_evidence`, `awr_session`,
`awr_continuity`, `awr_change`, `awr_compaction`). Probe status through
`awr_query`:

```json
{"child_tool": "awr_project_status", "arguments": {}}
```

Flat names remain callable but are not in the default catalog. Set
`AWR_MCP_TOOL_EXPOSURE_MODE=flat` only if this Cursor build cannot route through
domains. Confirm project identity before any mutation.

## Identity

Use L0 generic identity. Prefix the native conversation ID; fail if it is empty
so you never bind the literal `cursor:`:

```sh
: "${HOST_CONVERSATION_ID:?set the native host conversation ID first}"
AWR_EXTERNAL="cursor:${HOST_CONVERSATION_ID}"
```

0.6.0 records two distinct bindings. Matching external strings do not join them;
the shared AWR session ID is the explicit bridge. For Cursor stdio MCP, create
the MCP-bound session first (`--client stdio` is the local `awr-mcp` namespace,
not a hook dialect), then attach local continuity:

```sh
AWR_REV=$(awrj status | jq -er '.project_revision')
awrj session start --work "$AWR_WORK" --agent "$AWR_AGENT" \
  --provider generic --model "$AWR_MODEL" --claim --ttl-ms 3600000 \
  --client stdio --conversation "$AWR_EXTERNAL" \
  --expected-revision "$AWR_REV" > "$AWR_NOTES/start.json"
AWR_SESSION=$(jq -er '.session.id' "$AWR_NOTES/start.json")
awrj client bind --client generic --external-session "$AWR_EXTERNAL" \
  --work "$AWR_WORK" --session "$AWR_SESSION"
```

`--session` on `client bind` attaches to an **active** AWR session. For handoff,
take the L0 two-step path (`session resume`, then `bind --session`).
`bind --from-session` creates a successor, does not carry the claim, and requires
replacing `AWR_SESSION` from the receipt. See the
[host contract](../reference/host-contract.md#share-one-awr-session-between-local-continuity-and-mcp).

`--client` is required on every `client` subcommand. `--client cursor` is
`InvalidInput`. `awr client install --client cursor` is `Unsupported`. Do not
pass `--client cursor` on `session start` either: local stdio MCP uses
`--client stdio`; local continuity uses `--client generic`.

`--provider cursor` on `session start` is a display label only.

## HTTP

AWR shared HTTP uses a bearer token, not Cursor OAuth. HTTP tools need `project`.

Desktop smoke test on the same host as the service:

```json
{
  "mcpServers": {
    "awr": {
      "url": "http://127.0.0.1:8080/mcp",
      "headers": {
        "Authorization": "Bearer ${env:AWR_ENGINEERING_TOKEN}"
      }
    }
  }
}
```

Cloud Agents need a reachable HTTPS front door and server-side project roots.
This note does not verify Cloud Agent, team marketplace, or allowlist deployment.

## Hooks

Cursor documents `sessionStart`, `sessionEnd` and `preCompact` in
`.cursor/hooks.json`. [Official hook docs](https://cursor.com/docs/agent/hooks).
There is no L2 installer for that dialect. Checkpoint with L0 until a live hook
trigger and checkpoint receipt exist.

## Dated check

On 2026-09-19 a disposable copy of `examples/basic` was initialized with
`target/debug/awr` built from commit
`7ffcb8312a0309e42530cbcd889908dc7a3a473e`. The same tree's `awr-mcp` was
spoken to over stdio (initialize, then the default catalog):

1. `tools/list` returned the eight domain tools, including `awr_query`
2. `awr_query` with `{}` returned the child manifest; `awr_project_status` was present
3. `awr_query` with `{"child_tool":"awr_project_status","arguments":{}}` reported
   project `AWR example`, `EXAMPLE-001` ready,
   `freshness_basis: source_verified_readonly`

That is the grouped Cursor route against this source build. 0.6.0 still defaults
to the same eight domain tools and grouped `awr_query`, so the route still
applies. It is not a Cursor Agent Window recheck, a 0.6.0 live catalog replay,
Cloud Agent run, hook activation, model turn, or business acceptance.
