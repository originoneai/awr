# Durable provider-neutral delivery and source publication

The delivery coordination store persists the neutral records described in
[the delivery contract](pr-delivery-review.md). It does not require a GitHub
repository, a pull request, SHA-1 revisions or a webhook. An artifact delivery
and a local Git delivery use the same candidate and observation boundaries.

The observation path stores facts and notification intent. It does **not**
execute a repository operation, resolve review references as approvals or
accept a task. A separate authenticated publisher can write bounded delivery
references to the project's configured authoritative source. These store APIs
do not establish deployed integration or native business acceptance;
authenticated MCP operations, background scheduling and adapter execution are
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
