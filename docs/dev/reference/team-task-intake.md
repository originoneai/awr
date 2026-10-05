# Team task responsibility and intake

Task responsibility records the member accountable for a result. An execution
instance identifies the person or explicitly bound Agent doing the work. A
coordination lease is temporary. These are separate facts: releasing or expiring
a lease does not put an owned task back into the available pool.

## Responsibility persistence kernel

The current Rust kernel provides `ResponsibilityStore::claim_available` and the
pure `apply_claim_available` transition. The request contains `request_key`,
`expected_version` and `claimant`. It takes sole ownership only when the current
responsibility has **no owner, no pending reservation and no executor**. The
claimant is removed from the collaborator list; other collaborators and the
independent reviewer remain. The independent reviewer cannot become the owner.
The transition creates neither an execution instance nor a coordination lease.

This kernel is not an authenticated MCP self-claim command. Its callers must
separately enforce membership, current scoped authorization, source/contract
availability, waits, accepted dependencies and execution admission. It must not
be used to treat a supplied member name as proof of identity. Authenticated
assignment, acceptance and self-claim entry points are a subsequent integration.

## Retry contract

Team PG responsibility operations bind a project-scoped request key to the complete
canonical request: the protocol version, tenant, project, task, operation,
attributed person, expected responsibility version and every supplied parameter.
This covers assignment, acceptance, available ownership, execution claim/release,
owner transfer, Agent swap and pending-state changes. Execution claim and Agent
swap have different request discriminators even though they share an event type.

| Request | Result |
| --- | --- |
| Same key and complete input | Original immutable receipt with `replayed: true`, plus the current responsibility projection. |
| Same key with changed input, task or operation | `IdempotencyConflict`; no data changes. |
| New key with a stale responsibility version | `PreconditionsChanged`; no data changes. |
| Existing historical receipt without a verified request hash | `IdempotencyConflict`; historical data stays intact. |

Replaying an old assignment does not restore its old owner or executor. A receipt
describes the original transition; the accompanying task describes the current
state. Read that state before deciding whether any new action is needed. Do not
blindly change the key to bypass an unknown result or unverifiable old receipt.

Request-key locking precedes task locking, including requests for different
tasks. First inserts and version changes serialize on the task. Projection,
collaborators, immutable event and receipt commit in one transaction; failure in
any write rolls all of them back. Competing available claims cannot both become
owners.

The SQLite event codec recognizes the additive available-claimed event. Existing
local responsibility API behavior is unchanged by this Team PG retry contract.

## PostgreSQL schema 40

Migration 40 adds nullable `responsibility_receipts.request_hash`. New kernel
operations write a canonical SHA-256 hash. Existing receipts remain null because
their event payloads cannot reconstruct the complete original request. The
migration does not rewrite tasks, events, member identities or old receipts.

Stop old writers before upgrading; an older writer can still produce null hashes
and cannot participate in the new replay contract. Use the normal schema-version
check and retain a matching database/binary snapshot for rollback. This schema
change does not announce a package release or certify native teamwork acceptance.
