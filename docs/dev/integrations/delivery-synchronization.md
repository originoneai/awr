# Durable provider-neutral delivery observations

The delivery coordination store persists the neutral records described in
[the delivery contract](pr-delivery-review.md). It does not require a GitHub
repository, a pull request, SHA-1 revisions or a webhook. An artifact delivery
and a local Git delivery use the same candidate and observation boundaries.

This layer stores facts and notification intent. It does **not** execute a
repository operation, resolve review references as approvals, accept a task,
write the authoritative source, or count as native business acceptance.
Source publishing, authenticated MCP operations and adapter execution are
separate integration layers.

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
There are no network or filesystem effects in these transactions. Restart or
response loss recovers through the durable request/inbox receipts.

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

These are filesystem/library mechanisms. They do not yet provide a durable
delivery publication journal, a synchronization endpoint or native business
acceptance. Successful patching alone must not set `source_synchronized`.
