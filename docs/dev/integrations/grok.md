# Continuing AWR work with Grok Build

> **Layer: L1 host note.** Use the [L0 session workflow](session-workflow.md)
> for the shared CLI/MCP path. This page records Grok merge paths, native
> continuation flags, and a dated check. It does not add an L2 installer.
> See [host integration layers](README.md).

Use the local **Grok Build** client's terminal tool to call AWR, or connect its
stdio MCP client to the same initialized project. This guide was checked against
`grok 1.0.13 (5e9a58528b76)` on **2026-09-08**. Grok's web connectors are a separate
transport boundary described below.

## Optional project MCP configuration

Build `awr` and `awr-mcp` using [the repository instructions](../../../README.md).
From the intended initialized project's directory:

```sh
cd /absolute/path/to/initialized/project
grok mcp add --scope project awr -- /absolute/path/to/awr-mcp \
  --project /absolute/path/to/initialized/project
grok mcp doctor awr --json
```

`add --scope project` writes or updates `.grok/config.toml`; use a distinct
server name if `awr` already refers to another project. User scope is the default
when `--scope project` is omitted. The equivalent table is:

```toml
[mcp_servers.awr]
command = "/absolute/path/to/awr-mcp"
args = ["--project", "/absolute/path/to/initialized/project"]
```

The server's absolute project argument determines the AWR runtime. `grok mcp
list --json` shows configuration; `grok mcp doctor awr --json` checks that named
server. The local CLI has no `grok mcp get` subcommand. Use `/mcps` in Grok Build
to inspect/refresh connections, then call `awr_project_status` and verify the
project identity. [Official MCP guide](https://docs.x.ai/build/features/mcp-servers)

Project trust is a separate prerequisite. During this delivery, the local Grok
CLI wrote the intended table in an isolated fixture and its named diagnostic
reported `folder untrusted`, leaving the server unstarted. No trust bypass was
used and no native Grok-to-AWR connection is claimed. Review the exact project in
the client's normal trust workflow before treating its MCP server as available.

## AWR start and cross-client resume

Use Bash and `jq` for these terminal commands. Run each step separately and
inspect the result. Replace the paths, work and model metadata with the actual
environment. AWR does not call Grok or configure its model/provider.

```sh
AWR_BIN=/absolute/path/to/awr
AWR_PROJECT=/absolute/path/to/initialized/project
AWR_WORK=EXAMPLE-001
AWR_MODEL=your-current-grok-model
AWR_NOTES=$(mktemp -d "${TMPDIR:-/tmp}/awr-grok.XXXXXX")
awrj() { "$AWR_BIN" --project "$AWR_PROJECT" --json "$@"; }
awrj session list --active
awrj ready
```

For new work without a session:

```sh
AWR_REV=$(awrj status | jq -er '.project_revision')
awrj session start --work "$AWR_WORK" --agent grok-primary \
  --provider xai --model "$AWR_MODEL" --claim --ttl-ms 3600000 \
  --expected-revision "$AWR_REV" > "$AWR_NOTES/start.json"
AWR_SESSION=$(jq -er '.session.id' "$AWR_NOTES/start.json")
```

For the same work handed over from Kimi, Codex or an earlier Grok session, use
this **alternative**. Supply the predecessor's AWR session ID from its handoff:

```sh
AWR_PREDECESSOR=the-recorded-awr-session-id
awrj session show "$AWR_PREDECESSOR"
AWR_REV=$(awrj status | jq -er '.project_revision')
awrj session resume --from-session "$AWR_PREDECESSOR" \
  --agent grok-primary --provider xai --model "$AWR_MODEL" \
  --budget 5000 --expected-revision "$AWR_REV" > "$AWR_NOTES/resume.json"
jq -e '.context_ready' "$AWR_NOTES/resume.json"
AWR_SESSION=$(jq -er '.resumed.session.id' "$AWR_NOTES/resume.json")
```

Resume retains the work and branch while refreshing current sources and applying
the receiving agent's rule scope. It inherits the checkpoint by reference.
A still-live claim keeps its original expiration. Use explicit `--claim` with
a TTL for a new claim when the old one expired or was released. Do not run a
second worker against a claim still owned by another active session.

Read current context after either route:

```sh
awrj context bootstrap --session "$AWR_SESSION" --budget 1000 \
  > "$AWR_NOTES/bootstrap.json"
jq -e '.context.complete' "$AWR_NOTES/bootstrap.json"
awrj context compile --session "$AWR_SESSION" --budget 5000 \
  > "$AWR_NOTES/context.json"
