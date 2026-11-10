# Daily workflow

Find the next action, claim or resume work, read its context and save progress for the next session. This routine works through CLI or MCP.

## Use it

Before you start, make sure your project is initialized and you know the basic
concepts (work items, sessions, claims, checkpoints) — see
[Quickstart](quickstart.md) and [Concepts](concepts.md). The same routine works
over MCP; see [MCP](mcp.md).

Run `awr --help` to check the capabilities of your installed build.

### 1. Start your day: find the next action

Run a status check before doing anything else:

```sh
awr status
```

```text
Guide project — 1 to continue · 1 claimable · 0 waiting · 0 blocked

Continue (1 of 1)
  GUIDE-1  Review the revised draft  [note, 2h]
Claimable (1 of 1)
  GUIDE-2  Draft the introduction  [ledger, 1d]

Next: awr work prepare GUIDE-1
More: awr work show <KEY> · awr status --view full
[ledger] = Read from an authoritative project source. [note] = Supplied by a caller without an independent program check.
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

`awr ready` selects work eligible for a new claim. Use the action queues for continuation and waiting.

## How it behaves

| Stage | Result |
| --- | --- |
| Status | Continue, claimable, waiting and blocked queues |
| Session | Owned work and a current context packet |
| Checkpoint | Recorded progress and recovery action |
| Completion | Evidence bound to current acceptance |

### 2. Claim or resume the task

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

A claim records runtime ownership.

A project can declare a default claim lifetime and renewal in `.awr/policy.toml`; with it `--ttl-ms` can be left out, and `--no-expiry` asks for a claim without an expiry. See [claim policy](../dev/reference/claim-policy.md).
If a session already exists for the work, inspect it with `session show` and
confirm ownership — a different agent or model continues through
`session resume` into its own session.

### 3. Compile your context

Before editing, build the context packet for the session:

```sh
awrj context bootstrap --session "$AWR_SESSION" --budget 1000 \
  > "$AWR_NOTES/bootstrap.json"
jq -e '.context.complete' "$AWR_NOTES/bootstrap.json"

awrj context compile --work "$AWR_WORK" --session "$AWR_SESSION" \
  --budget 5000 > "$AWR_NOTES/context.json"
jq -e '.completeness.complete and (.work_context != null)' "$AWR_NOTES/context.json"
```

Read the complete packet. On `BudgetExceeded`, widen the budget or narrow the scope; on `SourceStale`, run `awr source reindex` first.

### 4. Work, then checkpoint

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

Use the top-level `project_revision` returned by `session show`. On conflict, inspect intervening changes before retrying. Change the authoritative source to revise the work contract.

### 5. Preview small field edits

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

Supported fields are `--title`, `--summary`, `--priority`, and `--next-action`. After an uncertain response, check `host status --key guide-next-1`.

### 6. End the session — or hand it off

When you stop for the day:

```sh
AWR_REV=$(awrj status | jq -er '.project_revision')
awrj session end --session "$AWR_SESSION" --outcome incomplete \
  --expected-revision "$AWR_REV"
```

Ending releases the claim. The checkpoint from step 4 supplies the next recovery action.

### 7. Pick up where you (or an agent) left off

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

Resume creates a new AWR session. Compile context again (step 3). After compaction, compile for the still-active session; use resume for a handoff.

To compare the plan with what was last recorded, check the `progress` object
exposed by `status --view action` and `work show KEY`:

| Progress field | Meaning |
| --- | --- |
| `source_next_action` | Authoritative plan text, locator, source revision and freshness; `trust=source` |
| `latest_checkpoint_next_action` | Latest checkpoint for this work, branch and ownership, or null; `trust=reported` |
| `differs_from_source` | Whether checkpoint and source text differ |

Progress text is capped at 240 characters (flagged `truncated`); use
`work show KEY` and `session show SESSION` for the full text.

### When something goes wrong

| Situation | Next action |
| --- | --- |
| Uncertain save | Read `host status --key REQUEST_KEY`; recover an inspected pending operation |
| Revision conflict | Read `status`, inspect changes and retry with the current `project_revision` |
| Suspicious checkpoint | Read the full record with `session show SESSION` |

For more failure modes, see [Troubleshooting](troubleshooting.md). For the
vocabulary in this routine, see [Concepts](concepts.md).

## Limits

- The action view and `work edit` described here follow development source; released packages have their own capability boundary.
- Queue rows are navigation; ownership and completion are checked by their respective operations.
- Claims leave source work status unchanged.
- Checkpoint `--agent` is caller reported; mismatches require resume, matching labels remain unauthenticated, and omission records `undeclared`.
- Use the actual last-consumed context hash. Digest and hash are caller assertions; checkpoint success records progress rather than verified tests or completion.
- The revision guard uses `project_revision`; `session.revision` belongs to the session object.
- Checkpoint next actions preserve the authoritative source-backed next action.
- Field edits update planning text; ownership, lifecycle and verification use their own operations.
- Ending a session leaves source completion to the completion operation.
- AWR resume changes its own session identity; the host retains control of its native chat.
- Checkpoint progress records observations; the source contract and completion remain separately governed.
- Recovery checkpoints require a completed save receipt; an interrupted attempt remains an attempt.

## Code and tests

- [Status output tests](../../crates/awr-cli/tests/checkpoint_progress.rs)
- [Session lifecycle](../../crates/awr-cli/tests/session_cli.rs)
