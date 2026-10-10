# Host-agnostic session workflow

This is the **L0** operator path. It works from any coding agent's terminal
tool, and from any MCP client that can start `awr-mcp` or reach the shared
HTTP service. Host-specific merge paths live in [L1/L2 notes](README.md).

Published AWR 0.5.1 packages include the CLI, stdio MCP, session/claim tools and
the [shared HTTP service](../reference/mcp-service.md). The 0.4.0 package is not
the current release.

## Bind the project

Initialize the target project from a reviewed source manifest, as in
[the basic example](../../../examples/basic/README.md). Starting `awr-mcp` does
not create or migrate a database. Use absolute executable and project paths.

```sh
AWR_BIN=/absolute/path/to/awr
AWR_PROJECT=/absolute/path/to/initialized/project
AWR_WORK=EXAMPLE-001
AWR_AGENT=agent-primary
AWR_MODEL=your-current-model
AWR_NOTES=$(mktemp -d "${TMPDIR:-/tmp}/awr-session.XXXXXX")
awrj() { "$AWR_BIN" --project "$AWR_PROJECT" --json "$@"; }

awrj session list --active
awrj ready
```

`--provider` and `--model` are recorded labels. They do not invoke a host.
If a session already exists for the work, inspect `session show` and confirm
ownership before using that AWR session ID. A different agent/provider/model
continues through `session resume` into its own session. Native chat IDs are not
AWR session IDs.

### Recording incomplete progress and caller identity

