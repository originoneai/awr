# Daily workflow

This guide walks through a repeatable daily routine with AWR: check the state
of your project, claim or resume a task, record progress, checkpoint, and hand
off so you — or an agent — can pick up exactly where things stopped.

Before you start, make sure your project is initialized and you know the basic
concepts (work items, sessions, claims, checkpoints) — see
[Quickstart](quickstart.md) and [Concepts](concepts.md). The same routine works
over MCP; see [MCP](mcp.md).

A note on versions: some capabilities below (the action view and `work edit`)
reflect the current source tree and may not be enabled in the released package.
Run `awr --help` to see what your installation supports.

## 1. Start your day: find the next action

Run a status check before doing anything else:

```sh
awr status
```

`awr status` and the MCP tool `awr_project_status` default to the action view
(`view="action"`). It separates your open work into four queues, with at most
five entries per queue plus exact total and omitted counts:

| Queue | Meaning | What you do |
| --- | --- | --- |
| `current` | Active or claimed work with no known wait or blocker | Continue with the owning session, or explicitly resume it |
| `ready` | Structure and claim readiness both pass | Prepare context and acquire ownership |
| `waiting` | A user wait, unresolved execution record, or unfinished dependency | Get the reply or inspect the prerequisite before retrying |
| `blocked` | Invalid structure, unavailable dependencies, source problems, or explicit blockers | Inspect the cited work and fix the cause |

Narrow the selection with repeatable selectors — they intersect:

```sh
awr status --work GUIDE-1 --goal GOAL-1 --milestone M1
```

The queue is navigation, not authorization: claims and completion checks stay
enforced by their own actions. `awr ready` keeps its narrower meaning —
eligible for a *new* claim — and omits in-progress work.

## 2. Claim or resume the task

Set up once per shell so every command is project-bound and JSON-formatted:

```sh
AWR_BIN=/absolute/path/to/awr
AWR_PROJECT=/absolute/path/to/initialized/project
AWR_WORK=EXAMPLE-001
AWR_AGENT=agent-primary
AWR_MODEL=your-current-model
AWR_NOTES=$(mktemp -d "${TMPDIR:-/tmp}/awr-session.XXXXXX")
awrj() { "$AWR_BIN" --project "$AWR_PROJECT" --json "$@"; }
```

New work, claimed under a new session:

```sh
AWR_REV=$(awrj status | jq -er '.project_revision')
awrj session start --work "$AWR_WORK" --agent "$AWR_AGENT" \
  --provider generic --model "$AWR_MODEL" --claim --ttl-ms 3600000 \
  --expected-revision "$AWR_REV" > "$AWR_NOTES/start.json"
AWR_SESSION=$(jq -er '.session.id' "$AWR_NOTES/start.json")
```

The claim is runtime ownership; it does not rewrite the source work status.

A project can declare a default claim lifetime and renewal in `.awr/policy.toml`; with it `--ttl-ms` can be left out, and `--no-expiry` asks for a claim without an expiry. See [claim policy](../dev/reference/claim-policy.md).
If a session already exists for the work, inspect it with `session show` and
confirm ownership — a different agent or model continues through
`session resume` into its own session.

## 3. Compile your context

Before editing, build the context packet for the session:

```sh
awrj context bootstrap --session "$AWR_SESSION" --budget 1000 \
  > "$AWR_NOTES/bootstrap.json"
jq -e '.context.complete' "$AWR_NOTES/bootstrap.json"

awrj context compile --work "$AWR_WORK" --session "$AWR_SESSION" \
  --budget 5000 > "$AWR_NOTES/context.json"
jq -e '.completeness.complete and (.work_context != null)' "$AWR_NOTES/context.json"
```

Read the packet, not just the boolean. On `BudgetExceeded`, widen the budget
or narrow the scope; on `SourceStale`, run `awr source reindex` first.

## 4. Work, then checkpoint

Record what really happened — failures, missing context, and open loops:

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

A few things to understand about checkpoints:

- `--agent` is a caller declaration. One that does not match the session is
  rejected with resume guidance; a matching one stays **unverified** — the CLI
  cannot authenticate the real model behind a label. Omitting it records the
  origin as `undeclared`.
- Digest and context hash are your assertions. Use the real last-used hash; a
  saved checkpoint does not prove verified context, passing tests, or
  completed work.
- For the revision guard, use the top-level `project_revision` from
  `session show`, not `session.revision`. On a conflict, inspect the
  intervening changes before retrying.
- A checkpoint never rewrites the source-backed next action. To change the
  contract, edit the authoritative source.

## 5. Fix small field edits without hand-editing YAML

When the source plan needs a small correction — say the source says "Draft the
guide" but the real next step is "Review the conclusion" — preview the edit:

```sh
awr --json work edit GUIDE-1 --request-key guide-next-1 --actor writer \
  --reason 'Clarify the review step' --next-action 'Review the conclusion'
```

The response shows the source location, old and new values, and the
fingerprints to accept with. Review them, then repeat the command adding:

```sh
--accept --source-fingerprint SOURCE_FINGERPRINT \
--expected-preview PREVIEW_FINGERPRINT --expected-revision REVISION
```

Supported fields are `--title`, `--summary`, `--priority`, and `--next-action`.
After an uncertain response, check `host status --key guide-next-1`; a field
edit never changes ownership, lifecycle, or verification.

## 6. End the session — or hand it off

When you stop for the day:

```sh
AWR_REV=$(awrj status | jq -er '.project_revision')
awrj session end --session "$AWR_SESSION" --outcome incomplete \
  --expected-revision "$AWR_REV"
```

Ending releases the claim; it does not complete the source work. Your
checkpoint from step 4 is what lets the work resume cleanly.

## 7. Pick up where you (or an agent) left off

Later — or from a different agent — resume from the predecessor session:

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

Resume creates a new AWR session; it does not switch your host's native chat.
Then compile context again (step 3) and continue. After host compaction, if the
same AWR session is still active, just compile again — reserve
`session resume` for a real handoff.

To compare the plan with what was last recorded, check the `progress` object
exposed by `status --view action` and `work show KEY`:

- `source_next_action` — the authoritative text from the source plan, with its
  locator, source revision, and freshness.
- `latest_checkpoint_next_action` — the most recent checkpoint for this exact
  work, branch, and ownership, or null if there is none.
- `differs_from_source` — true when the checkpoint text diverges from the
  source. An observation, not a problem: saving progress never modifies the
  original contract or establishes completion.

Progress text is capped at 240 characters (flagged `truncated`); use
`work show KEY` and `session show SESSION` for the full text.

## When something goes wrong

- Uncertain save response → check `host status --key REQUEST_KEY`; use
  `host recover` only for an inspected pending operation.
- Revision conflict → read `status` for the current `project_revision` and
  retry with it.
- Suspicious checkpoint → read the full record with `session show SESSION`; an
  interrupted save without a completion receipt is no recovery checkpoint.

For more failure modes, see [Troubleshooting](troubleshooting.md). For the
vocabulary in this routine, see [Concepts](concepts.md).
