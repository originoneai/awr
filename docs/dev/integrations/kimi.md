# Continuing AWR work with Kimi Code

> **Layer: L1 host note.** Use the [L0 session workflow](session-workflow.md)
> for the shared CLI/MCP path. This page records Kimi merge paths, native
> session flags, and a dated check. It does not add an L2 installer.
> See [host integration layers](README.md).

Use Kimi Code's terminal tool to call the AWR CLI. Optionally connect the same
project through AWR's stdio MCP server. AWR keeps work identity, claims and
checkpoints independently of Kimi's conversation history.

This guide targets **Kimi Code 0.41.0**, inspected on **2026-09-08**. Its local
help exposes `--continue` and `--session`, and links to the `kimi-code`
documentation. Older `kimi-cli` releases use different configuration paths and
flags; check `kimi --version` and `kimi --help` before applying an older recipe.

## Optional MCP connection

Build `awr` and `awr-mcp` with the [repository instructions](../../../README.md),
then initialize the target project from its reviewed source manifest. For an
already initialized project, keep its existing database and mapping.

Merge this server into `.kimi-code/mcp.json` in the target project, replacing
the absolute paths and preserving other entries:

```json
{
  "mcpServers": {
    "awr": {
      "command": "/absolute/path/to/awr-mcp",
      "args": ["--project", "/absolute/path/to/initialized/project"],
      "cwd": "/absolute/path/to/initialized/project"
    }
  }
}
```

