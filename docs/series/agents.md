# Connect your agent

Connect a supported agent client to the same initialized project, verify its identity and follow the shared session lifecycle. Each client keeps its own tools and native conversation.

## Use it

AWR supplies current project facts, context, claims and recoverable session records to Codex, Claude Code, Kimi, Cursor and Grok clients.

Every supported client follows the same pattern:

1. Build both executables from an AWR checkout:

```sh
cargo build --locked -p awr-cli -p awr-mcp
```

2. Initialize the target project from its reviewed source manifest (see
   [Quickstart](quickstart.md)), then register the `awr-mcp` server with the client, pointing
   `--project` at the project's absolute path. Each server binds to one canonical project root;
   give servers for different projects distinct names.
3. Verify the connection — a config file on disk is not a live connection — and confirm project
   identity with a status read before any work.
4. Work the shared session lifecycle: start or resume a session, read context, checkpoint before
   handoff, end when you stop. See [CLI](cli.md) and [MCP tools](mcp.md).

The sections below give each client's merge paths, configuration, and caveats. Where a client
documents automatic lifecycle hooks, treat them as unverified until you have seen a real trigger and
receipt; the manual checkpoint/resume flow always works.

## How it behaves

| Host | Entry point |
| --- | --- |
| Codex | MCP plus optional installed lifecycle hooks |
| Claude Code | Controlled observation adapter; source builds add lifecycle installation |
| Kimi, Cursor, Grok | CLI/MCP plus generic conversation binding and manual checkpoints |

### Codex

Codex loads project configuration only for trusted projects; its CLI, desktop, and IDE clients share
MCP configuration on the same host. Pick one of two options: merge the MCP configuration template
into the project's `.codex/config.toml` (replacing its two paths, preserving existing
configuration), or register at user level:

```sh
codex mcp add awr -- /absolute/path/to/awr-mcp \
  --project /absolute/path/to/initialized/project
codex mcp get awr --json
```

In `/mcp`, confirm the connected server and project identity. Read children include `awr_project_status`, `awr_work_ready`, `awr_work_get`, `awr_context_compile`, `awr_search`; mutation children include `awr_work_transition`, `awr_event_append`, `awr_evidence_record`.

Use `awr client install` to preview Codex lifecycle configuration before `--accept`. Existing hooks are preserved. Codex documents `SessionStart` (`startup|resume|clear|compact`), `PreCompact` (`manual|auto`), and advisory `SessionEnd` with a short timeout. Checkpoint manually until actual triggers deliver receipts.

### Claude Code

The controlled adapter `claude_code` reads status and collects result forensics. Start Claude Code yourself, then bind the native conversation.

Operator path:

1. Start Claude Code yourself.
2. Bind it to AWR with the generic client and the native conversation ID:

```sh
awrj client bind --client generic --external-session claude:<native-id> \
  --work "$AWR_WORK" --session "$AWR_SESSION"
```

3. When you need AWR observations, report phases through the shared CLI's external execution
   report.
4. On retry, reconnect the same execution identity before starting anything new.

### Kimi

This guide targets Kimi Code 0.41.0 (inspected 2026-09-08); older `kimi-cli` releases use different
configuration paths and flags, so check `kimi --version` and `kimi --help` first. Use Kimi's
terminal tool to call the AWR CLI, and optionally connect the same project over stdio MCP by merging
this server into the project's `.kimi-code/mcp.json` (user level: `~/.kimi-code/mcp.json`),
preserving other entries:

```json
{
  "mcpServers": {
    "awr": { "command": "/absolute/path/to/awr-mcp",
      "args": ["--project", "/absolute/path/to/initialized/project"],
      "cwd": "/absolute/path/to/initialized/project" }
  }
}
```

Project configuration requires workspace trust. Use `/mcp-config` for configuration and `/mcp` to
inspect connection status; servers added by editing configuration join newly created sessions.
Confirm project identity with `awr_project_status`. The provider label for Kimi session metadata is
`moonshot`.

Kimi’s `kimi --continue`, `kimi --session <kimi-conversation-id>` and `/compact` control its native conversation. After compaction, compile for the same active AWR session; resume AWR for a real handoff. Kimi documents `SessionStart`, `SessionEnd`, `PreCompact` and `PostCompact` hooks.

### Cursor

Checked against Cursor 3.18.9 (2026-09-17). Bind with `--client generic`. Merge the stdio template into project `.cursor/mcp.json` or user `~/.cursor/mcp.json`:

```json
{
  "mcpServers": {
    "awr": { "type": "stdio", "command": "/absolute/path/to/awr-mcp",
      "args": ["--project", "/absolute/path/to/initialized/project"] }
  }
}
```

Cursor requires `"type": "stdio"`, an absolute `command` and `--project`. Reload the window; inspect **Output → MCP Logs** on startup failure and **Customize** to identify the selected configuration when both files exist.

