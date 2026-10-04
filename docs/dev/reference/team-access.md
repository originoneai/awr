# Team command validation and remote profiles

The development branch separately provides a
[Team workstream HTTP/MCP service](team-workstream-service.md). It authenticates
queries, sessions, claims, caller-managed execution and authorized recovery in
PostgreSQL. [Operator access provisioning](team-operator-access.md) uses a separate
schema-owner CLI. The legacy command limitations below still apply; personal MCP
and remote-profile commands do not route to that service.

The public Team command path does **not** yet implement authenticated HTTP/MCP
transport, credential loading, queuing or domain dispatch. `awr team command`
and `awr-server command` return `Unsupported` for a valid command; they never
report `accepted=true`, replay status or cached results. Missing/offline remotes
cannot fall back to personal SQLite authority. `awr team capabilities` describes
local protocol metadata, explicitly reports `local=true`, `submitted=false` and
`command_transport=false`, and does not contact a server.

The shared pure parser currently recognizes `capabilities` and `work.claim`.
Only capabilities are locally executable. Claim has a validation schema, not a
remote dispatch target. Other operations, including `execution.cancel` and
`source.activate`, return `Unsupported` online and offline. PostgreSQL store APIs
are separate from this wire entry. Future adapters must register an operation's
schema, read/write policy and actual dispatch before announcing support.

Received envelopes must be JSON objects with explicit `protocol_version`,
nonempty `request_id` and `op`. Receivers never invent these fields. Only version
1 (integer or canonical decimal string) is supported; conversion never truncates.
`args` must be an object. Claim requires nonempty string `work_id`, `scope_id`,
`session_id`, `expected_contract_hash`, and an unsigned `expected_work_version`
(integer or canonical decimal string; decimal strings preserve all 64 bits).
Unknown argument fields are rejected. Business preconditions remain the domain
store's responsibility. Capabilities permit only empty args, or omitted args.

For local integration testing, the server skeleton offers an explicit validator:

```sh
awr-server validate-command --op work.claim \
  --tenant-id tenant-a --project-id project-a --actor-id actor-a --client-id client-a \
  --body '{"protocol_version":1,"request_id":"example","op":"work.claim","args":{"work_id":"work-a","scope_id":"main","session_id":"session-a","expected_work_version":"1","expected_contract_hash":"example-hash"}}'
```

This performs no database/network operation and returns `validation_only=true`,
`submitted=false`, `authenticated=false`, and the separately supplied context.
Those local flags are **test context, not authentication credentials**. Request
identity declarations cannot override any tenant, project, actor or client field.
`awr_team::execute` also checks retained declarations against its supplied context;
a future authenticated adapter must obtain that context independently of the body.
The `http`, `mcp`, `cli` labels in pure-function tests are not three live transports.
Personal `awr-mcp` has no Team command dispatch tool; its shared MCP service remains
independent of this Team skeleton.

`awr remote` stores an endpoint, project key and credential environment *name*,
using TOML serialization. Profiles are validated on both save and load. Endpoints
must be absolute HTTPS URLs, or HTTP URLs on localhost/loopback IPv4/IPv6. Database
schemes, user/password information, query strings and fragments are rejected;
credentials belong in the referenced environment, not the URL. Output independently
replaces invalid endpoints with a placeholder, and TOML parse errors never echo
configuration contents. Valid Unicode project keys roundtrip through add/inspect.
Profiles must explicitly declare supported `protocol_version = 1`.

These command-validation checks do not establish authenticated network isolation,
PostgreSQL authorization, business acceptance or release readiness.


## Team MCP business action permissions (AWR-TMCP-010)

This section freezes the Team MCP **business** permission contract. It is separate
from the command-envelope validation above. Enforcement at live MCP/HTTP/PG domain entry points is AWR-TMCP-011 (shared
`authorize_command` / `authorize_query` decision). This section freezes types,
fixtures and rules; the live gate maps current workstream commands onto these
actions and refuses planning/access/audit ops for roles that lack them.

### Role templates

| Template | Meaning |
| --- | --- |
| `reader` | Read authorized work and context |
| `developer` | Claim, maintain own session/execution, propose planning changes, submit delivery |
| `maintainer` | Edit/approve/publish planning drafts and finalize delivery |
| `project_admin` | Manage project membership/grants and read project audit export |

`review.decide` is **not** part of any template. It requires an explicit
independent-review add-on for an eligible non-reader member. Project admins have
no bypass.

### Action matrix

