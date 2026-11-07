# Using AWR from WorkBuddy

> **Layer: L1 host note.** Session start, claim, compile, checkpoint and resume
> stay on the [L0 session workflow](session-workflow.md). This page records only
> WorkBuddy merge paths, the connector trust step, tool exposure mode, identity,
> and a dated check. It does not add an L2 installer. See
> [host integration layers](README.md).

Checked against **WorkBuddy 5.7.7** on **2026-10-12**. WorkBuddy owns the agent
window, the native conversation ID and compaction. AWR owns project facts, claims
and checkpoints.

## Why this is L1

L0 is enough to run AWR from WorkBuddy's terminal tool. WorkBuddy still has
merge-path and activation mistakes that L0 cannot name:

- the user-scope stdio config lives in `~/.workbuddy/mcp.json`; the format is
  plain `mcpServers`, and JSONC comments are accepted
- a saved server is **not** live until the user clicks **Trust** in the connector
  manager (连接器管理 → 自定义连接器 → 信任). Editing the file alone leaves the
  server unconnected
- the documented entry fields are `type`, `command`, `args`, `env`, `url`,
  `headers` and `description`. There is no `cwd` field, so every path must be
  absolute
- default MCP `tools/list` is **grouped**. A host that builds its tool catalog
  from `tools/list` must call a domain tool, not assume a flat
  `awr_project_status`

Do not copy the L0 tutorial here. Do not add `--client workbuddy`. Do not run
`awr client install` for this host.

## MCP merge path

This note describes the **packaged** `@originoneai/awr`. Install
the pinned release; source builds are out of scope for this host.

Initialize the project before starting the server, then merge one entry into
`~/.workbuddy/mcp.json`. Keep unrelated entries and add `awr`:

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

AWR only needs an absolute `command` plus `--project`. When the launcher is a
Node script, point `command` at the Node executable and pass the script as the
first argument, or make sure Node is on the PATH WorkBuddy inherited — WorkBuddy
does not add the installer's Node directory for you:

```json
{
  "mcpServers": {
    "awr": {
      "type": "stdio",
      "command": "/absolute/path/to/node",
      "args": [
        "/absolute/path/to/node_modules/@originoneai/awr/bin/awr-mcp.cjs",
        "--project",
        "/absolute/path/to/initialized/project"
      ]
    }
  }
}
```

Save, then **Trust** and enable the server in the connector manager. A file on
disk is not a live connection. Use the connector manager's status and the MCP
logs to diagnose a failed start.

Then follow L0 MCP discovery: default `tools/list` returns eight domain tools
(`awr_query`, `awr_context`, `awr_work`, `awr_evidence`, `awr_session`,
`awr_continuity`, `awr_change`, `awr_compaction`). Probe status through
`awr_query`:

```json
{"child_tool": "awr_project_status", "arguments": {}}
```

Flat names remain callable but are not in the default catalog. Set
`AWR_MCP_TOOL_EXPOSURE_MODE=flat` in the entry's `env` only if this WorkBuddy
build cannot route through domain tools. Confirm project identity before any
mutation.

## Identity

Use L0 generic identity. Prefix the native conversation ID; fail if it is empty
so you never bind the literal `workbuddy:`:

```sh
: "${HOST_CONVERSATION_ID:?set the native host conversation ID first}"
AWR_EXTERNAL="workbuddy:${HOST_CONVERSATION_ID}"
awrj client bind --client generic --external-session "$AWR_EXTERNAL" \
  --work "$AWR_WORK" --session "$AWR_SESSION"
```

`--work` is required on `client bind`, and the bound AWR session must already be
active on that work item (`session start --work … --claim`). `--client
workbuddy` is an unknown value and is rejected; `awr client install --client
workbuddy` returns `Unsupported`.

## Dated check

On **2026-10-12** the packaged `awr-mcp` **0.5.1** (Node **v24.21.0**) was
started over stdio against an initialized project on this host, using the
absolute-Node form above:

1. `initialize` returned `serverInfo {name: "awr-mcp", version: "0.5.1"}`
2. `tools/list` returned the eight domain tools, including `awr_query`
3. `tools/call` on `awr_query` with
   `{"child_tool": "awr_project_status", "arguments": {}}` reported project
   `agent-work-runtime`, `business_execution_ready: true`

The stdio client was a probe process, not the WorkBuddy agent window. This is a
transport and catalog check; it is not a WorkBuddy tool invocation, a trusted
connector activation, or business acceptance. Confirm the first real tool call
from inside WorkBuddy before relying on the binding.
