# Durable provider-neutral delivery and source publication

The delivery coordination store persists the neutral records described in
[the delivery contract](pr-delivery-review.md). It does not require a GitHub
repository, a pull request, SHA-1 revisions or a webhook. An artifact delivery
and a local Git delivery use the same candidate and observation boundaries.

The observation path stores facts and notification intent. It does **not**
execute a repository operation, resolve review references as approvals or
accept a task. A separate authenticated publisher can write bounded delivery
references to the project's configured authoritative source. Development on
`main` exposes these operations through the existing Team HTTP/MCP entry points.
This does not establish deployed integration or native business acceptance.
Background scheduling is optional, explicitly configured server behavior as
described below. Repository adapters and neutral finalization remain separate
implementation layers.

## Authenticated HTTP/MCP operations

Use `awr_team_command` or `POST /v1/projects/{key}/command`. The existing
command header supplies the stable `request_id`, actual work/workstream, epoch,
authority and ownership versions, contract hash and project audit cursor.
Every neutral operation additionally requires `args.source_snapshot_id` from
the current preparation. Do not supply another `read_set`, request identity,
caller identity or filesystem path in `args`. The complete command must fit
65,536 bytes, including its header.

| Operation | Additional `args` |
| --- | --- |
| `delivery.connector.configure` | `expected_connector_version`, `mapping` (connector, provider, resource, subject actor/client, fact origin, enabled) |
| `delivery.candidate.select` | `expected_selected_digest` (or null), owned `session_id`, `claim_id`, `fence`, `lease_version`, typed `candidate` |
| `delivery.inspection.reserve` | `connector_id`, `connector_version`, `candidate_digest`, `lease_seconds` |
| `delivery.facts.ingest` | `connector_id`, `inspection_id`, `event_id`, typed `records` |
| `delivery.source.prepare` | `candidate_digest`, `expected_selection_version`, `expected_metadata_revision`, `expected_source_fingerprint`, optional `completion_receipt_id`, `lease_seconds` |
| `delivery.source.renew` | `publication_id`, `fence`, `lease_seconds` |
| `delivery.source.write` | `publication_id`, `fence` |
| `delivery.source.confirm` | `publication_id`, `fence` |
| `delivery.source.abandon` | `publication_id`, `fence` |
| `delivery.integration.prepare` | `connector_id`, `connector_version`, `candidate_digest`, `selection_version`, `evidence_id`, `review_round_id`, `review_decision_id`, `operation: "fast_forward"` |
| `delivery.integration.reject_prepared` | `integration_id`, nonblank `reason` (at most 1,024 UTF-8 bytes) |

