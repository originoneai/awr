# Team MCP workflow · Claude Code (`claude_code`) — AWR-TMCP-041

> **Remote Team MCP**, not personal stdio `awr-mcp`. Do not treat
> `awr team command` placeholders as a live remote transport
> ([team-access.md](../reference/team-access.md)).
>
> WS-024 adapter id: [`claude_code`](named-agent-host.md) (not auto-startable).
> Capability summary: [claude-code-agent.md](claude-code-agent.md).

## Prerequisites

From the operator: [member handoff](../reference/team-member-handoff.md) —
HTTPS MCP URL, personal bearer claim, and repo.

Merge
[`examples/team-mcp-deploy/clients/claude_code.mcp.json.example`](../../../examples/team-mcp-deploy/clients/claude_code.mcp.json.example)
into Claude Code's MCP config (user or project). Bearer via env / headers only.

Start Claude Code yourself (AWR will not launch it). Confirm the Team MCP server
lists `awr_team_query` and `awr_team_command`.

## Tools

Same Team surface as Codex. Arguments are production `WorkstreamQuery` /
`WorkstreamCommand` JSON (`deny_unknown_fields`): query fields are **top-level**
(no `args` wrapper); commands require the full precondition envelope plus
op-specific `args`. See [team-mcp-codex-cli.md](team-mcp-codex-cli.md) for the
complete reusable envelopes — copy values from an authorized `work.prepare`
before the first mutation.

| Tool | Use |
| --- | --- |
| `awr_team_query` | `capabilities`, work/session/claim reads, controlled content |
| `awr_team_command` | session / claim / execution / delivery / rework / complete |

Use a stable `request_id`. Prefer `command.inspect` / `planning.outcome` on
uncertainty before retrying with a new id.

## Natural workflow

### 1. Query

Call `awr_team_query` with:

```json
{"protocol_version":1,"op":"capabilities"}
```

then list/search on your authorized stream (top-level fields):

```json
{"protocol_version":1,"op":"work.list","workstream_id":"00000000000000000000000001","limit":20}
```

```json
{"protocol_version":1,"op":"work.search","workstream_id":"00000000000000000000000001","search":"alpha","limit":20}
```

Only claim work you can see.

### 2. Prepare, then session / claim

1. `work.prepare` for the chosen `work_id` (fills the command envelope).
2. `session.start` with `conversation_id` like `claude:<native-session-id>`.
3. `claim.acquire` with session + expected work/session versions and TTL.
4. `claim.renew` on the active fence/lease before expiry.
5. `claim.inspect` when unsure — do not interpret an acquire replay as renew.

Example prepare + session.start envelopes:

```json
{
  "protocol_version": 1,
  "op": "work.prepare",
  "workstream_id": "00000000000000000000000001",
  "work_id": "API-1",
  "max_context_bytes": 120000
}
```

```json
{
  "protocol_version": 1,
  "request_id": "00000000-0000-4000-8000-000000000011",
  "op": "session.start",
  "workstream_id": "00000000000000000000000001",
  "work_id": "API-1",
  "coordinator_epoch": "epoch-a",
  "expected_project_revision": "3",
  "expected_authority_version": "1",
  "expected_ownership_version": "1",
  "expected_contract_hash": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
  "args": {"conversation_id": "claude:<native-session-id>"}
}
```

Bind external session labels with L0 habits from
[claude-code-agent.md](claude-code-agent.md) (`claude:<native-id>`), but perform
coordination on **Team** tools above.

### 3. Context

Re-run `work.prepare` when the read set may have moved. Read `required_specs`
and `authorized_readable_refs`. Fetch bodies only through `source.content` /
`artifact.content`.

### 4. Checkpoint

`session.checkpoint` with the **consumed** `context_hash`, concrete
`next_action`, and `open_loops` inside a full command envelope:

```json
{
  "protocol_version": 1,
  "request_id": "00000000-0000-4000-8000-000000000012",
  "op": "session.checkpoint",
  "workstream_id": "00000000000000000000000001",
  "work_id": "API-1",
  "coordinator_epoch": "epoch-a",
  "expected_project_revision": "5",
  "expected_authority_version": "1",
  "expected_ownership_version": "1",
  "expected_contract_hash": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
  "args": {
    "session_id": "<id>",
    "expected_session_version": "1",
    "context_hash": "<hash from prepare you actually consumed>",
    "next_action": "Implement filtered query API",
    "open_loops": ["Await schema review"]
  }
}
```

Closing Claude Code or its MCP transport does not end the logical Team session.
Ongoing work may checkpoint and keep its valid ownership. Only when logical
closure is intended, settle execution effects and resolve open waits. If this
session holds an active claim, release it with its current fence and lease
version. Then call `session.end` with the current session version. This closes
coordination state; it does not complete, approve or integrate the work. A
returned `known_rejection` is a known refusal: follow its action guidance rather
than treating it as a missing response or repeating the unchanged request. An
authorized handoff remains a separate continuation path.

### 5. PR link

Develop in your git checkout of the handed repo. After opening the PR, register
facts with a full command envelope:

```json
{
  "protocol_version": 1,
  "request_id": "00000000-0000-4000-8000-000000000013",
  "op": "delivery.register_pr",
  "workstream_id": "00000000000000000000000001",
  "work_id": "API-1",
  "coordinator_epoch": "epoch-a",
  "expected_project_revision": "6",
  "expected_authority_version": "1",
  "expected_ownership_version": "1",
  "expected_contract_hash": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
  "args": {
    "session_id": "<id>",
    "expected_session_version": "2",
    "repository": "originoneai/example",
    "pr_number": 1,
    "pr_url": "https://github.com/originoneai/example/pull/1",
    "head_sha": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "fact_source": "authorized_human_github_verification",
    "observed_at": "2026-09-23T04:00:00+08:00"
  }
}
```

Then `evidence.submit` / `review.open` / `delivery.submit_and_request_review` as
authorized. See [pr-delivery-review.md](pr-delivery-review.md). GitHub UI state
is not AWR completion.

### 6. Rework

On `review.return`, author runs `work.rework`, fixes the head, notifies with
`delivery.observe_pr` (`expected_head_sha`), and requests a fresh review round.
Retained history keeps failed/rejected rounds.

### 7. Complete

Eligible independent person: `review.accept` / `review.decide`. Maintainer:
`delivery.finalize` / `work.complete`. Agents of the same person are not
team-independent reviewers.

## Host adapter note

`claude_code` supports status / forensics only when backed by
attributable observation; **start**, **stop_confirmation** and
**reconnect_resume** return human continuation. Prefer reconnecting the same
execution identity before starting anything new
([named-agent-host.md](named-agent-host.md)).
