# Team MCP workflow · Codex (`codex_cli`) — AWR-TMCP-041

> **Remote Team MCP**, not personal stdio `awr-mcp`. Do not treat
> `awr team command` placeholders as a live remote transport
> ([team-access.md](../reference/team-access.md)).
>
> WS-024 adapter id: [`codex_cli`](named-agent-host.md). Personal Codex L1 + L2 notes
> remain in [codex.md](codex.md); this page is the **Team service** natural path.

## Prerequisites

From the operator: [member handoff](../reference/team-member-handoff.md) —
HTTPS MCP URL, personal bearer claim, and repo. Clone the repo for code work;
coordination goes through Team MCP.

Merge the remote MCP template
[`examples/team-mcp-deploy/clients/codex_cli.mcp.toml.example`](../../../examples/team-mcp-deploy/clients/codex_cli.mcp.toml.example)
into the trusted project's `.codex/config.toml` (or register with `codex mcp`).
Load the bearer from the environment — never commit it.

Reload Codex and confirm `/mcp` shows the Team server connected.

## Tools

| Tool | Use |
| --- | --- |
| `awr_team_query` | Reads: `capabilities`, `work.list` / `search` / `prepare`, `claim.inspect`, `session.inspect`, `planning.outcome`, … |
| `awr_team_command` | Writes: `session.*`, `claim.*`, `execution.*`, `evidence.*`, `review.*`, `delivery.*`, `work.rework` / `work.complete` |

Tool arguments are the **production** `WorkstreamQuery` / `WorkstreamCommand`
JSON objects (`deny_unknown_fields`). Query fields are **top-level** (there is
no `args` wrapper on `awr_team_query`). Commands carry workstream/work IDs,
coordinator epoch, and expected project/authority/ownership/contract
preconditions at the top level; the op-specific body lives in `args`.

Stable `request_id` on every command. On disconnect, inspect the **same**
`request_id` before minting a new one.

## Natural workflow

Placeholders below that look like `<from prepare…>` must be copied from an
authorized `work.prepare` (or prior command receipt) **before** the first
mutation. Example numeric ULID `00000000000000000000000001` stands in for a real
`workstream_id` from capabilities / prepare.

### 1. Query

`awr_team_query` payloads (top-level fields only):

```json
{"protocol_version":1,"op":"capabilities"}
```

```json
{"protocol_version":1,"op":"work.list","workstream_id":"00000000000000000000000001","limit":20}
```

```json
{"protocol_version":1,"op":"work.search","workstream_id":"00000000000000000000000001","search":"alpha","limit":20}
```

Pick a claimable work id from the authorized list only.

### 2. Prepare (read set before mutation)

Call `work.prepare` **before** `session.start` / `claim.acquire`. The response
supplies `workstream_id`, `work_id`, `coordinator_epoch`, `project_revision`,
`authority_version`, `ownership_version`, and `contract_hash` for the command
envelope.

```json
{
  "protocol_version": 1,
  "op": "work.prepare",
  "workstream_id": "00000000000000000000000001",
  "work_id": "API-1",
  "max_context_bytes": 120000
}
```

Consume the returned contract / `required_specs` / `authorized_readable_refs`
before mutating. Controlled body reads use `source.content` /
`artifact.content` — never invent server paths.

### 3. Session / claim / renew

Full `awr_team_command` envelopes (preconditions + `args`):

```json
{
  "protocol_version": 1,
  "request_id": "00000000-0000-4000-8000-000000000001",
  "op": "session.start",
  "workstream_id": "00000000000000000000000001",
  "work_id": "API-1",
  "coordinator_epoch": "epoch-a",
  "expected_project_revision": "3",
  "expected_authority_version": "1",
  "expected_ownership_version": "1",
  "expected_contract_hash": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
  "args": {"conversation_id": "codex:<native-thread-id>"}
}
```

```json
{
  "protocol_version": 1,
  "request_id": "00000000-0000-4000-8000-000000000002",
  "op": "claim.acquire",
  "workstream_id": "00000000000000000000000001",
  "work_id": "API-1",
  "coordinator_epoch": "epoch-a",
  "expected_project_revision": "4",
  "expected_authority_version": "1",
  "expected_ownership_version": "1",
  "expected_contract_hash": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
  "args": {
    "session_id": "<from session.start receipt>",
    "expected_session_version": "1",
    "expected_work_version": "0",
    "ttl_seconds": 3600
  }
}
```

Renew before expiry with `claim.renew` (fence + lease version from the receipt).
Use `claim.inspect` for **current** lease state — replay of `claim.acquire` is a
historical receipt, not a renew.

### 4. Checkpoint

After real progress (reuse a fresh prepare when the read set may have moved):

```json
{
  "protocol_version": 1,
  "request_id": "00000000-0000-4000-8000-000000000003",
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

MCP disconnect ≠ session end. Exit with `session.end` only when finished or
handing off.

### 5. PR link

Open the GitHub PR from the repo checkout, then register facts on Team MCP
([pr-delivery-review.md](pr-delivery-review.md)):

```json
{
  "protocol_version": 1,
  "request_id": "00000000-0000-4000-8000-000000000004",
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

A URL alone is not acceptance. Submit evidence / request review with
`evidence.submit`, `review.open`, `delivery.submit_and_request_review` as
authorized.

### 6. Rework

When review returns the round, acknowledge with `work.rework` (author
acknowledgment — not independent review). Fix code, push a new head, then
`delivery.observe_pr` with `expected_head_sha` and re-open review as needed.
Old approvals bound to a prior head/contract invalidate.

### 7. Complete

Independent reviewer calls `review.accept` / `review.decide` under
`independent_review`. Maintainer finalizes with `delivery.finalize` /
`work.complete`. Green CI, admin role, or GitHub merge **never** skip AWR
acceptance.

## Host adapter note

For WS-024 capability negotiation, adapter id is `codex_cli`. Coordination
admission is not process start/kill — see [named-agent-host.md](named-agent-host.md).