jq -e '.completeness.complete and (.work_context != null)' "$AWR_NOTES/context.json"
jq -r '.work_context.rendered_context' "$AWR_NOTES/context.json"
```

Read the context, including its hard rules, next action, loops and gaps. Bootstrap
alone is not execution context. On `BudgetExceeded`, inspect the required count
and explicitly choose a larger budget or clearer scope without truncating hard
facts. Pass concrete paths/tags when needed. The context's evidence gaps do not
become passing tests simply because its completeness flag is true.

With connected MCP, use `awr_context_compile` with
`{"session":"<awr-session-id>","budget":5000}`. Its source check is read-only;
on `SourceStale`, inspect the source change and explicitly reindex through the
CLI. See [the eight-tool reference](../../../crates/awr-mcp/README.md). Every
mutation uses an observed revision; a lost response requires receipt inspection
before retrying.

## Manual checkpoint and later continuation

Before a planned compact or handoff, save actual progress and the last context
hash used. The text below illustrates unfinished work; replace it with facts:

```sh
AWR_HASH=$(jq -er '.work_context.context_hash' "$AWR_NOTES/context.json")
AWR_REV=$(awrj status | jq -er '.project_revision')
awrj session checkpoint --session "$AWR_SESSION" --context-hash "$AWR_HASH" \
  --digest "Read the work context; implementation and review remain unfinished." \
  --next-action "Inspect the source-intake result and complete the remaining work." \
  --open-loop "Independent review and final delivery remain unfinished." \
  --expected-revision "$AWR_REV" > "$AWR_NOTES/checkpoint.json"
awrj session show "$AWR_SESSION"
```

Keep the absolute root, work key, AWR session/checkpoint IDs, receipt directory,
next action and loops in the handoff. Checkpoint digest/hash are caller
assertions. Use returned/current revisions, since one save advances revision
more than once. An interrupted save is not a recovery checkpoint.

Grok's native conversation continuation uses:

```sh
grok --cwd "$AWR_PROJECT" --continue
```

Or choose the actual Grok conversation ID with
`grok --cwd "$AWR_PROJECT" --resume <grok-conversation-id>`. Grok's
`--session-id` creates a new conversation; it is not an AWR ID or a resume flag.
Native conversation recovery and AWR work recovery are separate operations.
[Official session guide](https://docs.x.ai/build/features/sessions)

After native compaction, compile context for the same active AWR session. For
an actual AWR handoff, use `session resume` above. Check nonzero resume output:
`resumed.session.id` can identify a committed successor whose final context is
not ready. Fix its context and continue it instead of repeating resume.
Inspect `session show <predecessor>` after a lost response. Use the existing
runtime database; a copied ledger alone cannot restore its checkpoints/events.

When stopping before a receiver takes over, checkpoint and close the session:

```sh
AWR_REV=$(awrj status | jq -er '.project_revision')
awrj session end --session "$AWR_SESSION" --outcome incomplete \
  --expected-revision "$AWR_REV"
```

The claim is then released; later resume needs a fresh claim for mutation.
Ending a session does not mark the source work completed or certify its evidence.

When the work is finished instead, complete it with `awr work complete ... --end-session` (or end the
session as the `close_out` instruction of the response says); never complete work by editing its
status in the source, which records no receipt and releases no claim. See
[Finish the work and end the session](session-workflow.md#finish-the-work-and-end-the-session).

## Hooks, web connectors and verification

Grok Build documents session and compact events in its hook system. Project
hooks need trust, and passive event output does not control the main flow.
Preserve existing project/user hooks and verify an actual trigger/receipt before
claiming automatic AWR persistence. If events are missing, hooks are inactive,
or delivery is unverified, use the manual checkpoint process above.
[Official hook guide](https://docs.x.ai/build/features/hooks)

Grok web's custom connectors require a reachable MCP URL; a local executable path
cannot be entered as that URL. AWR 0.3.3 offers a
[shared Streamable HTTP service](../reference/mcp-service.md). Deploying it and
meeting a connector's authentication requirements remains
separate from configuring Grok Build; this guide does not verify that connector.
[Official web connector guide](https://docs.x.ai/grok/connectors)

Local command/version checks, project-scoped configuration parsing and the
documented untrusted-folder diagnostic are recorded separately from AWR's CLI
fixture, which exercises the same work across provider/model metadata. This
delivery claims no Grok model turn, successful native Grok MCP tool invocation,
automatic hook, web connector, E4 business scenario or release. For the shared
terminal lifecycle example, see [examples/codex](../../../examples/codex/README.md).
