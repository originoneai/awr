# The MCP service

Connect an agent host to AWR through local stdio or a shared HTTP service. MCP tools read project facts, compile context and record work or evidence.

## Use it

AWR exposes work, context, evidence and session operations through MCP (Model Context Protocol). Agent hosts discover tools and call them directly. Choose local stdio for one project or shared HTTP for several registered projects and independent clients.

- **stdio** — a local process your client spawns directly, one project at a
  time. This is the classic setup and remains available.
- **Shared HTTP** — a single Streamable HTTP endpoint serving multiple projects
  and multiple independent clients. Both the npm and PyPI packages include this
  capability.

This article focuses on the shared HTTP service, which is what you run when
more than one person or agent needs the same projects. For what agents do with
these tools once connected, see [Agents](agents.md). For the human-facing
equivalent of the same operations, see [The AWR CLI](cli.md).

### How the shared service works

You initialize each project on the server with `awr init`, then register its
canonical absolute root together with the `project_id` returned by:

```sh
awr --json status
```

AWR verifies each registered project identity at startup and on every request. Each tool selects a registered `project`; files and databases stay separate. Clients connect over the network to files readable on the server.

### Configuring the service

Create an operator-owned TOML file outside the public repository:

```toml
version = 1
allowed_hosts = ["awr.internal.example"]

[[projects]]
key = "billing"
root = "/srv/projects/billing"
project_id = "<actual project ID>"

[[projects]]
key = "support"
root = "/srv/projects/support"
project_id = "<actual project ID>"

[[clients]]
id = "engineering"
token_env = "AWR_ENGINEERING_TOKEN"
write = ["billing", "support"]

[[clients]]
id = "reviewer"
token_env = "AWR_REVIEWER_TOKEN"
read = ["billing"]
```

Each client names an environment variable holding a distinct, randomly generated bearer credential of at least 32 characters. `write` includes read access. Keep client IDs stable through credential rotation; changing an ID creates a separate conversation/request scope. Restart to apply configuration changes.

### Running the service

```sh
awr-mcp --registry /etc/awr/service.toml --listen 127.0.0.1:8080
```

Each HTTP request authenticates with a static bearer credential. For remote access, terminate HTTPS at an authenticated deployment boundary and restrict direct backend access. Additional access controls:

| Control | Rule |
| --- | --- |
| `allowed_origins` | Explicit allowlist; native clients usually omit Origin |
| `allowed_hosts` | SDK enforcement, defaulting to loopback hosts |
| Browser frontend | Gateway handles CORS preflight |

On Unix, SIGTERM and Ctrl-C stop new requests and drain active HTTP work. Use your deployment’s process supervisor for startup and restart. For local, single-client use:

```sh
awr-mcp --project /absolute/project
```

### Connecting a client

Configure each MCP client for Streamable HTTP with the same service URL using
the `/mcp` path, plus its own bearer credential in an `Authorization: Bearer …`
header delivered through that client's credential mechanism. Once connected,
call `awr_projects_list` to discover the project keys your credential is
authorized for. Every project tool then requires `project`:

```json
{"project": "billing", "work": "INVOICE-001"}
```

## How it behaves

| Transport | Setup and result |
| --- | --- |
| stdio | Client spawns `awr-mcp` for one canonical project |
| Shared HTTP | One endpoint serves registered projects and independently authenticated clients |
| Discovery | Inspect the installed build’s catalog, then call a domain empty for child schemas and limits |

### The tools

The shared HTTP service exposes 21 tools in total (20 over stdio, which omits
the project-listing tool). They fall into three groups.

#### Work and context tools

These mirror the everyday CLI actions:

| Tool | What it does |
| --- | --- |
| `awr_project_status` | Current continuation, claimable work, waits, blockers, history summary |
| `awr_work_ready` | Ready items with diagnostics and claim hints |
| `awr_work_get` | A work item's tasks, acceptance, dependencies, decisions, evidence |
| `awr_context_compile` | Compile the context packet for a work item or branch |
| `awr_work_transition` | Progress, block, unblock, cancel, reopen, or complete work |
| `awr_event_append` | Append an event to the ledger |
| `awr_evidence_record` | Record evidence bound to work and acceptance items |
| `awr_search` | Search across the project ledger |

#### Session and continuation tools