Current Kimi documentation specifies project `.kimi-code/mcp.json` and user
`~/.kimi-code/mcp.json`. Project configuration requires workspace trust. Use
`/mcp-config` for configuration and `/mcp` to inspect connection status; servers
added by editing configuration join newly created sessions. This local release's
help does not expose the older `kimi mcp` subcommand or `--mcp-config-file` flag.
[Official MCP guide](https://www.kimi.com/code/docs/en/kimi-code-cli/customization/mcp.html)

Confirm the connected server's project identity using `awr_project_status`.
Its [work/context tools](../../../crates/awr-mcp/README.md) include `awr_context_compile`.
AWR 0.3.3 also provides MCP session start/checkpoint/resume and a
[shared HTTP service](../reference/mcp-service.md).
The CLI workflow below remains usable. A configuration file or a running Kimi
conversation does not establish that these tools are connected.

## Start or receive an AWR session

The following snippets use Bash and `jq`. Run one step at a time, inspect errors,
and replace the work/model/session placeholders with the actual receiving
environment. Provider/model fields record metadata; they do not launch Kimi.

```sh
AWR_BIN=/absolute/path/to/awr
AWR_PROJECT=/absolute/path/to/initialized/project
AWR_WORK=EXAMPLE-001
AWR_MODEL=your-current-kimi-model
AWR_NOTES=$(mktemp -d "${TMPDIR:-/tmp}/awr-kimi.XXXXXX")
awrj() { "$AWR_BIN" --project "$AWR_PROJECT" --json "$@"; }
awrj session list --active
awrj ready
```

For fresh work without an existing session, start and claim it:

```sh
AWR_REV=$(awrj status | jq -er '.project_revision')
awrj session start --work "$AWR_WORK" --agent kimi-primary \
  --provider moonshot --model "$AWR_MODEL" --claim --ttl-ms 3600000 \
  --expected-revision "$AWR_REV" > "$AWR_NOTES/start.json"
AWR_SESSION=$(jq -er '.session.id' "$AWR_NOTES/start.json")
```

For work handed over from Codex, Grok or another Kimi session, use this
**alternative** instead of creating a competing session:

```sh
AWR_PREDECESSOR=the-recorded-awr-session-id
awrj session show "$AWR_PREDECESSOR"
AWR_REV=$(awrj status | jq -er '.project_revision')
awrj session resume --from-session "$AWR_PREDECESSOR" \
  --agent kimi-primary --provider moonshot --model "$AWR_MODEL" \
  --budget 5000 --expected-revision "$AWR_REV" > "$AWR_NOTES/resume.json"
jq -e '.context_ready' "$AWR_NOTES/resume.json"
AWR_SESSION=$(jq -er '.resumed.session.id' "$AWR_NOTES/resume.json")
```

The work and work branch stay the same. A live predecessor claim transfers with
its existing expiration. If it has already expired or been released, explicitly
request a new claim with `--claim` and an appropriate TTL when ready; default
resume does not revive it. An AWR work branch does not switch a Git checkout.
Use the same project/runtime database for local continuation. Moving only the
source ledger to another computer does not copy session history.

After either route, read bootstrap and execution context:

```sh
awrj context bootstrap --session "$AWR_SESSION" --budget 1000 \
  > "$AWR_NOTES/bootstrap.json"
jq -e '.context.complete' "$AWR_NOTES/bootstrap.json"
awrj context compile --session "$AWR_SESSION" --budget 5000 \
  > "$AWR_NOTES/context.json"
jq -e '.completeness.complete and (.work_context != null)' "$AWR_NOTES/context.json"
jq -r '.work_context.rendered_context' "$AWR_NOTES/context.json"
```

Read the full delivered context and its gap/omission metadata. L0 is orientation;
L1 contains the work's execution facts. Resolve unknown rule scope with concrete
paths/tags. `BudgetExceeded` requires inspecting the required size and explicitly
adjusting budget or scope. Do not truncate hard facts. Known missing completion
evidence remains visible even when context completeness passes.

With MCP, pass `{"session":"<awr-session-id>","budget":5000}` to
`awr_context_compile`. If its read-only source check reports `SourceStale`,
inspect the source change and run `awr source reindex` before reading again.
Use the refreshed revision for every mutation and inspect durable receipts after
a lost response. For the shared command and error semantics, see the [L0 session workflow](session-workflow.md).

## Manual checkpoint and compact recovery

Before a planned compact or handoff, substitute actual progress and save the
hash of the context that was consumed:

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

Keep the root, work key, AWR session/checkpoint IDs, receipt directory, next
action and unresolved loops in the handoff. The digest/hash are caller-supplied;
checkpoint creation does not independently verify what the model read. Revisions
can advance more than once, so use returned/current values.

Kimi's own `kimi --continue`, `kimi --session <kimi-conversation-id>` and
`/compact` operate on its conversation. Their IDs are separate from AWR IDs.
After compact, compile context for the same active AWR session. For a real
execution-session handoff, use the explicit AWR resume route above.
[Official session guide](https://www.kimi.com/code/docs/en/kimi-code-cli/guides/sessions)

On nonzero resume output, inspect `resumed.session.id`: a successor can exist
with `context_ready: false`. Correct the inputs and compile for that successor
rather than repeating resume. `session show <predecessor>` also exposes an
existing successor after a lost response. Without a successful checkpoint,
recovery explicitly reports unavailable saved memory.

If stopping before a receiver takes over, close after checkpointing:

```sh
AWR_REV=$(awrj status | jq -er '.project_revision')
awrj session end --session "$AWR_SESSION" --outcome incomplete \
  --expected-revision "$AWR_REV"
```

This releases the claim; a later receiver needs a fresh claim. Source work
completion still requires AWR's evidence/acceptance checks.

## Hook and verification boundary

Current Kimi documentation includes `SessionStart`, `SessionEnd`, `PreCompact`
and `PostCompact` hooks. `PreCompact` is observational and its return value does
not block compaction. Hook support alone does not prove a checkpoint was saved.
[Official hook guide](https://www.kimi.com/code/docs/en/kimi-code-cli/customization/hooks.html)

Use the manual process when the client version lacks a needed event, the hook is
not installed/active, or its delivery is unverified. This guide does not install
an automatic adapter. Such an adapter would need actual session-ID mapping,
last-consumed context, meaningful handoff content and revision-conflict handling.

The local CLI version/options and guide configuration syntax were checked.
AWR's CLI continuation across provider/model metadata is exercised on a retained
fixture. This delivery does not claim a Kimi model turn, native Kimi MCP tool
invocation, automatic compact hook or E4 business scenario. The reusable
[CLI lifecycle example](../../../examples/codex/README.md) is a local demonstration;
its location under `codex/` does not make it a Kimi adapter.
