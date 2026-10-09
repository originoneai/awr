# Team task intake

Development on `main`. These commands are not part of the published 0.5.1 packages.

Members connect their chosen Agent to the project's Team MCP service with their
own credential. The Agent calls `work.next`, consumes `work.prepare`, and takes
work within its current permissions. A manager can reserve work for a member;
unassigned, ready work can also be claimed by a member's Agent. Both paths use
the same responsibility, authentication and concurrency checks.

| Command | Required action | Result |
| --- | --- | --- |
| `task.assign` | `work.assign` | Reserve responsibility for an eligible project member, awaiting acceptance. No execution session or lease is required for the supervisor. |
| `task.accept_assignment` | `claim.manage_own` | The designated member accepts the current reservation and acquires a coordination lease in one transaction. |
| `task.claim_available` | `claim.manage_own` | Take responsibility for an unassigned task and acquire a coordination lease in one transaction. |

Membership, duty ceilings, resource grants and one live covering Agent delegation
must all permit the action. Acting identities come from the authenticated
delegation's exact person/Agent binding. `assignee_person_id` is a dispatch target;
it cannot change the acting member. Separate grants are not combined to invent
command authority. A supervisor needs an explicit assignment grant and, when
acting through an Agent, a matching assignment delegation.

## Agent workflow

Supervisors discover work through `work.inbox`, consume the selected task's
`work.prepare`, then find eligible recipients through `task.assignees`:

```json
{
  "protocol_version": 1,
  "op": "task.assignees",
  "work_id": "current-work-reference",
  "search": "Mei",
  "limit": 25
}
```

This read requires current assignment permission and write scope. An Agent needs
**one live covering delegation with both Inspect and AssignWork**; separate
read/assignment grants cannot be combined. No project-admin permission is needed.
Only active registered members eligible for this workstream are returned, as
`name` and `assignee_person_id`. Duplicate names retain separate identities;
never guess which member a duplicate name denotes. Credentials, Agent labels,
administrative configuration and unrelated members are not returned.

`search` is an optional, case-insensitive literal name substring (at most 512
UTF-8 bytes). Page size defaults to 25, with a maximum of 100. Each response scans
at most 100 members. Follow `next_cursor` even when `items` is empty; one empty
page does not establish that no recipients exist. Cursors contain ordinal
positions, not hidden member IDs, and expire after relevant identity, authority,
source, ownership, contract or project-revision changes. Refresh instead of
combining pages from different snapshots.

The result is advisory. Refresh `work.prepare` before `task.assign`; assignment
rechecks current recipient eligibility and task ownership atomically. The read
does not reserve work, authorize execution or replace dependency admission.
Single-task dispatch guidance gives this lookup as its one next query, while
recovery and missing mandatory context retain priority.

1. Query `work.next` for current-client continuation and scoped tasks.
2. Consume `work.prepare`; keep its contract, ownership and responsibility versions.
3. Start your own AWR session when one does not exist.
4. Accept your pending assignment, or claim a ready pool task. Continue owned
   work through the existing claim path when a fresh lease is needed.
5. Use the separate execution preparation/admission workflow before effects.
   A responsibility or claim receipt alone never authorizes execution.

All three operations use the existing `awr_team_command` envelope. Obtain its
workstream, epoch, authority, ownership and contract fields from current reads.
Keep a stable request ID for an exact retry. The following are **args only**:

```json
{
  "assignee_person_id": "member-reference",
  "expected_responsibility_version": "0"
}
```

Assignment may reserve work whose dependencies are not ready. Acceptance and
self-claim recheck enabled work, current contracts, waits, unresolved effects and
accepted dependency receipts. A completed upstream flag alone is insufficient.
Current cross-workstream export/adoption limits still apply.

```json
{
  "session_id": "owned-session-reference",
  "expected_session_version": "1",
  "expected_responsibility_version": "1",
  "expected_work_version": "0",
  "ttl_seconds": 60,
  "assignment_request_key": "current-reservation-reference"
}
```

For `task.accept_assignment`, copy the reservation reference from
`responsibility.pending.transfer_request_key`. A responsibility handoff is a
different operation. For `task.claim_available`, omit `assignment_request_key`.
TTL is 1–3600 seconds. Versions are canonical decimal strings; the responsibility
version is separate from the workstream ownership, work and lease versions.

Assignment does not require a supervisor's execution session. If an optional
`session_id` is supplied, its current `expected_session_version` must also be
supplied, and the session must belong to that caller and task.

## Continuation, conflicts and receipts

Sole responsibility survives lease expiry and safe release. Another member,
Agent or client cannot take over simply because a connection closed or a lease
elapsed. A retained executor requires a controlled, accepted handoff; its current
binding and client are checked again during claim renewal and execution admission.
An accepted handoff by the successor client records the responsibility version
used for continuation. It does not revive an old execution or resolve unknown effects.

Two self-claims, or a dispatch racing a self-claim, cannot both own a pool task.
On conflict, query current work and select the appropriate next action. Do not
overwrite another member's reservation or generate new request IDs to hide an
unknown outcome. Responsibility, claim, fence, events and operation receipts
commit together; a failed transaction leaves none of those partial effects.

An exact retry preserves the original immutable `receipt`. The response also
returns `current_responsibility`, which may have changed since that receipt.
Replay does not renew a lease, restore historical ownership or authorize effects.
Current credentials and action permissions still gate replay. Use `claim.inspect`
for current lease state and `command.inspect` for a recorded request outcome.

Preparation and observation expose current responsibility, version, reservation
and executor. Navigation distinguishes pool work, your assignment, owned work,
another member's task and required handoff. One bounded guidance item supplies
its condition, basis, next action and re-evaluation trigger. These reads neither
reserve work nor create member records.

Task intake does not depend on a repository provider. Implementation, review,
merge and delivery remain separate facts with their own evidence contracts.

## Responsibility kernel and historical receipts

`ResponsibilityStore::claim_available` and the pure `apply_claim_available`
transition remain persistence/domain APIs. They establish ownership only when
there is no owner, reservation or executor, preserve other collaborators and the
independent reviewer, and reject the reviewer becoming the owner. They do not
authenticate a caller or create a lease. The authenticated commands reuse the
kernel inside their own covering transaction.

Kernel receipts bind the complete canonical request, including its operation,
attributed member and expected version. Assignment, acceptance, pool ownership,
execution claim/release, owner transfer, Agent swap and pending-state changes keep
separate request discriminators. Request-key locking precedes task locking;
first inserts and version CAS serialize competing writers.

| Kernel request | Result |
| --- | --- |
| Same key and complete input | Original receipt and current responsibility. |
| Same key with changed input, task or operation | `IdempotencyConflict`, without writes. |
| New key with stale responsibility version | `PreconditionsChanged`, without writes. |
| Historical receipt without a verified request hash | `IdempotencyConflict`; retain the historical data. |

Schema 40 adds nullable `responsibility_receipts.request_hash`. New operations
write canonical SHA-256 hashes. Existing receipts remain null because partial
event payloads cannot reconstruct their complete original requests. The
migration does not rewrite tasks, events, identities or historical receipts.
Stop old writers before upgrading; they can still produce unverifiable null
hashes. Retain a matching database/binary snapshot for rollback. The additive
SQLite event codec remains supported, and local responsibility API behavior is
unchanged. Schema compatibility and regression success do not certify native
teamwork acceptance or announce a package release.