Sessions bind a conversation to a unit of work. Supply a stable `conversation`
identifier from your host — not an HTTP connection identifier. The same
conversation string under another project or client is a separate binding.

| Tool | What it does |
| --- | --- |
| `awr_session_start` | Start a work-bound session, optionally claiming the work |
| `awr_session_get` | Inspect binding, session, claims, checkpoint, interrupted saves |
| `awr_session_list` | Page through this client's session history |
| `awr_session_checkpoint` | Persist the consumed context hash, digest, next action, open loops |
| `awr_session_claim` | Acquire or release the session's claim |
| `awr_session_end` | Explicitly end or interrupt a session and release its claims |
| `awr_session_resume` | Create a successor session with inherited checkpoint and claims |
| `awr_session_wait` | Save a checkpoint and record a persistent question for the user |
| `awr_session_reply` | Deliver the user's answer to a recorded wait |
| `awr_operation_get` | Inspect the recorded outcome of a write by request ID |
| `awr_operation_recover` | Record a committed outcome for an interrupted write when proof exists |
| `awr_source_reindex` | Refresh the project's projections from its authoritative sources |

A pending wait blocks transitions and resume until the host records a reply. Then inspect the session, compile fresh context and choose continuation or a successor.

### Writes, revisions, and uncertain outcomes

Every shared HTTP write requires two things:

- a client-generated, stable `request_id` (up to 256 bytes),
- the `expected_revision` you last observed.

```json
{
  "project": "billing",
  "request_id": "host-turn-42-start",
  "expected_revision": 120,
  "work": "INVOICE-001",
  "conversation": "invoice-review",
  "agent": "billing-assistant",
  "provider": "example-provider",
  "model": "example-model",
  "claim": true
}
```

An exact request ID/argument replay returns the recorded result. Changed arguments require a new ID. Use the returned `project_revision` for the next write.

After timeout or disconnect, inspect `awr_operation_get` with the same project and request ID. Unfinished requests report `write_outcome: unknown`. Reads run concurrently; writes serialize per project with independent locks across projects. On stale-write conflict, read current state and reconsider.

### Workstream reads (in development)

Development registry `version = 2` grants reads for explicitly listed immutable workstreams in YAML workstream projects.

```toml
[[clients.workstreams]]
project = "billing"
workstream_id = "<actual immutable workstream ID>"
authority_version = 1
```

Authorized clients discover shared-only `awr_workstream` capabilities and listed scopes with `action: "capabilities"` and `action: "list"`. Enablement and reindex remain local operator actions.

### Where to go next

- [Agents](agents.md) — how an agent uses sessions, claims, and checkpoints
  over these tools.
- [The AWR CLI](cli.md) — the same domain operations from the command line,
  for operators and debugging.
- [Troubleshooting](troubleshooting.md) — revision conflicts, stale sources,
  and recovery.

## Limits

- The service uses registered server roots; repository cloning, client filesystem mounts, ledger merging and arbitrary server-path opening are outside its scope.
- Client-local paths remain local. Each project call carries its own selector; clients can connect independently of a local AWR executable.
- Read grants authorize reads; mutations require a write grant.
- Built-in authentication is static bearer authentication; OAuth authorization and enterprise identity services belong to the deployment.
- Origin validation covers the request Origin; browser CORS preflight requires a gateway.
- Disconnect and service restart preserve AWR work sessions. End those sessions explicitly; daemon installation belongs to the deployment.
- Replies clear waits while preserving work status; the host schedules its next turn.
- Request journaling and domain writes both consume revisions. Use observed revisions instead of computing the next number.
- A closed response stream leaves execution outcome unknown. Recovery requires an unambiguous bound terminal event and never re-invokes the original tool.
- Isolated workstreams require explicit operator-owned workstream grants; project-level read/write grants alone are insufficient.
- Workstream access covers reads; mutations, content-file reads and Team PostgreSQL operations require separate implementations.
- Enabling isolated workstreams rejects legacy shared tools, including writes, until an authorized implementation supports them.

## Code and tests

- [Stdio contract tests](../../crates/awr-mcp/tests/stdio.rs)
- [Shared HTTP tests](../../crates/awr-mcp/tests/shared.rs)
- [Description limits](../../crates/awr-mcp/tests/description_style.rs)
