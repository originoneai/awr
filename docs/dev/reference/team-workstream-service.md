# Team workstream HTTP and MCP service

The development branch provides authenticated, multi-project HTTP/MCP queries
and durable sessions, coordination claims and caller-managed execution backed by
PostgreSQL. This is not a release
announcement or a complete Team execution service. It does not dispatch
executions, resume agents or adopt cross-workstream deliveries. Use its live
capabilities response to discover available operations.
Unsupported capabilities are refused. Live authorization is enforced in PostgreSQL
transactions shared by HTTP and MCP domain entries; clients must not treat UI
filtering as an ACL. AWR-TMCP-011 maps each command/query onto the frozen
TMCP-010 business action matrix at that same gate: verified bearer credentials
supply identity, while request bodies, tool names and reconnects cannot forge
actor, role or grants. Readers cannot claim or write; developers may maintain
their own sessions/executions on authorized work but cannot edit/publish plans
or manage project access. Exact receipt replay reuses the original result;
changed intent or expired/revoked authority is refused without a business write. Team rows retain historical `scope_id=main` while
`workstream_id` isolates streams. Legacy unscoped Team entrypoints refuse writes
against enabled projects. An operator-local
[reference runner](team-reference-runner.md) can consume scoped admissions for
bounded file writes and attest their results; those local filesystem effects are
not a server ACL or confidentiality sandbox.

## Start an operator-bound service

Build `awr-server` from this source branch. Migrate the intended database to
schema 65 explicitly as its owner, and apply application-role grants using the
[PostgreSQL setup](team-postgres.md). `serve` checks the schema without migrating
it. Run the listener using the application connection, not an owner or superuser
connection.

```toml
version = 1
listen = "127.0.0.1:9908"
allowed_hosts = []

[[projects]]
key = "example"
tenant_id = "tenant-a"
project_id = "project-a"
```

```sh
# AWR_TEAM_DATABASE_URL is supplied by the operator's environment.
awr-server serve --config /absolute/path/team-service.toml
```

Each additional `[[projects]]` entry binds another public alias to an exact
tenant/project pair. The process shares a connection pool across aliases. The
request path selects an alias; request JSON cannot supply tenant, project,
actor, client or grants. Possession of an alias grants no access.

The loopback listener accepts its actual host/port and `localhost` at that port.
Non-loopback listeners require explicit `allowed_hosts` entries. Hosts are exact
authorities, including a port when clients send one. This server has no built-in
TLS: remote use requires operator-provided HTTPS termination and a protected
connection to the backend. Classic `/v1/projects/*` and MCP reject all browser `Origin` headers and have
no cookie login. WS-044 adds an explicit `/v1/web/*` entry gated by
`allowed_web_origins` with HttpOnly session cookies; see
[Team Web entry](../integrations/team-web-entry.md).