**Grouped tools.** In the current source tree, the default `tools/list` returns eight domain tools
(`awr_query`, `awr_context`, `awr_work`, `awr_evidence`, `awr_session`, `awr_continuity`,
`awr_change`, `awr_compaction`) rather than flat tool names. Probe status through `awr_query`:

```json
{"child_tool": "awr_project_status", "arguments": {}}
```

Flat children remain directly callable. Inspect the current build’s catalog; source builds advertise grouped domains. Use `AWR_MCP_TOOL_EXPOSURE_MODE=flat` for hosts requiring flat discovery.

Cloud Agents use a reachable HTTPS front door with server-side roots. Shared HTTP supplies bearer authentication:

```json
{
  "mcpServers": {
    "awr": { "url": "http://127.0.0.1:8080/mcp",
      "headers": { "Authorization": "Bearer ${env:AWR_ENGINEERING_TOKEN}" } }
  }
}
```

For identity, bind the generic client with the native conversation ID, failing if it is empty so you
never bind the literal `cursor:`:

```sh
: "${HOST_CONVERSATION_ID:?set the native host conversation ID first}"
AWR_EXTERNAL="cursor:${HOST_CONVERSATION_ID}"
awrj client bind --client generic --external-session "$AWR_EXTERNAL" \
  --work "$AWR_WORK" --session "$AWR_SESSION"
```

Cursor documents `sessionStart`, `sessionEnd` and `preCompact` in `.cursor/hooks.json`. Use manual checkpoints until a live trigger returns a checkpoint receipt.

### Grok

Checked against Grok Build 1.0.13 (2026-09-08). Use Grok's terminal tool to call the AWR CLI, or
connect its stdio MCP client from the project directory:

```sh
cd /absolute/path/to/initialized/project
grok mcp add --scope project awr -- /absolute/path/to/awr-mcp \
  --project /absolute/path/to/initialized/project
grok mcp doctor awr --json
```

`add --scope project` writes or updates `.grok/config.toml` (user scope is the default when the flag
is omitted); use a distinct server name if `awr` already refers to another project. The equivalent
table is:

```toml
[mcp_servers.awr]
command = "/absolute/path/to/awr-mcp"
args = ["--project", "/absolute/path/to/initialized/project"]
```

Use `/mcps` in Grok Build to inspect and refresh connections, then call `awr_project_status` and
verify the project identity. Project trust is a separate prerequisite: an untrusted folder leaves
the server unstarted, so review the project in the client's normal trust workflow first. The
provider label for Grok session metadata is `xai`.

Grok's native conversation continuation is separate from AWR session recovery:

```sh
grok --cwd "$AWR_PROJECT" --continue
grok --cwd "$AWR_PROJECT" --resume <grok-conversation-id>
```

Grok’s `--session-id` creates a native conversation. After compaction, compile for the active AWR session; use AWR resume for a handoff.

Grok Build documents session/compact events. Use manual checkpoints until a real trigger returns a receipt. Grok web connectors can use a reachable AWR shared HTTP endpoint.

### Next steps

- [MCP tools](mcp.md) — the full tool surface your client can call once connected.
- [Daily workflow](daily-workflow.md) — the start-work-checkpoint-resume loop, whichever client you use.

## Limits

- Agent clients execute work with their own tools. Provider/model labels are metadata and leave client launch and configuration to the host.
- Configuration becomes a verified connection after a live tool call and identity check. AWR itself requires no model API key.
- Hook installation preserves host trust decisions; approve trust through the host’s normal workflow.
- Development source also supports the `claude-code` lifecycle dialect; check installed capabilities before using an installer.
- The controlled adapter leaves Claude Code startup and shutdown to the operator; lifecycle hook installation is a separate capability.
- Kimi native IDs and AWR session IDs have separate scopes. Documented Kimi lifecycle hooks use the manual checkpoint path here.
- Cursor installer requests return `Unsupported`; binding with `--client cursor` returns `InvalidInput`. Use the generic client identity.
- Cursor stdio configuration excludes `cwd`; use absolute executable and project paths.
- Grouped discovery availability follows the installed version; the source route shown here requires a build exposing `awr_query`.
- Cloud agents require server-readable project files. Laptop executables and loopback endpoints stay local; bearer authentication is separate from Cursor OAuth.
- This integration supplies a generic Cursor binding; automatic installation for the Cursor hook dialect remains unsupported.
- Grok’s conversation IDs are distinct from AWR IDs, and `--session-id` creates a conversation rather than resuming one.
- Grok automatic lifecycle installation remains outside this integration.
- Web connectors require a reachable MCP URL and appropriate authentication. Deploying that endpoint and verifying connector authentication are separate operator steps.

## Code and tests

- [Client integration tests](../../crates/awr-cli/tests/client_continuity.rs)
- [MCP configuration tests](../../crates/awr-mcp/tests/codex_config.rs)
- [Claude Code lifecycle](../dev/integrations/claude-code.md)