Unknown actions default to **deny**. Action names are proposed business semantics;
they are not necessarily current CLI/MCP command names.

Fixtures: `tests/fixtures/team-mcp/role_action_matrix.json`.

| Action | reader | developer | maintainer | project_admin |
| --- | :---: | :---: | :---: | :---: |
| `work.read` | yes | yes | yes | yes |
| `session.maintain_own` | — | yes | yes | yes |
| `claim.manage_own` | — | yes | yes | yes |
| `execution.request_and_report_own` | — | yes | yes | yes |
| `planning.propose` | — | yes | yes | yes |
| `planning.edit_draft` | — | — | yes | yes |
| `planning.approve` | — | — | yes | yes |
| `planning.publish` | — | — | yes | yes |
| `delivery.submit_and_request_review` | — | yes | yes | yes |
| `review.decide` | — | add-on | add-on | add-on |
| `delivery.finalize` | — | — | yes | yes |
| `access.manage_project` | — | — | — | yes |
| `audit.read_project` | — | — | — | yes |

### Authority scope

An allow decision requires an `AuthorityScope` that binds all of:

- `tenant_id` / `project_id` (required)
- optional `workstream_id` and optional `work_ids` set
- `person_id`, optional `execution_identity`, `client_id`
- `allowed_actions`, optional independent-review flag
- `policy_version`, expiry (`not_after_unix_ms`) and revocation

Role display names, tool discovery/visibility lists and model self-reports **cannot**
authorize any action by themselves (`deny_role_name_only`,
`deny_tool_visibility_only`, `deny_model_self_report_only`).

Project admin authority is project-scoped: it does not grant other projects,
`database_owner`, schema migration, trusted executor attestation, execution
reconciliation, arbitrary source filesystem control or tenant-wide recovery.

### Legacy migration preview

Coarse legacy roles `reader` / `reviewer` / `worker` / `admin` and grants
`read` / `write` / `manage` produce a **preview** only
(`preview_legacy_migration`). Effective actions are the **intersection** of
legacy membership and client grant templates (never a union); missing either
side contributes no actions from that side. Newly introduced privileges
(`planning.*`, `access.manage_project`, `review.decide`) are always withheld,
independent of which grant branch is supplied. Person links without verified
evidence stay `unknown` and grant nothing.

Fixtures: `tests/fixtures/team-mcp/migration_preview.json` and
`tests/fixtures/team-mcp/allow_deny_pairs.json`.

## Delegated Agent read scope

Native Agent reads intersect access grants with live WS-016 delegations.
`workstreams.list` and `work.next` discover all independently authorized readable
workstreams. `work.list`, `work.search` and `events.list` require a returned
`workstream_id` when more than one readable workstream is available. An explicit
selection uses a delegation covering that workstream; credential access alone
does not disclose an undelegated stream. Scoped Agents cannot inspect
project-wide planning outcome receipts. A live `propose_planning` delegation
does allow recovery of the same actor and client's own `planning.propose`
receipt. One current delegation must cover every affected task, and the current
task owners must remain readable. Separate task grants cannot be combined to
recover a wider suggestion. An explicit project delegation retains project-wide
inspection; ordinary scoped `inspect` grants do not gain it.

HTTP, dedicated MCP outcome lookup and query `planning.outcome` use the same
checks. Missing or still-reserved commands remain unknown and expose no reserved
payload. Lookup neither resubmits a suggestion nor grants execution, approval or
publication authority. Revoked or expired delegations, disabled person bindings,
changed clients and lost access are checked again on every lookup.

Navigation checks each task's actions against a covering delegation. A claim
permission on one workstream never makes another read-only workstream claimable.
Commands still select one covering authorization independently on every request.
Discovery cursors bind the effective authorization bodies as well as identity,
access grants and source, so changes invalidate earlier pages. Resume queries
carry the returned work and workstream selectors to resolve the correct grant.

Task delegations also support selector-free `work.next`, limited to exact assigned
tasks whose current owners are independently readable through credential access.
Their own current-client sessions on those tasks can be resumed. This does not
expose the owning stream's catalog, sibling tasks or other discovery surfaces;
other reads still require an explicit covered `work_id`, including session
inspection. Task-pool delegations continue to require an explicit covered task
on every read. Project-scoped delegation retains
project discovery. Humans and system actors retain their existing access rules.
Agents start with an empty product-action set until a live delegation is
resolved. Separate audit/access surfaces do not inherit an Agent's broad role
without such authorization.