The project must have an explicitly approved and activated
[`workstreams.json` source bundle](workstreams.md#team-source-projection-in-the-development-branch).
Activation grants no permissions. A legacy project returns `Unsupported` after
authentication; old Team operations continue to refuse enabled projects.

## Credential and grant provisioning

Use [`awr-server access`](team-operator-access.md) to generate a local bearer file, inspect
and preview one client's policy, apply it with a state digest, and query an
uncertain result. This schema-owner CLI creates actors, project membership,
credentials and explicit workstream grants for an already enabled project.
It is not a public HTTP/MCP admin endpoint. Do not give agent clients direct
database credentials or use the owner connection for the running service.

A bearer has the form `awr1.<credential-id>.<secret>`. The ID contains 1–128 ASCII
letters, digits, underscores or hyphens; the secret is 32 cryptographically random
bytes encoded as 64 lowercase hexadecimal characters. Generate secrets with an
OS cryptographic generator; test-fixture tokens are never production credentials.
Store only the result of `awr_team_pg::workstream_credential_hash(bearer)` in
`awr_team.credentials.secret_hash`. Its versioned formula is:

```text
"sha256:" + lowercase_hex(SHA256(UTF8("awr-team-credential-v1:" + bearer)))
```

The credential row binds the ID to one tenant, actor and client, with optional
expiry and revocation. Each `workstream_grants` row binds the project, actor,
client and workstream ID to its current `authority_version`. Reading requires
an active `can_read` grant. Project membership, including the admin role, does
not implicitly grant all workstreams. Increment `grant_version` when changing a
grant and `membership_version` when changing membership. Source authority changes
require explicitly reviewed grants for the new version. These version values
are cursor/context identities, not substitutes for checking current permission.

Every request checks tenant/actor status, credential hash/expiry/revocation,
project membership and client-specific grants inside the same repeatable-read
transaction as the query. Shared row locks make a concurrent revocation take
effect before or after that transaction. A request that encounters a conflicting
snapshot may return `Unavailable`; it cannot return data using the stale grant.
Authorization is checked again on the next request. There is no authorization
cache or reusable execution permission in the returned context.

## Query protocol

POST JSON to `/v1/projects/<alias>/query` with an
`Authorization: Bearer <credential>` header. Version and operation are required:

```json
{"protocol_version":1,"op":"capabilities"}
```

The response advertises the supported queries and work commands.
The operation list describes implemented protocol, not a grant to invoke it.
Unsupported operations or protocol versions fail explicitly.

The response also carries `semantic_layers`. For each of the five result layers of AWR (`work_readiness`,
`execution_admission`, `context_completeness`, `delivery_observation`, `completion_validity`) it lists the Team queries
and commands that carry the layer and, in `without_team_operation`, the entries of the personal CLI and MCP server that
have no Team operation, each with its reason (for example `context compile`: no standalone context pack, `work.prepare`
returns the completeness and the context hash). A host that works in both modes can learn the gaps before asking;
asking anyway is refused as `Unsupported`. The lists name operations of this protocol and are checked against it.

| Operation | Selectors and result |
| --- | --- |
| `workstreams.list` | Authorized workstreams only; `limit`, `cursor` |
| `work.list` | Selected workstream's work summaries and count; `limit`, `cursor` |
| `work.graph` | Selected workstream's bounded progress nodes, current leases and readable dependencies; `limit`, `cursor` |
| `work.search` | Same visibility boundary; required literal substring `search` |
| `planning.candidates` | Readable planning candidates and server-computed differences from the current plan; optional `workstream_id`, `limit`, `cursor`, `max_context_bytes` |
| `sources.inspect` | Required document integrity in the active snapshot; optional `work_id`, `workstream_id`, `limit`, `cursor`, `max_context_bytes`; no document bodies |
| `documents.list` | Project document discovery across authorized workstreams and exact delegated tasks; optional `work_id`, `workstream_id`, literal case-insensitive `search`, `limit`, `cursor`, `max_context_bytes`; verified display metadata, availability and transaction `as_of` |
| `decisions.list` | Authorized ordinary questions and recorded answers, with optional `work_id`, `workstream_id`, exact `decision_id`, `decision_state`, `limit`, `cursor`, `max_context_bytes`. Defaults to open questions and includes transaction `as_of`. |
| `updates.list` | Current project period updates, newest period first; optional `limit`, `cursor`, `max_context_bytes`; no work/session selector. |
| `update.inspect` | Required `update_id`; one current or retained update with its author, record time, status, exact sentence, fact-snapshot reference and visibility-projected task facts with coverage. |
| `review.rounds` | Read-only ordered review history for a required work/session selection with bounded `limit`, `cursor`, `max_context_bytes` and transaction `as_of` |
| `handoff.list` | Read-only authorized handoff discovery with optional `work_id`, `workstream_id`, `recipient_person_id`, `handoff_state`, `limit`, `cursor`, `max_context_bytes` |
| `dependencies.list` | Read-only authorized dependency edges with required, available, adopted and consumed versions; optional `work_id`, `workstream_id`, `limit`, `cursor`, `max_context_bytes` |
| `work.prepare` | Required `work_id` or `session_id`; optional `max_context_bytes`; returns published contract, required_specs and authorized_readable_refs (TMCP-023) |
| `work.admission` | Required `work_id`; whether the work is governed, its admission state and diagnostics, the scope a decision will be checked against and its `binding_fingerprint`; see [Team governance](team-governance.md) |
| `work.observe` | Required `work_id` or `session_id`; returns bounded session/checkpoint, claim lease, execution and PR delivery observations under current WorkRead authority |
| `events.list` | Selected workstream, optionally narrowed by work/session; metadata only |
| `session.inspect` | Required `session_id`; its current-ownership checkpoint |
| `work.recovery` | Required work/session; up to two current-ownership recovery candidates |
| `command.inspect` | Required work/session and `request_id`; this actor/client's committed receipt or unknown outcome |
| `claim.inspect` | Required work/session and `claim_id`; current lease state, ownership, fence and epoch validity |
| `execution.inspect` | Required work/session and `execution_id`; current intent state/version, cancellation and contract/epoch validity |
| `source.content` | Required relative `source_path` on the **active** snapshot; optional `work_id`, `expected_sha256`, `max_context_bytes`. Rejects `..`, absolute paths, URLs and history selectors (TMCP-023). |
| `artifact.content` | Required `work_id` + `artifact_id` bound to that work's evidence; optional digest/budget (TMCP-023). |
| `planning.outcome` | Required `request_id`; planning mutation receipt or unknown (TMCP-023). |

Optional [session feedback](team-session-feedback.md) records declared client
metadata and nonterminal progress in existing checkpoint calls. `work.observe`
returns independently timed summaries and host usage snapshots, with explicit
missing reasons; it never infers token costs or settles execution from progress.

`work.list` and `work.search` add a versioned `summary` to every item. Its
`owner`, `phase`, `runtime`, `outcome`, `blocker`, `next_action` and `report`
fields each contain `value`, `as_of_unix_ms` and `reason`. A missing value is
explicitly null, with a reason such as `unassigned`, `no_session`,
`client_capability_unknown` or `no_reported_blocker`; it is never guessed from
the task title. A populated field has an as-of time and a null reason.

The owner includes the recorded `person_id` and `display_name`. Phase and next
action come from the latest actual progress report, even when a newer ordinary
checkpoint contains no progress. Their time is the report's receive time.
Runtime and responsibility are read as of the current transaction. Outcome
retains the reported summary and completed work alongside the selected
integration's state and `current_binding`. Only a rejection of the current
selection becomes an integration blocker. Report metadata retains attribution,
contract currency and staleness; no claim of independent validation is added.

List summaries use the same report and integration selectors as `work.snapshot`
under the caller's current WorkRead scope. One page performs four batched data
queries, regardless of its item count, in addition to existing authorization
and transaction checks. Pagination, search and visibility remain unchanged.
These fields are read-only observations; they grant no execution or approval.

`planning.candidates` returns proposed changes, their author and receive times,
the original baseline, and recorded approval, publication and source activation.
The baseline's `source_manifest_digest` identifies the source content used to
prepare the proposal. `matches_current` compares that digest and the authority
epoch with the active source. The response envelope's `source_snapshot_id`
identifies the current snapshot record.
Its `current_diff` groups work as added, removed from the active plan, changed or
unchanged. Field differences compare the proposal with the active source
snapshot, rather than trusting its historical before-image. The browser does
not calculate these differences. Changes include dependency acceptance and
execution settlement policies. An omitted optional field retains its current
value. Unreadable dependency values and acceptance maps are null with
`permission_restricted`; identifiers and titles from hidden work are not exposed.

Only candidates whose affected work is fully readable are enumerated. This
filter precedes counts, ordering and pagination. A workstream selector restricts
which candidates match without expanding the reader's scope. Cursors bind the
read authority, source snapshot and visible planning-record fingerprint, so a
changed draft or decision requires a fresh first page. Results include an
as-of record, `total`, `next_cursor` and `truncated`. The full response defaults
to a 256 KiB bound; a smaller requested budget is honored. Oversized results
return `ResponseTooLarge` rather than silently omitting differences.

Approval and publication must match the candidate's exact digest and draft
revision. A published proposal can still have source writeback pending; only
an activation record reports that it was applied. Missing records are explicit
nulls with reasons. The service records drafting, approved, published, superseded
and rejected candidate states. An explicit `planning.reject` decision records
the authenticated reviewer, reason and database decision time. `rejection` is
null with `not_rejected` when there is no such decision. A refused approval
request never produces a rejected-plan record. The bridge exposes
the same scoped read at `GET /api/team/plans?project=PROJECT`, with optional
`workstream_id`, `limit`, `cursor` and `max_context_bytes`; no planning commands
are exposed by that route.

`handoff.list` discovers metadata from the existing `execution` and
`responsibility` handoff records. It defaults to `handoff_state:pending`.
Use `history` for terminal or elapsed proposals and `all` for both groups.
An optional `recipient_person_id` filters the recorded receiving person.
Project discovery uses the same current workstream and exact-task read scopes
as `work.next`. A work or recipient filter cannot widen them. A session selector
is unsupported. Selecting a work requires its current read authority.

The standard envelope contains `data.work_id`, `recipient_person_id`,
`handoff_state`, `items`, `total`, `next_cursor`, `truncated` and `as_of`.
Each item includes its handoff and work identities, work title, kind, version,
recorded `status`, sender and recipient with available display names, and
creation, update and optional expiration times. `pending`, `expired`, `group`
and `effective_status` separate waiting work from history. The recorded states
remain `proposed`, `inspected`, `accepted`, `rejected`, `cancelled` or `timed_out`.
A proposal past its deadline has `effective_status:expired` and `pending:false`
while retaining its recorded status. The observation uses the transaction clock
and does not perform a timeout transition. Accepted or cancelled records do not
become expired merely because their former deadline has passed.

The default page has 20 rows and the maximum is 100, ordered by creation time
and identifier. The serialized response budget defaults to 262,144 bytes.
`handoff_position` is contiguous within the selected visible collection.
`total` counts authorized matches. `count_scope:authorized_matches` and
`count_limited:true` explicitly avoid promising a project-wide count.
`hidden_count` remains null. Neither hidden handoff existence nor its count is
disclosed. Packages, checkpoint content, raw bodies and acceptance material
are absent from discovery.

`as_of` binds the transaction read time, source snapshot, project revision
and visible handoff-history fingerprint. Cursors additionally bind current
authority, grant versions and request filters. Changes to readable records,
recipient filters, permission scope or relevant deadlines require a fresh page.
The Observer bridge exposes `GET /api/team/handoffs` with `project` and the same
selection, paging and budget fields. It checks attribution, expiry semantics,
conservative counts, metadata-only rows and complete page coverage.
Product pages use localized descriptions for these observations.
Reads never inspect a package for consumption, accept a transfer, cancel it,
start execution or alter responsibility.

`review.rounds` discovers review history for one readable work. Its standard
envelope contains `data.work_id`, `items`, `total`, `next_cursor`, `truncated`
and `as_of`. The default page has 20 rows and the maximum is 100. The response
budget, including the envelope, defaults to 262,144 bytes. Reads cannot grant
execution or approve a review.
Task-scoped Agent delegations require the exact `work_id` and `workstream_id`
alongside a session selector. A session selector alone cannot widen that grant.

Each readable row includes `round_id`, historical `round_index`, contiguous
`round_position`, recorded `state`, `bundle_hash`, `contract_hash`,
`contract_matches_current` and `artifact_digest`. These bindings use the same
stored round as `review.inspect`. Approved, returned and invalidated rounds
remain in history. State is one of `open`, `approved`, `rejected` or `invalidated`.
The latest decision matching that work and bundle supplies `decision`,
`decided_at_unix_ms`, reviewer attribution and `reason_first_line`. Ties use the
decision identifier after the recorded time, matching inspection order.

`reviewer_role` describes the recorded approval basis as `independent_reviewer`,
`author_reviewer`, `agent_reviewer` or `simulated_member_reviewer`.
It does not infer a historical role from current membership. Unknown historical
roles and absent decisions remain null with `missing` reasons. A round whose
bound evidence belongs outside the selected work stays counted as a placeholder
with `readable:false` and `reason:not_in_review_work_scope`. Only its round
identifier, index and position remain visible. The other summary fields are
null with the same missing reason. Unbound legacy rounds remain readable with
their recorded metadata and explicit gaps.

`as_of` records the transaction read time, active source snapshot, project
revision and a visible-history fingerprint. Cursors bind that history, work,
snapshot, epoch and current authority. A changed visible round or matching
decision expires the cursor. Hidden body values do not enter the fingerprint.
The Observer bridge exposes this through `GET /api/team/reviews` with `project`
and the same selection, paging and budget fields. It validates exact attribution,
missing-field semantics and complete positional pages. Internal metadata stays
in the service contract and backend records. Product pages use localized
business descriptions.

`sources.inspect` checks the required documents declared by readable workstreams.
An optional `work_id` selects that work's workstream; a supplied `workstream_id`
must agree. With neither selector it covers the caller's readable workstreams,
including current Agent delegation restrictions. It does not scan a developer's
filesystem or claim that unpublished local changes have been checked.

For example, `{"protocol_version":1,"op":"sources.inspect","work_id":"WORK","limit":20}`
returns the standard envelope with `data.items`, `total`, `next_cursor`,
`truncated`, `as_of` and `collection`. Each item contains its readable `path`,
`expected_sha256`, rehashed `current_sha256`, `checked_at_unix_ms`,
`source_position` and `status`: `consistent`, `changed`, `missing`, `forbidden`
or `unverified`. Missing metadata remains null with an explicit `missing`
reason; it never becomes a successful check. No document bodies are returned.
Forbidden documents expose neither paths nor digests.

`documents.list` discovers the unique documents registered in readable workstreams'
`acceptance_contracts`. A work selection restricts the catalog to that work's
workstream, using the same document authorization and metadata as the project read.
Unregistered files are excluded. Documents shared with another workstream require
read access to every referencing workstream before any metadata is disclosed. Selector-free
task grants contribute only the acceptance documents of their current task owners.
The response uses `authorized_works` when exact task scopes participate. Task
scope remains bound to the current authorized work and its source requirements.

Each row includes `document_position`, `path`, `name`, `summary`, `summary_kind`,
`sha256`, `byte_length`, `source_kind`, `readable`, `status` and `reason`.
`source_kind` is `registered_source`. `summary_kind` is `source_excerpt`, derived
from the first Markdown section after the stored source bytes and digest agree.
Titles and excerpts are bounded to 256 and 512 Unicode characters. They are display
metadata, without generated interpretation or a separate document body.

Availability is `available`, `missing`, `restricted`, `changed`, `unverified` or
`too_large`. Restricted rows disclose neither path, title, excerpt, digest nor size.
Missing or unverifiable content has no title, excerpt, digest or size. Only
`available` rows are readable. Documents above the existing 262,144-byte
`source.content` limit retain verified metadata and use `too_large`.
The browser requests bodies only through `source.content` with the listed digest,
which rechecks current access and source content independently. Rows can include `content_work_id` when the
controlled read needs an exact task selection. Carry that value as `work_id` in
`source.content`, alongside the registered path and expected digest. The catalog
uses the same selected read authority before advertising content availability.
For each source, one live delegation must cover every referencing workstream.
Independent grants can expose separate documents, while a shared document requires
one covering grant. Grant creation order does not change content availability.
Pages scoped to `authorized_workstreams` carry no task content selector.
Restricted rows retain no content selector. Revocation and ownership changes are
rechecked on every read and invalidate earlier cursors.

Search matches the authorized title, excerpt or path. Pagination uses opaque
cursors bound to the current identity, permissions, source, selection, search and
document facts. Changed inputs or authority invalidate the cursor. The response
includes the scoped count, `next_cursor`, `truncated`, `read_only: true`,
`execution_authorized: false` and transaction `as_of`. The Observer read bridge
validates page coverage, scope, redaction and availability before forwarding it.
The project knowledge page uses this catalog when advertised by capabilities.
Older services retain the existing work-reference view.
Catalog and whole-source browser requests explicitly use the supported 262,144-byte
budget. Source-derived titles pass a dedicated document-name presentation check.
Raw source excerpts remain service metadata until the user opens the document body.
An expired cursor immediately hides the retained page, clears the project's cached
catalog and document bodies, and reloads the first authorized page.
Shared catalog access is derived from the activated workstream catalog before
source bytes or availability are inspected. Removing a private workstream from
stored text, malformed text or missing text cannot reduce that access boundary.
Authorized readers still receive the appropriate integrity or missing-data state.

`collection.complete` covers the entire required set before pagination.
Its `reasons` includes all incomplete conditions; its `recovery` contains
read-only suggestions for the current page, linked by `source_position`.
Existing reasons include `required_spec_missing`, `required_spec_forbidden`,
`spec_digest_mismatch` and `spec_path_rejected`. Unrecognized reasons remain
explicit with `recognized:false` and a null `next_action`; no repair is executed.
The verification basis is `active_source_snapshot`, with a transaction read time.
An empty required set is complete but does not prove any documents were checked.

Cursors bind current read authority, snapshot and observed source integrity.
A changed binding returns `CursorExpired`; malformed selectors return
`InvalidInput`, unauthorized selection returns `Forbidden`, and a response
exceeding the requested bound (default 256 KiB) returns `ResponseTooLarge`.
Restart pagination after a cursor expires. The bridge exposes the same read at
`GET /api/team/sources?project=PROJECT`, accepting only `work_id`,
`workstream_id`, `limit`, `cursor` and `max_context_bytes` in addition to `project`.
It forwards the member's existing browser session and never falls back to
fixtures after a failed or revoked live read. Selected work and workstream
identities must be echoed exactly. The bridge rejects incomplete pages,
contradictory readiness, mismatched observation versions and forbidden metadata
disclosures with `BadGateway`, without forwarding the rejected payload.
Internal digests and reason codes
belong to backend diagnostics; product views translate them into document
readiness and recovery guidance without rendering technical fields.

Work/session selectors derive the workstream. An explicit `workstream_id` must
agree with them. Without work/session, a unique authorized workstream can be
selected; multiple grants require an explicit choice. Discovery/capabilities do
not take selectors. Unknown JSON fields and fields inappropriate for an operation
are rejected. Hidden and missing work/session IDs both return `Forbidden`.

Filtering precedes search, counts, ordering and pagination. Cursors bind the
credential/membership, selected authority and grant version, source snapshot,
operation and filters. Copying a cursor to another client does not transfer
permission. Event payloads and unattributed project events are withheld; work
events must agree with current ownership. Team ownership movement is not enabled
by this endpoint.

`work.prepare` includes the verified full contract hash and a `visible_contract`.
The latter omits cross-workstream dependency IDs until authorized export is
implemented, sets `dependency_export_unavailable` and marks context incomplete.
Its serialized content is therefore not the preimage of the full contract hash.
Required visible contract text is never shortened to fit a budget. A too-small
budget returns `ContextIncomplete` without a partial context.

The `awr-team-workstream-context-v1` semantic hash binds the selected contract,
visible facts, runtime state, ownership generation, workstream authority, reader,
grant version, coordinator epoch and project status. Unrelated project audit
revisions or source snapshot IDs remain in the envelope, outside that hash.
Absent runtime state is `null`, not an invented completion status. Preparation
always reports `execution_admission: "not_evaluated"`; context completeness and
a matching hash never authorize execution.

`work.observe` is a separate, read-only projection and does not change the
prepared context or its hash. Within a repeatable-read snapshot, it selects a
session from the current workstream and ownership generation, preferring a live
claim, then active sessions and the most recent checkpoint. A newer, closed
supervisor session cannot hide an ongoing worker's progress. It reports the checkpoint's contract currency,
claim expiry, latest execution, up to five registered PR deliveries and the time
of observation. Execution receipt payloads keep `execution.inspect`'s existing
same-client or reconciliation-authority restrictions. A recorded execution state
is not a live process heartbeat. Responsibility and claimant identity remain
separate; native conversation IDs are withheld, and unreported model and usage
are explicit nulls. Observation never authorizes execution, resumes a session,
or accepts a delivery.

The Observer bridge reads these observations for requested work. Its
optional public GitHub lookup follows only an exact PR reference in the
authorized response, checks the current head, and caches results for at least five
minutes. The interval grows with the observed PR count to fit the shared anonymous
request budget; the displayed observation timestamp makes its age explicit.
Anonymous API rate limits apply across lookups; reset and retry headers defer
further requests rather than retrying an exhausted quota every refresh.
A checkpoint mention is labeled separately from a registered delivery, and
GitHub checks do not constitute AWR acceptance.

Recovery reports each checkpoint's `contract_matches_current` as true, false,
or null when there is no checkpoint. Matching the contract alone does not prove
current dependencies, claims or authority; `automatic_resume` stays false.
Checkpoints from different ownership generations are not recovery candidates.

## MCP clients

For the member setup and development loop, see the
[Agent-first Team workflow](../integrations/team-agent-workflow.md). Members can
refresh, claim and update work directly through their Agent; browser sign-in and
browser task selection are optional.

The same listener serves Streamable HTTP MCP at
`/v1/projects/<alias>/mcp`. Configure the client's remote MCP connection with
that URL and its bearer credential in the Authorization header. Do not put
credentials in URLs or tool arguments. Each URL selects one registered project;
many clients and project endpoints share the same process and database pool.
No process is started for an individual connection or work session.

The transport uses the repository's RMCP SDK for protocol negotiation,
initialization and tool dispatch. It is stateless, including for older supported
MCP protocol versions: no `Mcp-Session-Id` carries permissions or selects work.
It exposes query/command tools plus access and planning tools, with arguments
identical to the corresponding HTTP JSON:

- `awr_team_query`: the query operations above. Start with
  `{"protocol_version":1,"op":"capabilities"}`. Includes controlled
  `source.content` / `artifact.content` and `planning.outcome`.
- `awr_team_command`: durable session/claim/execution/review commands below, with the same request
  identity and preconditions. Tool discovery is not a write grant.
- `awr_team_access_*`: project-admin member/role/credential management (TMCP-012).
- `awr_team_planning_suggest` / `_draft` / `_preview` / `_approve` / `_publish` /
  `_outcome`: authenticated planning ops (TMCP-023) calling the same business
  entrypoints as HTTP. No SQL tools, arbitrary file edit, or direct `done`.
  Stable `request_id` receipts; on disconnect call `_outcome` / `planning.outcome`
  before any new ID.

Initialization, discovery and notifications require current project/workstream
read access. Each tool call additionally checks authorization inside the same
transaction as its selected operation; initialization never caches authority.
Tools return structured content and a text fallback. Domain failures set
`isError: true` and use the same sanitized `code` as HTTP. Authentication,
transport limits and malformed MCP messages can fail at the HTTP/protocol layer
before a tool result exists. Legacy personal tools and unimplemented execution
operations cannot bypass this boundary.

Closing or reconnecting an MCP connection does not end a durable work session.
HTTP and MCP share command identities and receipts: a command submitted through
one transport can be inspected or exactly replayed through the other. A timeout,
disconnection or oversized response leaves the command outcome uncertain until
`command.inspect` returns its committed receipt. An absent receipt is still
`unknown`, not proof of non-execution. Planning mutations follow the same rule
via `awr_team_planning_outcome` / `planning.outcome`.

## Planning operations (TMCP-023)

HTTP routes under `/v1/projects/<alias>/planning/*` and the matching MCP tools
call SourceStore planning entrypoints only:

| HTTP / MCP | TMCP-010 action | Notes |
| --- | --- | --- |
| `POST .../planning/suggest` / `awr_team_planning_suggest` | `planning.propose` | Suggestion is not claimable and does not add formal work. |
| `POST .../planning/draft` / `awr_team_planning_draft` | `planning.edit_draft` | `mode=create|edit`; split/cancel/archive are `changes[].op`. |
| `POST .../planning/preview` / `awr_team_planning_preview` | read (propose/edit/approve/publish/work.read) | Exact diff + impact; no mutation. |
| `POST .../planning/approve` / `awr_team_planning_approve` | `planning.approve` | Bound to current `candidate_digest`. |
| `POST .../planning/reject` / `awr_team_planning_reject` | `planning.approve` | Closes the exact drafting candidate with a recorded reason. |
| `POST .../planning/publish` / `awr_team_planning_publish` | `planning.publish` | Optional `activate` uses the **registered** sole source only; client paths/URLs are refused. |
| `POST .../planning/outcome` / `awr_team_planning_outcome` / query `planning.outcome` | work.read or planning.* | Original `request_id`; scoped Agents with `planning.propose` recover only their own suggestion under one live covering delegation. Reserved commands stay unknown. |

Unsupported adapters (for example private management repo writeback), publish
blocked by in-flight claims, and missing context return sanitized `code` /
`message` / `next_step` on both HTTP and MCP. Members, planners and admins use
these surfaces for daily planning — not hand-edited JSON or SQL.

Rejection requires `protocol_version`, a stable `request_id`, `candidate_id`,
the current `candidate_digest`, and a nonblank `reason` of at most 4,096 UTF-8
bytes without NUL. The optional `author_person_id` remains a compatibility
assertion that must match the authenticated actor. Decision authority and full
candidate visibility are rechecked for every request, including replay.

Only drafting candidates can be rejected. A rejected candidate is terminal and
keeps its original content, version and decision. Create a new candidate to
propose a revision. A stale source baseline can be rejected because the decision
leaves the effective source and work unchanged. A stale candidate digest is
refused. Concurrent approval and rejection can produce only one decision.

After a disconnect, inspect `planning.outcome` using the original request ID,
then replay the exact request when instructed. Recovery reuses the committed
decision and its original time. Changed content under the same request returns
`IdempotencyConflict`. Invalid fields or a terminal candidate return
`InvalidInput`; missing current authority or visibility returns `Forbidden`.
Responses and errors follow the same contract through HTTP and MCP.

### Controlled context and content reads

`work.prepare` returns the published contract, `required_specs` (authorized
active-snapshot documents listed in the selected workstream’s `acceptance_contracts`,
with size, digest and read-authorization checks) and
`authorized_readable_refs`. Code `scope_paths` remain write boundaries and may
include directories or files that have not been created yet. They are not required
input documents. Missing declared acceptance documents still block complete context.
Dedicated queries:

- `source.content` — active snapshot path only; rejects `..`, absolute paths,
  URLs, and historical snapshot selectors.
- `artifact.content` — artifact bytes bound to evidence for an authorized
  `work_id`; metadata-only artifacts are refused.

Capabilities advertise `artifact_content` / `source_content` / `planning_mcp`.

## Project period updates (schema 65)

A supervisor records a project week or month through the dedicated MCP tool
`awr_team_update_record` (`update.record`). This is deliberately not a generic
`awr_team_command` operation: that envelope requires a task, contract and
workstream ownership anchor, while a period update belongs to the project. The
read operations remain ordinary `awr_team_query` / HTTP query operations.

The caller supplies the complete judgment. The service performs deterministic
validation, attribution, versioning, task-fact calculation and receipt recovery;
it does not call a model, generate a sentence, trim the sentence or rewrite it.
The six fact rows, exact sources, deduplication and half-open period window are
defined in [Project period-update fact snapshots](project-update-facts.md).
The window uses `projects.settings_json.time_zone`, defaulting to UTC when absent.

```json
{
  "protocol_version": 1,
  "request_id": "stable-request-for-this-intent",
  "period_kind": "week",
  "period_key": "2026-W47",
  "starts_on": "2026-11-16",
  "ends_on": "2026-11-22",
  "status": "risk",
  "summary": "The responsible supervisor wrote this exact sentence."
}
```

`period_kind` is `week` or `month`. Week keys use `YYYY-Www` (01–53), month keys
use `YYYY-MM`, and dates are real `YYYY-MM-DD` dates with `ends_on >= starts_on`.
`status` is `ok` (normal), `risk` (at risk) or `off` (off track). After trimming
only for the emptiness check, `summary` must be nonempty, at most 200 Unicode
characters and contain no control characters. The stored and returned sentence
retains the caller's original leading and trailing whitespace. Unknown fields,
including a caller-supplied fact snapshot, are refused as `InvalidInput`.

Recording requires all of the following, evaluated again for every call:

- the authenticated member's explicit `business_roles` contains `supervisor`;
- existing membership and workstream write authority satisfies the same
  `planning.edit_draft` action tier, including write access to every active
  workstream for a project-scoped Agent;
- one live project-scoped `edit_planning` delegation covers the Agent and its
  active responsible-person binding.

A workstream/task delegation cannot become project authority. A maintainer
without the explicit supervisor duty, an Agent without the covering project
delegation, a read-only member and a request through another tenant/project
binding are all `Forbidden`. Validation and authorization happen before the
update, operation or event insert; every such refusal leaves all three absent.

The first record for a project/kind/key has version `1`. Recording that period
again inserts the next version and advances a separate current pointer.
`project_period_updates` rejects update/delete through its history trigger, so
an earlier sentence, author, timestamp, version and fact snapshot remain
unchanged. The current pointer and supersedes reference use composite foreign
keys to prevent crossing periods. Both tables have forced project/tenant RLS.

An exact `request_id` and payload replay returns the original receipt with
`replayed=true`, does not recalculate facts and does not insert another version.
Reusing that ID with changed content returns `IdempotencyConflict`. The receipt
uses protocol `awr-team-project-update-v1` and includes the update ID, period,
status, exact summary, author, record time, version, supersedes ID, current
project revision and non-null `fact_snapshot_ref`. The facts and resolved window
are inserted in the same immutable history row. Rows recorded by schema 64 retain
a null reference; they are not backfilled as though they had been observed at the
time. Changed task data enter facts only through a newly recorded version.

### Read and sample contract

`updates.list` selects the current version of each period in the URL-bound
project, ordered by `starts_on`, `ends_on`, `period_key` and update ID descending.
It accepts `limit` (1–100, default 20), a keyset `cursor` and an optional response
budget. The cursor binds reader identity, live delegation, source catalog and
grant versions; an authority change returns `CursorExpired` instead of mixing
scopes. Newer periods cannot displace or duplicate an already returned keyset
entry. `data.items[]` contains `update_id`, `period_kind`, `period_key`,
`starts_on`, `ends_on`, `status`, `summary`, `author.person_id`,
`author.display_name`, `recorded_at_unix_ms`, decimal-string `version`,
`supersedes_update_id`, `fact_snapshot_ref` and `current=true`.

`update.inspect` requires `update_id` and returns the same record under
`data.update`; `current` is false for a retained version. It also returns
`data.fact_snapshot` and `data.fact_snapshot_coverage`. The stored project facts
are immutable; the returned facts are recounted through the reader's current
visible workstreams/tasks. `coverage.complete` is false when task IDs were hidden,
and `hidden_work_count` gives only their number. Unknown or foreign IDs return
`Forbidden`. Both reads run in the existing repeatable-read transaction, return
an `as_of` block and never mutate updates, operations or events. For an Agent,
they require a live project/workstream read delegation in addition to the
underlying workstream read grant. A task-only delegation cannot disclose a
project update. Read-only members may read within this boundary but cannot record.

Synthetic verbatim responses emitted by the actual read store are committed at
`crates/awr-team-pg/tests/samples/updates-list.json` and
`crates/awr-team-pg/tests/samples/update-inspect.json`. They are response-shape
fixtures for the later PPD-012 bridge, not hand-written demo shapes and not
proof of validation on a deployed service. AWR-PPD-090 performs that deployed
service verification. The recording and read integration tests are
`crates/awr-team-pg/tests/pg_project_updates.rs` and
`crates/awr-server/tests/project_updates_mcp.rs`.

## Ordinary decisions across sessions

An Agent with the current owned live claim can use `decision.request` to ask a
specific question and record its impact. Supply the owned `session_id`, current
`expected_session_version`, `claim_id`, `expected_fence`,
`expected_lease_version`, a stable `request_key`, `question`, `impact` and explicit
`allowed_responder_person_ids`. Person identities come from current project
membership or responsibility records. The selected people still need live
permission when responding.

The wait mechanism supports one open question per work, with execution,
claim and completion guards. The request binds the contract,
responsibility, workstream authority and verified required-source fingerprints.
Unrelated audit activity and unrelated source documents preserve that baseline.

Read `decisions.list` after reconnecting or losing a response. Reuse the same
business request key and content to recover its existing question across sessions.
The query supports `decision_state` values `open`, `replied`, `cancelled` and `all`.
`work.prepare` and `work.snapshot` include up to three selected questions in
`ordinary_decisions`, with a bounded follow-up query for history.

A selected responder uses their own active session with `decision.respond`.
Supply `session_id`, `expected_session_version`, the exact `decision_id`,
`expected_decision_version` from the read and `reply`. Concurrent responses have
one applied answer. A later caller receives `applied=false` and
`already_resolved=true` with the recorded outcome. Changed baselines refuse an
unapplied answer, and every request rechecks credential and scope authority.

`decision.cancel` uses the same fields and records the reason in `reply`.
The requester, selected responders, current task owner and an authorized
supervisor can withdraw an open question. This supports recovery after a goal,
responsibility or participant change. Versioned ordinary questions retain their
attributed resolution history.

Ordinary answers record project decisions. Execution admission uses
`execution.start`, formal review uses `review.decide`, and delivery acceptance
uses `work.complete` or `delivery.finalize`. Observation clients display the
question, impact, responsibility and confirmed result. Technical bindings belong
in service records.

## Durable session commands

POST to `/v1/projects/<alias>/command` with the same bearer authentication. The
following operations record a session journal and grant no execution rights:

| Operation | Strict `args` object |
| --- | --- |
| `session.start` | `conversation_id` |
| `session.checkpoint` | `session_id`, `expected_session_version`, `context_hash`, `next_action`, `open_loops` |
| `session.end` | `session_id`, `expected_session_version` |

Every command supplies `protocol_version: 1`, a stable `request_id`, `op`,
`workstream_id`, `work_id`, `coordinator_epoch`, `expected_project_revision`,
`expected_authority_version`, `expected_ownership_version`,
`expected_contract_hash`, and `args`. Read these preconditions from a current
`work.prepare` response; use its `authority_version` and the prepared data's
`ownership_version`. Version fields are canonical nonnegative decimal strings;
authority, ownership and session versions must be positive. Session versions
come from command receipts or `session.inspect`. Unknown envelope and argument
fields are rejected, including caller-provided actor/client/permission fields.

An active tenant/actor, membership other than `reader`, and a current explicit
`can_write` grant are all required. The server fixes session actor/client,
workstream and ownership generation from authenticated state. A grant to read
another session does not permit writing it: only its original actor/client can
save or close it. Starting another active session for the same actor, client,
conversation and work is rejected; resume the existing session or close it first.

Checkpoint saving recomputes the current scoped context hash in the write
transaction and rejects mismatches. The supplied context must have been consumed
by the caller; a matching hash proves correspondence, not that a model read it.
The checkpoint retains the observed revision and contract hash. Session versions
prevent stale updates even if a caller refreshes its project revision. Each
successful command commits state, scoped event, project revision and immutable
outcome receipt together. Failure rolls back all of them.

Ordinary commands validate a work-scoped read set (coordinator epoch, authority,
ownership, contract, and action tokens such as session/claim/fence/work versions).
The project revision remains an ordered audit cursor and is still returned on
receipts, but unrelated audit-cursor advances do not create semantic conflicts.
Writers still take the project admission lock for SQLite-compatible single-writer
serialization and a total audit order. Legacy clients may still send
`expected_project_revision`; the server accepts a well-formed decimal without
treating it as business CAS. Never automatically resubmit changed intent to
suppress true conflicts.

Paused/archived workstreams permit checkpoint preservation and session closure
with a still-valid write grant; they do not permit new sessions. A frozen,
importing or degraded project refuses new mutations. Closure also refuses active
claims, open waits or nonterminal/unknown execution on the work, and never treats
an ended session as proof that an external effect stopped. Execution admission,
claim transfer and resource release are not performed by these commands.

A native client exit or MCP disconnect is separate from logical session closure.
Ongoing work may save a checkpoint and retain its valid claim. When the caller
intentionally ends the logical session, it first reports and settles execution
effects and resolves any open wait. If this session holds an active claim, it
releases that claim with the current session, claim, fence and lease versions.
It then submits `session.end` with the current session version. This sequence
does not complete the work, approve a review, integrate a delivery or prove that
an external process stopped.

A known synchronous `session.end` refusal keeps HTTP 409 and the public
`RecoveryBlocked` code, and adds a bounded `rejection` object with
`operation: "session.end"`, `outcome: "known_rejection"`, one finite `reason`,
and `action_guidance` containing `when`, `because`, `action` and `recheck_on`.
Reasons are `UNRESOLVED_EFFECTS`, `WAIT_OPEN` and `ACTIVE_CLAIM`, in that
precedence when more than one condition applies. Guidance exposes no row counts,
peer identities or object values, performs no release, and grants no authority.

On timeout or an unconsumed response, query `command.inspect` with the original
`request_id` and work. It returns only this authenticated actor/client's receipt
under the current workstream and ownership boundary. A found receipt preserves
the original committed revision and result. Absence is `unknown`, because an
in-flight request might still commit. Retry only the **same request ID and exact
payload**: replay returns the original receipt without duplicating a session,
checkpoint or event; changing the payload returns `IdempotencyConflict`. Current
authorization and epoch/ownership still apply to replay. Already committed
receipts remain queryable during a freeze; replay performs no new mutation.
A returned `known_rejection` is not a missing or uncertain transport outcome:
follow its guidance and do not use `command.inspect` or repeat the unchanged
request merely because rejected commands have no committed operation receipt.

## Coordination claims

These commands use the same command envelope, live write authorization,
project revision, transaction and outcome protocol as session commands:

| Operation | Strict `args` object |
| --- | --- |
| `admission.decide` | `admission_decision` (`proposed`, `approved` or `denied`), `binding_fingerprint` and `expected_admission_version` (`admission_version`) from `work.admission`, and `admission_judgment`, `admission_reason` or `admission_proposal_id` as the decision needs; needs the planning approval permission; see [Team governance](team-governance.md) |
| `claim.acquire` | `session_id`, `expected_session_version`, `expected_work_version`, `ttl_seconds` |
| `claim.renew` | `session_id`, `expected_session_version`, `claim_id`, `expected_fence`, `expected_lease_version`, `ttl_seconds` |
| `claim.release` | `session_id`, `expected_session_version`, `claim_id`, `expected_fence`, `expected_lease_version` |

TTL is an integer from 1 to 3600 seconds. Version/fence fields are canonical
decimal strings; use work version `"0"` only when preparation reports no runtime
row. Fence and lease versions must be positive. The active session must belong
to the authenticated actor **and client**, selected work, current workstream and
ownership generation. Another client for the same actor cannot renew or release
the lease. Acquire and renew require an active stream and enabled work contract.
Release is also allowed for a paused/archived stream with current write access;
a frozen/importing/degraded project still refuses new mutations.

A claim records coordination ownership, not dependency admission, a resource
reservation or permission to perform effects. Capabilities advertise
`claim_semantics: "coordination_only"`, and receipts retain
`execution_authorized: false`. Acquiring a claim does not make a work item ready
or validate its upstream deliveries. Live competing claims, open waits, completed
work and unresolved executions/recovery block acquisition. Expiry cannot clear
unknown effects. A safely expired claim can be replaced atomically with a new
monotonic fence. An expired lease cannot be renewed.

Release requires the exact owner, epoch, current fence and lease version, and
resolved execution/recovery state. The owner may release an elapsed lease without
renewing it first. Release does not release resources; the receipt explicitly
reports `resource_release_performed: false`. Claimed work becomes `unclaimed`,
without inventing readiness.

A rejected `claim.acquire` keeps its existing error code and status and adds a
`rejection` object: `{reason, operation, authority, next_action, recheck}` where
`reason` is one of `ALREADY_CLAIMED`, `STALE_FENCE`, `EPOCH_CHANGED`, `WAIT_OPEN`,
`PRECONDITIONS_CHANGED` or `RECOVERY_BLOCKED`. A workstream, ownership, epoch or
fence mismatch keeps the top-level `code` `RecoveryBlocked` that callers already
branch on; the finer fact appears only in `rejection.reason`. The object is
diagnostic only: the rejected call writes nothing, advances no fence and no
epoch, and leaves claim rows, work version and project revision unchanged. It is
not a reservation, and no caller may use it to skip `claim.acquire` or its
`expected_work_version`.

A command that needs a live claim (`claim.renew`, `execution.prepare`,
`execution.start` and `delivery.candidate.select`) refuses a claim whose lease has
expired, or that was released, with HTTP 409 and the unchanged `code` `LeaseExpired`
and `message`. `delivery.candidate.select` answers a claim that a newer claim replaced
the same way; the other three answer it with `StaleFence`. The body adds
`reason: "claim_lease_expired"`, a fixed `next_step` and
`action_guidance: {condition, basis, action, recheck}`. They give the order of the
recovery:

| Step | Action |
| --- | --- |
| 1 | The claim cannot be renewed. Stop effects under it and do not repeat the request with it. |
| 2 | Read `session.inspect` (`session_version`) and `work.prepare` (`runtime.work_version` and the guidance for the next step). |
| 3 | Settle a running or unreported execution as that guidance says. Never replay an execution that already stopped; close one that was prepared but never started with `execution.cancel`. |
| 4 | Call `claim.acquire` with the current `expected_session_version` and `expected_work_version`. The session survives the expiry; a refusal carries its own `rejection.next_action`. |
| 5 | Continue only with the new claim's `claim_id`, fence and lease version. |

The text is fixed and never repeats a value of the request. HTTP, the Web entry and MCP
return it identically, the refused call writes nothing and leaves its `request_id`
unused, and it grants no authority: `claim.acquire` is still checked in full. Other
commands keep the bare `LeaseExpired` body, because this recovery does not fit them:
`delivery.source.write` and `delivery.source.confirm` answer it for an elapsed source
publication lease, `claim.release` for a claim that is already released, and
`execution.report` only when the claim lapses while a settlement is being recorded.
Every other refusal of these commands is unchanged.

Receipts are historical: `lease_state_basis: "at_commit"` describes the original
commit. Replaying an old acquire after expiry or release returns that same receipt
without creating or extending a lease. Use `claim.inspect` for current state:
`lease_live`, `owned_by_client`, `epoch_matches_current`, and `current_fence`.
Even a live lease is not execution permission. Missing/hidden/mismatched claims
return `Forbidden`; inspection never transfers ownership.

Schema 11 preserves legacy claim rows with null workstream/ownership/epoch
attribution. It does not infer ownership from today's source. Unattributed,
reassigned or old-epoch active rows require explicit recovery/migration before
replacement, even when their deadline has elapsed.

## Execution intents and cancellation

Execution intents persist a planned attempt under the same authenticated command
transaction. They are a prerequisite for the remaining execution lifecycle, not
permission to run a command. `execution.prepare` creates no outbox delivery and
returns `dispatched: false`, `admission: "not_evaluated"` and
`execution_authorized: false`. Explicit caller-managed start and observation
reporting, explicitly authorized executor attestations and operator reconciliation
are available. Dispatch is not exposed; capabilities distinguish these operations.

| Operation | Strict `args` object |
| --- | --- |
| `execution.prepare` | `session_id`, `expected_session_version`, `claim_id`, `expected_fence`, `expected_lease_version`, `expected_work_version`, `input_digest`, `declared_scope` |
| `execution.cancel` | `session_id`, `expected_session_version`, `execution_id`, `expected_execution_version` |

Preparation requires the active, exactly owned session and live claim, current
work version, active workstream and enabled contract. It refuses completed work,
open waits, recovery blocks, unfinished attempts and unknown resources. Each
intent captures immutable actor/client, session/claim, workstream ownership,
coordinator epoch, fence, contract and input digest. Another credential for a
different client cannot act as that executor, even when the actor is the same.
Preparation and cancellation advance work versions and atomically persist their
state, scoped event and command receipt.

`input_digest` is 64 lowercase hexadecimal characters identifying caller-held
input; recording it does not independently verify those input bytes.
`declared_scope` contains at most 128 unique, canonical, workspace-relative paths,
each at most 4096 bytes and contained by a contract scope path. Absolute paths,
parent/dot segments, repeated separators, backslashes, drive prefixes and control
characters are rejected. This is **lexical contract validation**, not filesystem
confinement, a resource reservation or a dependency check. The client cannot
self-select a trusted executor identity, receipt kind or stronger fencing class;
intents remain `uncontrolled` without an exactly-once claim.

The original client may cancel an intent after its claim deadline or while its
workstream is paused, provided it still owns the active session and has current
write access. A frozen project continues to reject new mutations. Cancellation
can synchronously mark an unexposed `prepared` intent as `cancelled`. Any outbox
delivery or execution receipt, or a queued/accepted/running/unknown state, instead
leaves the execution state intact and records `cancel_requested: true` with
`stop_confirmed: false`. No cancellation releases resource reservations or clears
a recovery block. A terminal execution cannot be rewritten through cancellation.

An execution receipt's `execution_state_basis: "at_commit"` is historical.
Replaying preparation after cancellation does not resurrect the attempt. Query
`execution.inspect` to distinguish current state, executor ownership, live lease,
contract currency and epoch currency; it never automatically resumes an attempt
or grants execution. Missing, hidden and mismatched execution IDs share the same
`Forbidden` response. Schema 12 leaves legacy execution attribution null rather
than assigning it from today's source. Such history needs explicit migration;
an old epoch can be inspected within its unchanged ownership but cannot be
cancelled under a new epoch without the recovery protocol.

## Caller-managed start and observations

`execution.start` takes `session_id`, `expected_session_version`, `execution_id`,
`expected_execution_version`, `claim_id`, `expected_fence`,
`expected_lease_version`, `expected_work_version` and
`execution_mode: "caller_managed"`. The optional `expected_input_digest` must
match the prepared input when provided. The bundled adapter uses
`reference_write_v1`, which requires that digest and explicit system attestation
authority at admission; other modes are rejected. The same
transaction checks current authority and ownership, the original prepared
contract, a live owned claim, open waits, recovery state and required completion
receipts. It reserves every declared path as a project-scoped lexical prefix and
moves the attempt to `running`. The admission receipt records its dependency
receipts and reservation identities. It creates no outbox and launches no process.
It also records the original input digest, declared scope and a conservative
remaining lease duration in milliseconds for a cooperating local adapter.

Only the **original successful response** has top-level
`execution_authorized: true`, permitting one caller-managed run under that lease.
The stored receipt always has `execution_authorized: false`; its
`admission: "granted_at_commit"` is a historical observation. Exact retries,
`command.inspect` and `execution.inspect` never issue fresh execution permission.
If the first response is lost, inspect and recover the attempt instead of starting
another external effect. A new request ID cannot restart an already running attempt.

A same-workstream required predecessor needs a selected completion receipt for
its current contract and completed runtime. A source status alone is insufficient.
Cross-workstream dependencies remain blocked until explicit export/adoption is
implemented, including when a client can read both workstreams. Admission rechecks adoption/bindings at prepare, dispatch and complete (WS-032)
so concurrent revoke cannot race past the boundary; mid-execution invalidation
keeps real effects and recovery duty. Fixed-delivery consumers are not
invalidated by unrelated upstream progress.

The resource check covers cooperating AWR clients in this project's lexical path
namespace. It does not inspect client filesystems, separate physical worktrees,
reserve undeclared external services, stop a process, or provide a hard fence.
The client must honor lease expiry and cancellation. `fencing_class` stays
`uncontrolled` and `exactly_once_supported` stays false. Full workspace/external
resource identity is a later protocol.

`execution.report` takes `session_id`, `expected_session_version`, `execution_id`,
`expected_execution_version`, `outcome` (`succeeded`, `failed`, `cancelled` or
`unknown`), `output_digest` (64 lowercase hex, required for `succeeded`),
`observed_paths` (at most 128 canonical paths) and a nonempty `note` (at most 4 KiB).
Only the original actor/client with current write authority may report. An elapsed
claim does not prevent recording an observation; current session, ownership and
epoch binding still apply. Reports may be recorded while the workstream is paused.

`observed_paths` includes every actual workspace effect, including baseline
imports; contribution paths relative to an imported target cannot replace it.
An optional closed `baseline_transition` records caller-asserted Git revisions,
operation and contribution paths for provenance only. It never changes the
scope check or releases resources. The full statement stays in the original
caller receipt; ordinary acknowledgements and verification expose only a compact
`baseline_transition_provenance` descriptor. Legacy fields remain absent when no
transition was supplied. See [actual effects and bounded baseline updates](team-execution-settlement.md#actual-effects-and-contribution-provenance)
for the exact input and separately reviewed baseline-task workflow.

These are **caller assertions**, not trusted result or stop confirmations. Each
report is kept verbatim in an attributed execution receipt, including observations
outside the declared scope and conflicting later reports. The execution becomes
`unknown`, recovery remains blocked and only reservations bound to that execution
by the persisted schema 13 execution identity become `unknown`; legacy/unbound or
other-execution reservations on the same work are preserved. Work is not completed
and resources are not released. An already terminal attempt cannot be rewritten.
Unknown fields such as `receipt_kind: "trusted_executor"` are rejected. A newer
current contract does not erase observations against the original execution
contract.

## Executor attestations and operator reconciliation

Two commands can settle actual execution effects. Neither completes work, selects
a completion receipt, upgrades its execution policy or grants permission to run
again. Use `execution.inspect` first, then fresh `work.prepare` preconditions.

| Operation | Strict `args` object |
| --- | --- |
| `execution.attest` | `session_id`, `expected_session_version`, `execution_id`, `expected_execution_version`, `facts` |
| `execution.reconcile` | The same fields plus `expected_work_version`, `reviewed_receipt_id` (the latest inspected receipt ID, or null when absent), `clear_recovery_block`, optional `previous_epoch_recovery` |

`facts` contains `outcome` (`succeeded`, `failed`, `cancelled`, `unknown`), the
original `input_digest`, optional `output_digest` (required for success),
`environment_digest`, `observed_paths` and `note`. Digests are 64 lowercase hex
characters. Paths and notes have the same bounds as `execution.report`. These
facts identify what the authorized reporter verified; the server does not inspect
the external process or independently hash its outputs.

An attestation requires an operator-provisioned `system` actor, effective write
access and the explicit `can_attest_execution` workstream grant. That authority
must exist **both at admission and when reporting**. Admission captures its grant
version and returns `result_authority: "trusted_executor"`; ordinary admission
returns `caller_asserted`. A later privilege change cannot upgrade an ordinary
or legacy attempt into trusted execution. Only the original actor/client/session
can attest, and the receipt kind is derived by the server as `trusted_executor`.
Caller strings cannot grant trust. An ordinary `execution.report` always remains
`caller_asserted`, even when submitted by a trusted executor.

Reconciliation requires an operator-provisioned `human` or `system` actor, admin
membership, effective write/manage access and explicit `can_reconcile_execution`.
An `agent` actor or admin membership alone is insufficient. The operator uses its
own current work-bound session and confirms the exact execution/work versions and
latest reviewed receipt. The new receipt is `reconcile`, never `trusted_executor`;
the original observation and executor attribution remain in history. Claim expiry
does not prevent settlement. Current workstream and ownership binding still apply.

`execution.inspect` reports `reconciliation_coverage` for its authorized
workstream. `covered` means at least one currently eligible actor, client,
credential and grant is configured; `insufficient` means an explicit recovery
grant and membership exist but no candidate currently satisfies the full authority
checks; `none` means no such candidate is identified in the workstream. This aggregate does not identify
members or disclose credentials. It cannot prove that the person or system still
holds its credential or is available to respond. A missing field on an older
service means this diagnosis is unsupported, not that the workstream lacks a
recovery operator.

If coverage is missing, a schema owner can inspect and provision the narrowly
scoped recovery grant through the [operator access flow](team-operator-access.md).
Ordinary project-admin HTTP/MCP access cannot grant this special authority.
Reinspect after configuration, credential, membership, grant or workstream
authority changes; reconciliation still requires a fresh authenticated session
and the current execution, work and receipt preconditions.

After a coordinator epoch change, ordinary attestation and reconciliation without
an explicit review reject the old execution. A current recovery operator may add:

```json
{
  "previous_epoch_recovery": {
    "execution_epoch": "the-original-execution-epoch",
    "executor_stopped": true,
    "review_reference": "operator-owned evidence reference"
  }
}
```

Use the exact `execution_coordinator_epoch` returned by `execution.inspect`.
The review reference must be nonempty, at most 2048 bytes and contain no control
characters. Settling a result requires `executor_stopped: true`; an unknown result
may record `false` but keeps its resources and recovery block. This is an
**authorized operator assertion**, recorded as
`recovery_review_basis: "authorized_operator_assertion"`. The service does not
independently verify process termination or a physical fence. Verify those facts
before asserting them. A review for a current-epoch execution is rejected as
stale preconditions, and `execution.attest`/`execution.report` reject this field.

The original execution epoch, actor/client, session and existing receipts remain
unchanged. The new reconciliation receipt records both execution and reporting
epochs, the review and the latest reviewed receipt. Missing historical attribution
or changed ownership cannot be inferred from current source data. Enabled-project
backup/restore and migration of unattributed history still require separate
protocols; this command does not implement either.

Terminal facts release only reservations bound to that execution. Unknown facts
retain reservations and block recovery. An executor that reports paths outside
its declared scope leaves the attempt unknown for operator review; an operator
cannot certify an out-of-scope success, but may reconcile failure or cancellation.
Existing terminal facts cannot be rewritten; an operator may confirm identical
facts to resolve an outstanding block.

Only reconciliation with `clear_recovery_block: true` may clear an existing work
block, and only when the result is settled and no other nonterminal execution or
reserved/unknown resource remains on the work. Clearing an unknown result is
rejected. Partial settlement keeps the remaining barrier and does not release
another attempt's resources. An executor cannot clear a work recovery block.

`execution.inspect` exposes current `attestation_authority` and
`reconciliation_authority`, plus `execution_coordinator_epoch`,
`previous_epoch_review_required` and `previous_epoch_recovery_available`.
These describe the review path, not permission to execute. Full `latest_receipt`
facts are visible only to the original actor/client or a currently authorized reconciliation operator; other
readers get metadata and `receipt_details_available: false`. Recheck after grant,
ownership, epoch, receipt or work-version changes. Exact retries return historical
command receipts and never repeat effects or issue execution permission.

Schema 13 gives existing grants neither new authority, leaves existing admissions
without attestation delegation, and retains legacy resource reservations as
unbound. It does not infer a resource's execution from today's work owner. Such
reservations cannot be released by these commands. Reporting or reconciling an
upgraded in-flight attempt preserves its unbound reservations and keeps recovery
blocked until an explicit history migration resolves them. Schema 14 adds the
operator provisioning CLI and its immutable receipts without granting existing
clients new rights. The [scoped reference runner](team-reference-runner.md)
integrates bounded local file writes and saved-fact reporting, but does not adopt
or backfill existing in-flight execution history. Generic agent dispatch and
a bounded ownership/work-inventory rebuild-from-manifest slice is available via
schema-owner `backup-rebuild-*` (fencing-quiet, empty-or-matching ownership only);
catalogs, contracts, receipts and grants remain outside that subset. Enabled-project
history still requires explicit migration before backup.


## Evidence, review, rework and completion (WS-018)

Mainline-enabled Team projects use authenticated workstream commands for the
review lifecycle. Legacy `ReviewStore` entrypoints still refuse enabled
projects; do not route enabled traffic through them.

| Command | Role |
|---|---|
| `evidence.submit` | Record evidence bound to the current contract (and optional execution). Agent self-report is `caller_asserted` and never equals trusted execution or human approval. |
| `delivery.evidence.submit` | Submit candidate-bound evidence with a typed `args.delivery_payload` object. Requires `delivery_candidate_digest`; optional `passed` and `output_digest` have declared types. Uses the same evidence storage and authority checks; submission does not prove currentness or approval. |
| `review.open` | Open a review round bound to exact `contract_hash`, artifact digest, execution result digest and round index. |
| `review.accept` / `review.return` | Human reviewer decision. Independence is by responsible **person**, not by a second agent of the same person. Personal self-review is allowed only when `completion_policy` is `trusted_execution_and_author_self_review`, and is labeled `personal_self_review` — never `team_independent`. |
| `work.rework` | Acknowledge a returned/rejected round. History is retained. |
| `work.complete` | Atomically validate runtime state, evidence↔execution binding, selected completion receipt and actual approver person. Distinguishes execution success, author self-report, human approval and task completion. When a delivery candidate is selected, the integration of that candidate must be confirmed for the current selection first (`IntegrationNotConfirmed` otherwise, with a `reason` naming the missing step). |

Queries: `evidence.inspect`, `review.inspect`, `completion.inspect`.

Completion receipts expose `independence_kind` and `team_independent_acceptance` for WS-030 adoption authorization. Provider-private session fields are never included (`provider_private_session` is always null).

## Hard delivery dependencies and adoption credentials (WS-030)

Store APIs (SQLite + Team PG) persist:

| Record | Role |
|---|---|
| `hard_delivery_dependencies` | Cross-stream hard edge bound to exact Work + contract + artifact + completion receipt + policy (`fixed_delivery` / `current_contract`). |
| `export_authorizations` | Auditable grant/deny/revoke of disclosure scope for a concrete delivery version. |
| `adoption_credentials` | Historical proof that a consumer adopted a verified delivery; fixed-delivery credentials keep the original selected version. |
| `delivery_credential_receipts` | Idempotent request-key receipts for register/revoke/grant/adopt. |

Author self-reported done (`author_self_report` / `personal_self_review` / non-`team_independent` completion) cannot produce an adoption credential and cannot unlock execution. Cross-project bindings are refused. Team PG tables use FORCE RLS with transaction-local `awr.tenant_id` / `awr.project_id` (schema 26).

Graph coordination (WS-031) keeps the same-project task DAG acyclic across
workstreams (`A1 → B1 → A2` legal), returns explainable hard cycle paths, applies
edge mutations atomically under the project lock (no cyclic union / dangling
refs), requires all necessary deps for readiness, and treats shared outcomes as
references consistent with adoption credentials above.

Selective invalidation (WS-032) re-evaluates only affected consumers under
`fixed_delivery` / `current_contract` policies, revalidates prepare/dispatch/
complete under the project lock (revoke-race safe; mid-execution keeps effects
+ recovery duty), and persists scoped planning changes for newly discovered
dependencies (block affected actions → authorized confirm of new graph +
acceptance contract). Team PG schema 27.

Artifact/contract changes invalidate prior open/approved rounds for other
bundles; reject/return/rework keep historical rounds.

## Dependency version observations

`dependencies.list` uses the current authenticated project scope. A reader may
narrow it to one work or workstream. Each page contains at most 100 edges,
ordered by consumer work and declared predecessor order. It carries contiguous
`dependency_position` values, `total`, `next_cursor`, and transaction `as_of`.
The cursor binds the current reader, grants, source snapshot and project revision.
A changed binding requires a fresh first page. Counts cover authorized consumers
and explicitly retain `count_limited=true` with `hidden_count=null`.

Each edge retains its consumer-owned predecessor reference. Upstream titles and
version metadata require current upstream read authority or a valid exact artifact
disclosure. Missing fields carry reasons such as `permission_restricted` and
`not_recorded`. The read returns no document bodies, export proof bundles or
execution payloads. It performs no adoption, claim, handoff or execution effects.

`required_version` describes the declared current-contract requirement or the
verified original fixed delivery. `available_version` describes the receipt that
currently satisfies that requirement. Same-workstream `adopted_version` comes
from the latest committed execution admission for the consumer's current contract.
Cross-workstream `adopted_version` comes from its selected, revalidated export.
`consumed_version` separately describes the inputs recorded at execution admission.
An unstarted consumer can therefore have an available result and null adopted
and consumed versions. A later adoption preserves the earlier execution inputs.

Fixed deliveries retain their original contract and source snapshot. Cross-stream
disclosures reuse the existing proof and artifact validation, including revocation.
Invalid or unreadable disclosure metadata becomes a reasoned null. Same-stream
availability reuses the exact current-receipt assurance gate used by admission.

`validity` is `valid`, `upstream_changed` or `unavailable`. A valid edge can still
have no recorded consumption. Reasons identify changed execution inputs, invalidated
bindings, unavailable completion, missing adoption, invalidated disclosure or
restricted access. The observation supplies coordination facts for a later Agent
decision. It grants no execution or completion authority.

`delivery.inspect` exposes the identical per-edge projection under
`data.delivery.dependencies`. Collection-only position and consumer-title fields
are added by `dependencies.list`. PostgreSQL and bridge fixtures compare every
shared field, including missing-value reasons. The Observer adapter exposes
`GET /api/team/dependencies` and preserves these fields through its bounded read
route. Product pages translate the states and reasons into their localized
progress and recovery vocabulary.

Dependency reads preserve separate current grants for consumer and upstream
objects, including task grants during workstream-filtered reads. Every selected
consumer still needs its own covering read grant. Delivery inspection uses the
same rule for work-selected reads. Command authorization continues to resolve
its own single covering grant.

Older supported services may omit the new dependency field in delivery
inspection. The adapter preserves their existing delivery facts. Callers treat
the absent observation as unknown and never convert it to an empty dependency
set. A present array receives full shape, binding and adoption-state validation.

## Limits and errors

Requests are limited to 64 KiB, pages to 100 items, search to 512 bytes, and
requested context to 256 KiB (default 64 KiB). Complete serialized responses
have a 1 MiB ceiling. Oversized responses fail without truncating obligations;
reduce the page or narrow the selector. An individual oversized checkpoint still
requires a trusted operator recovery path. The listener shares 64 concurrent
request permits across HTTP and MCP, with a 30-second timeout. Responses use
`Cache-Control: no-store`.
Commands share the 64 KiB request limit, permits and timeout. A checkpoint's next
action is limited to 8 KiB and its open loops to 32 entries of 4 KiB each, within
the whole-request limit. HTTP timeout is not proof of transaction failure; use
the outcome-query procedure above. MCP limits include its entire JSON-RPC
request and response envelopes, including the text fallback; a result that fits
in HTTP JSON may need a smaller MCP page. Required text is never truncated.
MCP handlers retain their request permit and deadline even if the receiver
disconnects.

| HTTP status | Meaning |
| --- | --- |
| 400 | Invalid JSON, selectors or bounds |
| 403 | Missing/invalid credential, denied scope, or hidden/missing object |
| 409 | Scope/cursor/context limits, unavailable required dependency receipt, resource conflict, declared scope outside contract, stale preconditions/fence, held/expired lease, open wait, changed epoch, idempotency conflict, project barrier or unresolved recovery |
| 413 | Request body too large |
| 501 | Unsupported operation or protocol |
| 503 | Busy, timed out, transient transaction conflict or unavailable data |

Public errors do not include SQL, connection strings, bearer tokens or source
bodies. Database/file access outside the service remains an operator privilege;
HTTP authorization does not establish an OS sandbox.

Real PostgreSQL tests cover client/tenant separation, cursor binding, ownership,
dynamic revocation and both orders of concurrent credential revocation. HTTP
tests use a real loopback listener and PostgreSQL with simultaneous clients.
Session command checks cover owned journaling, concurrent replay/conflicts,
write revocation while a command is waiting, full rollback after an event failure,
frozen/paused state and unknown-execution preservation. These are
protocol/integration checks, not native coding-client business acceptance. Real
RMCP clients also verify discovery, simultaneous scope isolation, live revocation,
reconnection, HTTP/MCP receipt parity and refusal of oversized envelopes. Claim
checks cover concurrent acquisition, renewal/release, client ownership, epoch and
fence changes, expiry, atomic rollback, migration preservation and historical
replay versus live inspection over HTTP/MCP. Execution-intent checks cover exact
client ownership, scope/version guards, preparation/cancellation rollback, live
revocation, unknown-effect preservation, legacy migration and shared HTTP/MCP
receipts without dispatch. Admission tests additionally cover live lease/contract/
wait/resource checks, required receipt coverage, one-time start responses, atomic
rollback and preservation of unverified reports. Recovery checks cover explicit
executor authority, operator settlement, receipt preservation, rollback and
explicit old-epoch review over PostgreSQL, HTTP and MCP. A local runner test also
installs a new generation barrier and rejects a delayed old-generation write;
the database boundary in that test is synthetic, not a physical backup/restore.
Generic agent dispatch remains unavailable through this surface. Bounded
owner-only history migration, active-claim/execution quarantine recovery,
explicit CHECK-safe execution attribution with a reviewed `executor_client_id`, and
enabled-project logical backup/fencing restore and bounded ownership rebuild are
separate schema-owner CLI flows
(`awr-server access history-*` / `quarantine-*` / `execution-attribution-*` / `backup-*`), not HTTP/MCP client
capabilities. `operator_surface_denial` (HTTP + MCP with real PG and provisioned
client bearers) asserts Unsupported/404 for those surfaces while schema-owner
CLI recovery-inspect/history-preview still succeed. Physical database basebackup
stays an external operator responsibility.