Candidates contain the strict `binding` and `manifest` objects from
[the neutral delivery protocol](pr-delivery-review.md). Records are typed
`DeliveryEnvelope` values: `protocol: "awr-delivery"`, `protocol_version: 1`,
and `record: {kind, data}`. Unknown fields are refused. Connector subject fields
describe the managed observer; they cannot change the caller's identity or
grant access. Source renewal uses the flat fields above, not a nested `step`.
Connector configuration and all source mutation operations still require an
eligible human/system operator with explicit current management scope.
Integration preparation and withdrawal require current `delivery.finalize` and
write scope; Agent callers also need a live corresponding delegation. They
reserve or withdraw an approved intent and do not execute a repository operation.
Worker lease, dispatch and confirmation are not exposed as Team commands. See
[durable integration authority](delivery-integration.md#supervisor-workflow-over-http-or-mcp)
for approval discovery, permissions and recovery.

Dispatch reuses the original neutral transaction, journal and audit event. It
returns `{replayed, receipt, execution_authorized: false}`; `receipt.protocol`
is `awr-delivery-sync-v1`. There is no second generic command journal. Identical
retries reuse the historical receipt; changed substantive requests conflict.
The project revision is a validated audit cursor, not a global business CAS.

Use `awr_team_query` or `POST /v1/projects/{key}/query` for these reads:

| Operation | Selectors and meaning |
| --- | --- |
| `delivery.neutral.inspect` | Explicit `work_id`; current selected candidate, compact fact/history and scoped connector versions; each list has explicit truncation flags |
| `delivery.neutral.outcome` | Explicit `work_id` and original **client command retry** `request_id`; current caller actor/client's neutral request receipt |
| `delivery.source.status` | Explicit `work_id`; current physical/source validity, publication phases and synchronization status |
| `delivery.integration.inspect` | Explicit `work_id` and generated **`IntegrationRequest.request_id`** (`receipt.data.integration_id`); immutable original candidate/read set and current intent state |

These reads use one authenticated `RepeatableRead` transaction and do not
accept a session selector. Outcome lookup rechecks current read permission for
the actual work. Another actor/client cannot retrieve the receipt; a request
bound to a different work is refused. Missing receipts return `outcome: unknown`,
which does not exclude a concurrent or late commit. A committed result remains
`state_basis: at_commit` and never authorizes execution or certifies currentness.

After a missing response, query the original request first. Compare current
neutral facts and source status before recovery; keep the original request
identity and payload. Recheck when the receipt, candidate, source or authority
changes. Source-status reads never create locks or repair files. Unsupported
source writers remain explicit `Unsupported` results.

Legacy `command.inspect`, PR-oriented `delivery.inspect` and existing review
operations retain their original meanings. Use `delivery.neutral.outcome` for
neutral operations above; generic `command.inspect` does not query their
journal. Discovery describes operation-specific fields and the capability
response explicitly reports no background scheduling, repository effects or
neutral finalization at this layer. Historical, current, synchronized and
accepted are separate facts.

## Configuration and identity

A connector is an explicit tenant, project, main scope, workstream, work and
resource mapping. It binds one actor/client principal, a fact origin, a
monotonic configuration version and the coordinator epoch. Provider labels
never grant permissions. Configuration and revocation require current project
management authority, an explicit workstream management grant and an eligible
human/system operator. Ordinary delegated developer Agents cannot configure
connectors. Configuration does not give the target principal a grant.

Every mutation authenticates the current credential and rechecks membership,
delegation, grants, ownership, authority version, source snapshot and the actual
work contract in its transaction. The project revision is an audit cursor,
not an unrelated-work compare-and-swap condition. The store shares the project
barrier and audit order with existing coordination commands. All new tables
enforce tenant/project row-level isolation.

`adapter_observation` mappings require a system principal; `operator_recorded`
requires a human/system principal. Caller declarations keep their origin.
Only bounded neutral envelopes are accepted; no credentials or raw provider
payloads are stored. Receipt time comes from the server. An unknown external
observation time remains unknown.

## Candidate selection

The current executor must have a live owned session and claim, current fence,
lease and responsibility. A peer's general workstream write permission does
not let it select or replace that executor's candidate. Lease expiry does not
transfer responsibility. Changing executor still uses accepted handoff.

Candidate selection requires an exact selected-digest precondition. Candidate
identity/version is immutable: a different binding under the same identity
and version conflicts. The candidate manifest is validated against its digest,
and the required-check set must match the current source contract. Source
revision, target precondition, checks and manifest remain in the binding.
A candidate can describe SHA-256 Git revisions or an artifact, without a PR.
Every selection also gets an independent server version. Selecting an older
candidate again cannot revive an inspection reserved before its replacement.

Observers cannot select a candidate or claim its task. They can reserve an
inspection for the already selected candidate and their exact configured
resource. Both source and target resource references must fit that mapping.

## Provider-neutral worker scheduling

`DeliverySyncStore::schedule` is a read-only library entry point for an optional
configured provider worker. `DeliveryScheduleQuery` accepts only `work_id`,
`connector_id`, `limit` (1–32), and an optional returned `cursor`. It accepts no
read set, caller identity, approval flag, credential reference or repository path.
It does not add an HTTP/MCP route or start a background worker.

Each poll authenticates the live system principal, membership and actual read
grant, then checks the enabled `adapter_observation` mapping's actor/client,
work, workstream and coordinator epoch. It derives `read_set` from the active
source contract, catalog authority and ownership. A missing mapping, different
client, revoked credential, unavailable project or old connector epoch is
refused. Reading the schedule grants no write or integration authority.

The result distinguishes the current selection (`missing`, `current` or
`stale`) from original integration intents. Selection currentness checks the
source, contract, ownership, execution fence and mapped source/target resource.
An old intent retains its original candidate, read set, request and connector
version even when a new source has been activated. Current admission to the
same mapped resource is still required. Remapping to a different resource does
not schedule the previous resource's effects; those need explicit operator
reconciliation. Original issuer credentials and private authority are omitted.

`query_original: true` means dispatch occurred and the original operation must
be observed rather than submitted again. It is not retry permission. Prepared
requests still require the separate live lease/dispatch eligibility checks;
terminal records remain historical. Every response sets `execution_authorized`,
`acceptance_ready` and `source_synchronized` to false.

Pages follow persisted creation time and ID, with an upper anchor fixed by the
first page. New intents appear on the next fresh scan. Continuations are bound
to the current principal/grants, work/source/ownership/authority/contract,
connector mapping and selection version. Changes expire a cursor; reauthenticate
and start a new scan. Both anchors must exist inside the admitted scope.
Responses contain at most 32 records and 256 KiB; a byte-limited page returns a
continuation without dropping remaining records. State is observed at each
read, not frozen across pages. The read creates no lease, inspection, receipt,
audit event, source write or repository effect. It makes no deployment or native
business-acceptance claim and requires neither GitHub nor a webhook.

## Inspection, inbox and notification outbox

1. `reserve_inspection` transactionally allocates a server generation and a
   short lease. It binds the connector version, epoch, credential authority,
   candidate, ownership and source snapshot. Provider timestamps never order
   reconciliation.
2. The configured observer queries its resource outside the database locks.
   A notification may wake this inspection; its body is not acceptance proof.
3. `ingest_facts` consumes one bounded batch against that reservation. All
   records must match its candidate, including contract, version, target,
   manifest and checks. Facts, an inbox receipt, selected fact references,
   notification intent, audit and retry receipt commit together.

Query, polling and optional webhook wakeups share this path. A connector/event
identity with the same input digest returns its original observation receipt;
changed content conflicts. Requests additionally have actor/client-scoped
canonical retry hashes. A repeated observation slot within one generation
conflicts, rather than silently overwriting another result.

An older generation, expired inspection or changed selection/source produces
`superseded` history and cannot replace current fact references. Connector,
principal, grant or epoch revocation refuses ingestion. Old committed records
stay inspectable; historical receipt replay does not grant new authority.
Receipt status describes the observation at commit, while `inspect` separately
reports whether its binding is current now.
Mutation responses explicitly label this `state_basis: at_commit`, including
historical retries.

Notification intent is separate from execution dispatch. `pending` means a
fact notification awaits a consumer, not that source writeback or integration
is complete. Superseded observations retain a superseded notification record.
The observation transactions have no network or filesystem effects. Restart or
response loss recovers through the durable request/inbox receipts. Source-byte
effects use the separate publication journal below.

## Read and verification boundaries

`inspect` returns a candidate plus compact fact summaries and recent observation
receipts, with a limit of 32 per collection and explicit truncation flags.
The serialized read response has a 256 KiB ceiling; oversized responses are
explicitly refused rather than silently dropping required binding fields.
It separates current binding from historical observations. Acceptance,
execution authorization and source synchronization remain false at this layer.
Review and integration request references are unresolved descriptions; their
names do not create decisions or perform effects.

The PostgreSQL migration creates empty observation tables without inferring
neutral candidates or trusted evidence from legacy PR flags. Its DDL and schema
version change are atomic. Schema gates continue to require the binary's exact
version, refusing an unmigrated or newer database.

Synthetic isolated PostgreSQL regressions cover replay/conflict, concurrent
server ordering, late/expired results, candidate and executor protection,
source-contract changes, revocation, provenance, row-level isolation and
rollback/restart. These checks do not establish native-client acceptance or
operational adapter support.

## Cooperative source writes

The source library provides `LockedSourceFile` for exact, server-held source
paths. Planning writeback now uses the same stable advisory lock as delivery
metadata writers. Acquisition is nonblocking, so a writer does not wait on a
filesystem lock while holding the PostgreSQL project barrier. The lock is keyed
by the target leaf under its exact parent; different authorized root spellings
for that same file cannot obtain different locks.

The guard refuses traversal, replacement links, nonregular or multiply linked
source/lock files, changed directory/lock identities and stale fingerprints.
Replacement preserves permissions, syncs a uniquely created temporary file and
renames through the held parent. Unix also syncs that parent. Windows retains
the durability limit of its file-sync/rename primitives. A read-only source is
refused explicitly. Storage failures keep the existing bounded planning error
classification and recoverable request rather than exposing private paths.

`prepare_delivery_source_note` patches only the matching work's typed
`delivery_sync` reference note. It preserves status, unrelated fields, comments
and other work records. References are bounded, unique and versioned; identical
notes preserve exact bytes, and older metadata/selection revisions conflict.
The note contains no raw provider payload, credential, approval or verification
flag. A completion reference describes a receipt ID; this library does not
verify it, complete work or overwrite a source status. An authenticated domain
publisher must resolve every referenced fact and receipt before using it.

`LockedSourceFile::observe` checks the recorded directory/lock identity without
acquiring a writer lock, creating a missing lock or repairing the source. A
currentness query must not report a historical confirmation as current merely
because replacement source bytes happen to match. Successful patching alone
must not set `source_synchronized`.

## Recoverable source publication

Publication resolves the sole-source binding from the active source snapshot.
It accepts no caller-supplied path. The first supported writer is
`server_directory` with a YAML ledger; other source writers return explicit
`Unsupported`. Existing project-management authority, a current workstream
management grant and an eligible human/system operator are required. A
developer Agent cannot grant itself source-writing authority.

The immutable active contract snapshot and the physical metadata revision are
different versions. Publication reparses the actual source, validates the
whole publish package and compares the contract, graph, work identities and
referenced specifications. A delivery reference does not create source approval,
activate a changed contract, overwrite runtime ownership or finalize work.
Changed contracts/specifications continue through source planning, review and
activation. Unrelated YAML fields and source status remain unchanged.

The store provides these stages:

| Operation | Required observation and effect |
| --- | --- |
| `prepare_source_publication` | Check the selected candidate, current domain references and exact source fingerprint. Persist before/after bytes, directory/lock identity, authority, metadata version, short lease and monotonic fence. Source bytes stay unchanged. |
| `write_source_publication` | Observe the real file first. Exact before bytes can be replaced once; exact after bytes mean the effect already landed; any other bytes remain a conflict. |
| `confirm_source_publication` | Reindex actual after bytes and recheck current authority, candidate, references and lease before confirming the metadata cursor. It cannot apply an unwritten intent. |
| `renew_source_publication` | Recheck the same intent and owner, then allocate a new fence. An old worker cannot use its previous fence. |
| `abandon_source_publication` | Withdraw only an exact unwritten before image with no known landing. Keep the journal; never discard an unknown or observed source effect. |
| `source_publication_status` | Read bounded history and present physical/current-domain validity without changing source bytes, claims, approvals or locks. |

Leases are bounded to 5–300 seconds. One pending publication slot serializes
cooperating source publications across works. A new request cannot steal an
expired or unknown intent. Planning uses the same source guard. File-sync and
confined atomic replacement preserve the platform durability limits described
above; noncooperating writers can still race a rename, so observation and
fingerprint checks remain necessary.

Journal phases are `pending`, `source_written`, `confirmed`, `conflict` and
`failed`. A known landing is retained even if later reindexing fails. Rolling
back the file externally cannot turn that known effect into an unwritten intent.
Database failure after a filesystem effect leaves a queryable durable intent;
recovery observes the actual bytes rather than blindly repeating the write.
Revoked identity, changed authority or source drift stays explicitly unresolved.

Mutation receipts describe facts **at commit**. A replay returns its historical
receipt; it does not certify present source validity. Use
`source_publication_status.source_synchronized` for the current work. History
separates `source_synchronized_at_commit` from `source_current`. A confirmed
metadata update for another work preserves an unchanged current note, while
changed candidates, observations, contracts or filesystem identities invalidate
the affected note. The query does not recreate a missing lock to obtain success.

Confirmation proofs contain digests and counts, not source/specification bodies
or private paths. History is limited to 32 entries with explicit truncation and
a 256 KiB serialized response ceiling. Fact origin, a publication confirmation
and domain acceptance remain separate facts.

An optional completion reference must resolve to the actually selected current
domain completion receipt, bound to the exact candidate and current contract.
The publisher checks its canonical evidence digest and finalized artifact bytes
against the candidate manifest. An external `approved`/`merged` flag, a caller
verification field or a legacy unbound completion receipt cannot create a
completion reference. Publication neither supplies missing domain approval nor
changes the source status into a completed state.

Schema 44 adds the publication cursor/journal and a nullable candidate binding
on completion receipts. The atomic migration preserves legacy receipts as
unbound, creates no inferred publications and retains tenant/project row-level
isolation. Stop writers and retain matching source/database/binary backups for
an upgrade; older binaries still require their matching schema.

Synthetic filesystem and isolated PostgreSQL regressions cover concurrent
slots, replay/conflict, lease/fence renewal, multi-work currentness, authority
rejection, missing/stale references, read-only failures, identity replacement,
database failure before/after file effects and migration rollback. Domain
completion fixture rows test reference validation; they do not constitute a
native review, merge or business-acceptance result.

## Durable synchronization workers

The PG library now provides a bounded processing pump. Each applied observation
creates two durable intents in the same transaction as its notification:

| Intent | Successful result |
| --- | --- |
| `refresh` | A refresh request is available to the consumer; notification state becomes `delivered`. |
| `source` | The existing source journal is physically observed, written if needed, reindexed and confirmed; the source intent settles in that confirmation transaction. |

Processing a refresh never proves source synchronization. Neither intent creates
approval, task completion, repository integration or an execution grant. These
operations are library entry points used by explicitly configured server workers.
The library does not install a scheduler or expose a deserializable worker
capability through MCP. Server lifecycle configuration is described below;
repository adapters remain a separate layer.

`sync_intents` reads one explicitly authorized workstream in an authenticated
`RepeatableRead` transaction. Its page limit is 1–64, with `has_more` and
`next_after`. It reports original bindings, state, worker fence, lease liveness,
retry eligibility, publication reference and current binding validity. The
cursor must belong to the visible stream. It neither allocates a lease nor
changes files; the serialized response is bounded to 256 KiB.

`claim_sync_intent` requires the original read set, intent ID, stable request
ID, configured worker ID, expected fence and a 5–300 second lease. The caller
must be an eligible human/system principal with current project and workstream
management authority. Credentials are private function/configuration inputs,
never queue, source or audit fields. An Agent's developer delegation cannot
acquire this management capability. A worker ID alone grants nothing.

The claim allocates a monotonic fence and returns an opaque `DeliverySyncLease`.
Concurrent requests cannot acquire the same effective lease. Identical claim
retries can recover only the same still-current capability; a historical claim
receipt cannot revive an expired or replaced worker. Every processing step
rechecks the actual credential, grants, scope, source snapshot, ownership,
contract, epoch, connector version/generation, candidate selection and current
execution fence. A stale intent without a publication may be superseded. An
intent with a publication retains that journal for investigation or recovery.

`process_sync_intent` performs one bounded operation. Source processing first
associates the publication with its intent before any file effect. Preparation,
lease renewal, write, confirmation and notification acknowledgement use the
same domain transactions and existing journal. Worker liveness is checked
inside effect transactions, immediately before replacement and after source
observation/reindexing. Pump-owned publications cannot be renewed, written, abandoned or
confirmed through direct APIs that omit their opaque worker guard. Existing
manual publications retain their original authenticated path.

After a timeout or restart, inspect the queue and source status first. A
`succeeded` intent is a persisted outcome, not a fresh execution permission. For
an unresolved associated publication, the same configured actor/client can
acquire the next worker fence, renew the existing publication fence and observe
physical bytes. Exact after bytes are not written again. A previous worker
cannot acknowledge or confirm, even if its publication lease has not expired.
Revoked authority or changed bindings remain unresolved rather than silently
transferring publication authority to another principal.

`defer_sync_intent` records a bounded failure code and a 1–3,600 second retry
delay. Accepted codes are `source_conflict`, `source_unavailable`, `source_failed`
and `preconditions_changed`; raw errors, paths and secrets are not accepted.
Backoff preserves an associated publication. Conflict resolution never
discards an unknown source effect to obtain a new write slot.

Schema 45 adds forced tenant/project RLS, scoped foreign keys and the intent
queue. Its atomic migration backfills pending applied notifications using
their immutable inspection, source, contract and catalog bindings. It invents
no current grant, worker identity, approval or completion. Retain matched
database/source/binary backups and stop writers for upgrades; exact schema
version checks continue to reject mismatched binaries.

Synthetic PG/filesystem regressions cover normal processing, concurrent and
stable claims, worker/publication interleaving, expiry within transactions,
revocation, changed bindings, scoped pagination, backoff, RLS, migration
rollback and restart after an actual file effect followed by a database
failure. They do not replace complete natural-client business acceptance.

## Optional server worker lifecycle

The Team server can schedule the durable pump while serving its existing HTTP
and MCP routes. Unset `AWR_TEAM_DELIVERY_WORKER_CONFIG` means **no workers**.
The existing service TOML and `ServiceConfig` fields remain compatible. Enabling
this source group does not approve work, create an administrator or enable a
repository worker. The optional fixed-repository Git group is configured
separately with `AWR_TEAM_LOCAL_GIT_WORKER_CONFIG`; see
[Local Git service workers](local-git-delivery.md#optional-team-server-worker).

Set `AWR_TEAM_DELIVERY_WORKER_CONFIG` to a separate bounded TOML file:

```toml
version = 1

[[workers]]
project = "team"                       # An existing service project key.
worker_id = "delivery-team"             # A stable, unique configured worker ID.
workstreams = ["00000000000000000000000001"] # Replace with actual authorized IDs.
credential_env = "AWR_DELIVERY_WORKER_CREDENTIAL" # Name only; never a token.
poll_interval_ms = 1000
lease_seconds = 60
operation_timeout_ms = 15000
max_backoff_ms = 60000
page_size = 16
max_pages_per_poll = 4
max_jobs_per_poll = 16
```

Provision the referenced credential through the existing access-management
process, outside this file. Its current principal must be an eligible human or
system identity with project-management authority and explicit management/read
grants for **every** configured workstream. An ordinary delegated developer
Agent's token is insufficient. The worker ID does not identify or impersonate
a member. The file accepts no role override, raw token, new tenant/project
mapping, source path or repository URL.

Configuration rejects unknown fields, unknown service project keys, duplicate
worker IDs, duplicate scopes within a worker and excessive limits. It supports
at most 16 workers and 32 workstreams per worker. Polling is 100–60,000 ms,
operation timeout is 100–60,000 ms and must be shorter than the 5–300 second
lease, and maximum backoff is between the poll interval and 300,000 ms. Page
size and jobs per poll are 1–64; pages per poll are 1–16. Workers may share a
scope; PostgreSQL still fences their cross-process competition.

The server validates its schema, configuration and routes before starting
workers. `DeliveryWorkers::start` owns the shared store and both sealed group
admissions. All source and Git worker credentials/scopes, plus Git's fixed
repository checks, finish before **either** group starts; the whole sequence has
a 30-second ceiling. Missing or invalid credentials fail startup without partial
workers or implicit grants. Absent/empty groups perform no credential, resource
or PG access of their own. A denied Git group cannot leave an already admitted
source group running.
Runtime credentials stay in zeroizing memory and do not enter queue/source,
audit, status or error bodies. Credential rotation requires an explicit server
restart. Keeping the configured actor/client stable allows the original
publication to be recovered; a different or revoked principal cannot take it over.

Workers share one delivery store and pool, separate from HTTP handling. Each
worker has at most one operation in progress. Polls visit workstreams in turn,
advance bounded page cursors and resume partial pages after the last visited
item. Completed history does not permanently hide a pending intent. Pending,
due blocked and expired leased intents are eligible; a live lease is observed
and left alone. Retired cursors are reset for a fresh authenticated scope read.

Each actual fence acquisition has a fresh request ID. A missing claim or effect
response stays unknown: the next poll inspects durable state rather than
fabricating a handle or claiming a committed live lease again. After lease
expiry, the same current principal obtains a new fence and recovers the
associated source journal. Exact landed bytes are not rewritten. Changed
bindings retain their unresolved publication. Known source failures and changed
prerequisites use bounded persisted deferral codes; database errors/timeouts
after a possible effect never count as successful synchronization. Failure
backoff is capped and a successful authorized clean poll resets the loop delay.

Ctrl-C, Unix SIGTERM, HTTP termination and runtime drop stop both groups and cancel
in-flight asynchronous work. Graceful HTTP shutdown and worker joining each
have a five-second ceiling; groups join concurrently and remaining worker tasks
are aborted. Cancellation
does not acknowledge an intent or erase an unknown physical effect. Restart
uses the durable queue/journal and respects any still-live lease.

`DeliveryWorkerRuntime::monitor()` exposes at most one snapshot per configured
worker: configured project/worker, finite state/failure code, bounded counters,
retry delay and server observation time. State-change logs include only the
configured identity and finite codes; raw PG/filesystem/provider errors,
credential values and environment names are omitted. Snapshots are runtime
observations, not delivery receipts. There is no new anonymous management
endpoint; authenticated Inspector aggregation remains a separate integration.
The generic library/MCP capability flags describe their own layer and do not
certify an operator's worker configuration or liveness.

Synthetic isolated server/PG regressions cover all-or-none admission, normal
refresh/source processing, backlog/partial-page/scope traversal, contention,
credential revocation/expiry, source drift, retained stale publications,
actual file effects followed by database failure or timeout, recovery without
rewriting, cancellation during effects, runtime drop and complete loopback
HTTP startup/shutdown. These mechanism tests do not constitute native team
business acceptance or a deployment.