The current source adds checkpoint caller declarations and
[dual-source progress queries](../reference/daily-work.md#source-plans-and-recorded-progress).
This describes source capabilities, not a claim that older packages include them.

Use `session checkpoint --session SESSION --agent YOUR_AGENT ...` to record a
caller declaration independently from the session label. MCP
`awr_session_checkpoint` accepts the optional `agent` field. A declaration that
does not match the selected session is rejected with resume guidance. A matching
declaration remains **unverified**: local CLI and MCP cannot authenticate the real
model behind an agent label. MCP's existing client access checks still apply.

For compatibility, omitting the agent records `actor.origin="undeclared"` and
`actor.agent_id=null`. Historical receipts without attribution are read as
`origin="not_recorded"`. Neither case inherits authorship from the session's
agent/provider/model labels; historical events are not rewritten.

`ContextIncomplete` does not prohibit recording unfinished facts. Preserve actual
failures, missing context and open loops in a checkpoint, using the real last-used
context hash. Do not invent a hash or treat a saved checkpoint as verified context,
passing tests or completed work. The CLI and MCP surface that limitation when a
checkpoint is saved and when its metadata is read.

Checkpoint writes accept `--expected-project-revision`; `--expected-revision` is
a compatible alias. Use the top-level `project_revision` from `session show`
(MCP `awr_session_get`), **not** `session.revision`. On a conflict, inspect
intervening changes before retrying with the current project revision. Recording
a checkpoint never rewrites source-backed `next_action`; an intended contract
change belongs in the authoritative source or its authorized writeback path.

## Generic MCP

Any MCP client can launch a project-bound stdio server:

```json
{
  "mcpServers": {
    "awr": {
      "command": "/absolute/path/to/awr-mcp",
      "args": ["--project", "/absolute/path/to/initialized/project"]
    }
  }
}
```

Reload the client and confirm the project identity before work. The default
tool exposure is **grouped**: `tools/list` returns eight domain tools
(`awr_query`, `awr_context`, `awr_work`, `awr_evidence`, `awr_session`,
`awr_continuity`, `awr_change`, `awr_compaction`), not the flat names. Call a
domain with no arguments to discover its children, then invoke through it:

```json
{"child_tool": "awr_project_status", "arguments": {}}
```

So the status probe is an `awr_query` call with the payload above. Flat names
such as `awr_project_status` remain directly callable for already-integrated
hosts, but they are not in the default catalog; a host that builds its tool
list from `tools/list` must use the domain call. Set
`AWR_MCP_TOOL_EXPOSURE_MODE=flat` on the server only when the host cannot route
through domain tools.

The server does not need a model API key. Shared HTTP uses a URL and a bearer
token; every tool then requires an explicit `project` key. See
[MCP tools](../../../crates/awr-mcp/README.md).

Stdio cannot run on a laptop filesystem from a remote/cloud agent. Those hosts
need a reachable HTTP service whose registered roots exist on the server.

## Start or resume

New work, with a claim:

```sh
AWR_REV=$(awrj status | jq -er '.project_revision')
awrj session start --work "$AWR_WORK" --agent "$AWR_AGENT" \
  --provider generic --model "$AWR_MODEL" --claim --ttl-ms 3600000 \
  --expected-revision "$AWR_REV" > "$AWR_NOTES/start.json"
AWR_SESSION=$(jq -er '.session.id' "$AWR_NOTES/start.json")
```

The claim is runtime ownership. It does not rewrite source work status.

Handoff from a predecessor session:

```sh
AWR_PREDECESSOR=the-recorded-awr-session-id
awrj session show "$AWR_PREDECESSOR"
AWR_REV=$(awrj status | jq -er '.project_revision')
awrj session resume --from-session "$AWR_PREDECESSOR" \
  --agent "$AWR_AGENT" --provider generic --model "$AWR_MODEL" \
  --budget 5000 --expected-revision "$AWR_REV" > "$AWR_NOTES/resume.json"
jq -e '.context_ready' "$AWR_NOTES/resume.json"
AWR_SESSION=$(jq -er '.resumed.session.id' "$AWR_NOTES/resume.json")
```

Resume creates a new AWR session. It does not switch the host's native chat.

## Compile context

```sh
awrj context bootstrap --session "$AWR_SESSION" --budget 1000 \
  > "$AWR_NOTES/bootstrap.json"
jq -e '.context.complete' "$AWR_NOTES/bootstrap.json"

awrj context compile --work "$AWR_WORK" --session "$AWR_SESSION" \
  --budget 5000 > "$AWR_NOTES/context.json"
jq -e '.completeness.complete and (.work_context != null)' "$AWR_NOTES/context.json"
```

Read the packet, not only the boolean. On `BudgetExceeded`, inspect `required`
and widen the budget or scope. On `SourceStale`, inspect the source change and
run `awr source reindex` before reading again. MCP `awr_context_compile` is
read-only against the persistent index; CLI context reads may refresh it.

## Checkpoint and continue

```sh
AWR_CONTEXT_HASH=$(jq -er '.work_context.context_hash' "$AWR_NOTES/context.json")
AWR_REV=$(awrj status | jq -er '.project_revision')
awrj session checkpoint --session "$AWR_SESSION" --agent "$AWR_AGENT" \
  --context-hash "$AWR_CONTEXT_HASH" \
  --digest "Record the work actually done; do not invent a passing review." \
  --next-action "State the exact next operator or agent action." \
  --open-loop "List every unresolved loop." \
  --expected-project-revision "$AWR_REV" > "$AWR_NOTES/checkpoint.json"
```

Digest and hash are caller assertions. Use returned/current revisions after a
save. A completed checkpoint save may record unfinished work. An interrupted save
without a completion receipt is not a recovery checkpoint.

When the same AWR session is still active after host compaction, compile again.
Use `session resume` only for a real session handoff.

## Bind a native conversation

Prefix the host's conversation ID with the host name, so the durable identity
stays distinct per host without a new client enum or schema change. Fail fast
if the native ID is missing: an empty suffix yields the literal `cursor:`, which
is accepted as a valid identity and would merge unrelated host conversations
into one binding:

```sh
: "${HOST_CONVERSATION_ID:?set the native host conversation ID first}"
AWR_EXTERNAL="cursor:${HOST_CONVERSATION_ID}"
```

Get the native ID from the host itself (its session/conversation metadata, not
an invented value), and keep it stable for the life of that conversation. A new
chat means a new ID.

Attach to an **active** AWR session:

```sh
awrj client bind --client generic --external-session "$AWR_EXTERNAL" \
  --work "$AWR_WORK" --session "$AWR_SESSION"
```

For a handoff, prefer the two-step path: run `session resume` from the
[Start or resume](#start-or-resume) section first (it handles the successor
session, context and claim transfer), then bind the host conversation with
`--session "$AWR_SESSION"` as above.

A one-step alternative binds while resuming. It creates a **successor** session,
so the receipt's session ID must replace the variable — the predecessor is no
longer active and later commands against it return `NotFound`:

```sh
awrj client bind --client generic --external-session "$AWR_EXTERNAL" \
  --work "$AWR_WORK" --from-session "$AWR_PREDECESSOR" \
  > "$AWR_NOTES/bind.json"
AWR_SESSION=$(jq -er '.binding.session_id' "$AWR_NOTES/bind.json")
```

Unlike `session resume`, this path does **not** transfer or acquire the work
claim (`claim: None` on the successor). Acquire a claim explicitly before any
mutation.

Do not pass both flags. `--client generic` is the L0 identity; use the same
namespaced external ID on `client progress` and `client show`. `awr client
install` is L2 and currently exists only for Codex; other values return
`Unsupported`. That error is not a missing binary or a failed MCP connection.

## End

```sh
AWR_REV=$(awrj status | jq -er '.project_revision')
awrj session end --session "$AWR_SESSION" --outcome incomplete \
  --expected-revision "$AWR_REV"
```

Ending releases claims. It does not complete source work.

## Fixture

[examples/codex/lifecycle.py](../../../examples/codex/README.md) walks this CLI
path on a copy of `examples/basic`. It invokes AWR, not a coding agent. The
directory name is historical; treat the script as the L0 fixture.

## Verification boundary

A configured MCP server, a bind receipt or this fixture does not prove that a
named host called a tool, installed hooks or accepted business work. Record the
host version and the exact tools or commands that ran.
