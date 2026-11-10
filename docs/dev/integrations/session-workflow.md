# Host-agnostic session workflow

Start or resume a work-bound session, read its required context and save progress before handoff. This guide connects CLI, MCP and native conversation identities.

## Use it

This page covers the **AWR 0.6.0** baseline and development source. Packages include CLI, stdio MCP, session/claim tools and [shared HTTP](../reference/mcp-service.md). Version 0.5.1 introduced grouped discovery; 0.6.0 adds schema 14 and `session start --client/--conversation`.

Installation examples use the canonical 0.7.0 package names. Existing users should
follow the [package migration steps](../release/DISTRIBUTIONS.md#migrate-from-the-previous-package-names) first.

```sh
npm install -g @originoneai/awr@0.7.0
# or: python -m pip install originoneai-awr==0.7.0
```

0.5.1 projects use SQLite schema 7. 0.6.0 migrates **7 → 14** on the first
explicit database write, normally `awr source reindex`. Back up `.awr/state.db`
first. Older binaries refuse schema 14. See
[AWR 0.6.0](../release/0.6.0.md) and
[Upgrading from 0.5.x](../release/DISTRIBUTIONS.md#upgrading-from-05x-schema-7--14).
0.6.0 packages compile Team code but leave it off unless enabled; this L0 path
stays on personal CLI/MCP. Optional source tools are available separately from the CLI packages.

### Bind the project

Initialize from a reviewed source manifest, as in [the basic example](../../../examples/basic/README.md). Use absolute executable and project paths.

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

`--provider` and `--model` are recorded labels. Inspect an existing session and confirm ownership before using its ID. A different agent/provider/model continues through resume.

#### Recording incomplete progress and caller identity

Development source adds checkpoint caller declarations and [dual-source progress queries](../reference/daily-work.md#source-plans-and-recorded-progress).

Record a caller declaration with `session checkpoint --session SESSION --agent YOUR_AGENT ...`; MCP `awr_session_checkpoint` accepts optional `agent`. A mismatch returns resume guidance.

Omission records `actor.origin="undeclared"` and `actor.agent_id=null`. Historical unattributed receipts read as `origin="not_recorded"`.

`ContextIncomplete` permits checkpoints of actual failures, missing context and open loops. Use the actual last-consumed context hash.

Checkpoint writes accept `--expected-project-revision` and its compatible alias `--expected-revision`. Read the top-level `project_revision` from `session show` / `awr_session_get`; inspect intervening changes after a conflict.

### Generic MCP

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

Probe status through `awr_query` with the payload above. Existing flat names remain callable. Hosts discovering through `tools/list` route via the advertised domains.

Shared HTTP uses a service URL, bearer token and explicit `project` key per project call. See [MCP tools](../../../crates/awr-mcp/README.md).

## How it behaves

| Stage | Result |
| --- | --- |
| Bind and claim | Explicit work ownership and conversation identity |
| Assess and compile | Management requirements and consumed context |
| Checkpoint | Attributed progress and open loops |
| Complete and end | Durable completion receipt, then session close-out |

### Start or resume

New work, with a claim:

```sh
AWR_REV=$(awrj status | jq -er '.project_revision')
awrj session start --work "$AWR_WORK" --agent "$AWR_AGENT" \
  --provider generic --model "$AWR_MODEL" --claim --ttl-ms 3600000 \
  --expected-revision "$AWR_REV" > "$AWR_NOTES/start.json"
AWR_SESSION=$(jq -er '.session.id' "$AWR_NOTES/start.json")
```

A claim records runtime ownership.

A project can declare a default claim lifetime and renewal in `.awr/policy.toml`; with it `--ttl-ms` can be left out, and `--no-expiry` asks for a claim without an expiry. See [claim policy](../reference/claim-policy.md).

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

Resume creates a successor AWR session.

### Assess before compiling

After start or resume, read the management assessment before compiling context or changing source:

```sh
awrj work assess "$AWR_WORK"
```

Read the compact assessment with `work assess`. Intensity is `lightweight`, `continuous` or `undetermined`. Its `execution_admission`, `observation_basis`, required actions and gaps describe the current management obligations. See [management intensity](../reference/management.md).

`work prepare --response-view action` returns required context and management requirements together. Consume `next_action`, `admission_gaps` and required context. See [context continuity](context-continuity.md).

#### Specification coverage (current source)

Development source supplies read-only `awr spec check`, or `awr_spec_check` under `awr_work`. Registered refs report `covered`, `uncovered`, `needs_review` or `unknown`. See [specification coverage](../reference/specification-coverage.md).

### Inspect before retrying

After CLI timeout, MCP disconnect or claim expiry, inspect original records before starting another session or repeating a side-effecting command:

```sh
awrj session show "$AWR_SESSION"
awrj recovery inspect --session "$AWR_SESSION"
awrj session compaction inspect --session "$AWR_SESSION"
```

If an execution id is already known, inspect that attempt rather than guessing:

```sh
awrj execution inspect "$AWR_EXECUTION"
```

Read session, recovery and compact compaction state to decide whether to continue. `--include-observation` expands telemetry on its separate diagnostic path.

Use the CLI commands above for session and recovery diagnostics.
[Project Progress Overview](../../../tools/project-progress-overview/README.md) shows local project maps,
and [Observer](../../../tools/observer/README.md) provides the Team observation workspace.

### Compile context

```sh
awrj context bootstrap --session "$AWR_SESSION" --budget 1000 \
  > "$AWR_NOTES/bootstrap.json"
jq -e '.context.complete' "$AWR_NOTES/bootstrap.json"

awrj context compile --work "$AWR_WORK" --session "$AWR_SESSION" \
  --budget 5000 > "$AWR_NOTES/context.json"
jq -e '.completeness.complete and (.work_context != null)' "$AWR_NOTES/context.json"
```

Read the entire packet. On `BudgetExceeded`, inspect `required` and widen budget or scope. On `SourceStale`, inspect changes and reindex before reading again.

### Checkpoint and continue

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

Use returned/current revisions after saving. A completed save can record unfinished progress.

When the same AWR session is still active after host compaction, compile again.
Use `session resume` only for a real session handoff.

### Finish the work and end the session

`work complete` or `work cancel` releases the work’s claim. When the session’s active claims are exhausted, `close_out` gives one instruction under 60 tokens: checkpoint, then end with `--outcome ended`.

```sh
awrj work complete "$AWR_WORK" --session "$AWR_SESSION" --reason "Evidence reviewed" \
  --input "$AWR_NOTES/completion.json" --expected-revision "$AWR_REV" --end-session
```

`--end-session` (MCP `end_session` on `awr_work_transition` with `action=complete`) ends the
session in the same request, after the completion is durable. The receipt names both events
(`session_end.completion_event_id` and the `completion_event_id` of the `session.ended` event). It
is refused, and nothing is written, while the session holds an active claim on other work, or is not
active. If the completion succeeded but the end did not, the response reports `session_end.status`
`failed` with a nonzero exit; the completion stands, and `session end --outcome ended` finishes the
job. Without the flag the call behaves as before.

### Bind a native conversation

This section uses local client continuity. Its `--client generic` is a hook
dialect; it is distinct from the caller-supplied MCP namespace recorded by
`session start --client ... --conversation ...`. HTTP authenticates its registry
principal separately. For a host mixing CLI and MCP, create the
MCP-bound session first and attach here with its returned session ID. Follow the
[host contract's resolution rules](../reference/host-contract.md#resolve-an-external-session-in-its-own-namespace);
matching external strings alone do not connect the two bindings.

Prefix the host's conversation ID with the host name, so the durable identity
stays distinct per host without a new client enum or schema change. Fail fast
if the native ID is missing: an empty suffix yields the literal `cursor:`, which
is accepted as a valid identity and would merge unrelated host conversations
into one binding:

```sh
: "${HOST_CONVERSATION_ID:?set the native host conversation ID first}"
AWR_EXTERNAL="cursor:${HOST_CONVERSATION_ID}"
```

Get the native conversation ID from host metadata and keep it stable for that conversation. A new chat gets a new ID.

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

Acquire a work claim explicitly before mutation after one-step `client bind --from-session`.

Select either `--session` or `--from-session`. Use `--client generic` and the same namespaced external ID with `client progress` and `client show`.

An existing local binding stays pinned to its original AWR session after a
resume. Use a new native conversation when attaching continuity to a successor;
if the native conversation stays the same, use explicit successor session
operations. MCP's ability to reuse a conversation for a successor does not move
the old local continuity binding.

### End

When the work is not finished, end the session by itself. For finished work see
[Finish the work and end the session](#finish-the-work-and-end-the-session).

```sh
AWR_REV=$(awrj status | jq -er '.project_revision')
awrj session end --session "$AWR_SESSION" --outcome incomplete \
  --expected-revision "$AWR_REV"
```

Ending releases runtime claims.

### Fixture

[examples/codex/lifecycle.py](../../../examples/codex/README.md) runs the CLI path on a copy of `examples/basic`. Its historical directory names the L0 fixture. It reads `work assess` after claim and before bootstrap.

## Limits

- Use a current package; 0.4.0 predates this integration baseline.
- Starting `awr-mcp` binds an initialized project; database creation and migration use explicit operator commands.
- Session labels leave host launch to the client. Native chat IDs and AWR session IDs are separate identities.
- Development-source capabilities require matching installed versions; earlier packages retain their documented surfaces.
- Matching checkpoint declarations remain `trust=reported`; local CLI/MCP labels cannot authenticate the actual model. Existing MCP client access checks still apply.
- Omitted and historical attribution preserve their recorded origin rather than inheriting session labels. Historical events stay immutable.
- Checkpoint success records progress rather than verified context, passing tests or completed work; save receipts and metadata retain this distinction.
- The checkpoint guard uses project revision rather than `session.revision`. Source-backed next actions change through authoritative source edits or authorized writeback.
- Default discovery advertises domains; set `AWR_MCP_TOOL_EXPOSURE_MODE=flat` when a host requires flat discovery.
- AWR’s server operates independently of model API credentials.
- Remote/cloud clients require reachable HTTP with server-readable roots; stdio accesses files where its process runs.
- Claim operations preserve source work status.
- AWR resume leaves the host’s native chat under host control.
- Queue readiness and runtime claims describe their own layers; execution admission remains separately governed.
- This path uses the compact read; `--explain` expands details and `work manage` records host observations.
- Management intensity leaves permission, source-write and completion rules unchanged; `not_granted_by_management_classification` explicitly retains separate admission.
- Listed acceptance and goal links declare intent; criteria quality and implementation coverage need independent evidence.
- `host_assertion_not_independently_verified` keeps host observations unverified.
- Preparation supplies required reads; callers still consume them before proceeding.
- `spec check` postdates 0.6.0 packages. `covered` means a declared binding; tests, review quality and parent completion need separate evidence.
- Specification checks use registered refs; PRD scanning and automatic task creation are outside this path.
- Timeouts, disconnects and expired claims leave external execution outcome unknown.
- Inspection preserves sessions, observations, deferrals and processes. Missing occupancy and unverified executions remain unknown rather than zero or failure.
- CLI context reads may refresh projections; MCP `awr_context_compile` preserves the persistent index.
- Digest and context hash are caller assertions. Recovery uses completed checkpoint receipts; interrupted saves remain attempts.
- The close-out instruction appears only when its session/claim conditions apply.
- Direct source-status edits omit completion receipts and evidence bindings; the session and claim stay open until explicitly closed.
- Conversation identity comes from the host; invented or empty native IDs can merge unrelated continuity bindings.
- One-step bind-resume creates a successor with `claim: None`; session resume uses its own transfer/acquisition gates.
- Generic binding is L0; automatic installation is L2. Unsupported dialects return `Unsupported`, independently of binary availability or MCP connectivity.
- Codex installation is in the baseline; development source also provides the `claude-code` lifecycle dialect.
- Ending a session preserves the source work’s completion state.
- The L0 fixture invokes AWR rather than a coding agent; it omits `work manage` and `spec check`.
- Configuration, bindings and fixtures verify their own surfaces. Named-host tool calls, hook triggers and business acceptance require host/version-specific receipts.

## Code and tests

- [Lifecycle fixture](../../../examples/codex/README.md)
- [Checkpoint tests](../../../crates/awr-cli/tests/checkpoint_progress.rs)
- [Session tests](../../../crates/awr-cli/tests/session_cli.rs)
