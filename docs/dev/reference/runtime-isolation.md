# Events, claims and work branch ownership

AWR coordinates local work through sessions, claims, work branches and optimistic
project revisions. Source files remain authoritative and source ownership is
separate from runtime claim ownership.

## Event history

Public Store callers can append an observation or invoke a domain operation. They
cannot obtain its SQL connection or arbitrary transaction callback. Reserved event
types such as `work.completed` and `claim.expired` require their corresponding
domain operation; appending an observation cannot fabricate those receipts.

An event bound to a session inherits that session's work and branch. A conflicting
work, branch, session or project is rejected. An explicit CLI `--branch main` or
MCP `"branch": "main"` must agree with that session, just like a named branch.
Store and Runtime callers use `append_event_in_branch` when the draft branch is
an explicit selection, including `None` for main. The existing `append_event`
method permits session inference when the branch is unspecified. Updating a returned event value only
changes the caller's copy. Existing stored event metadata and bodies are protected
against UPDATE, DELETE, UPSERT and REPLACE on configured Store connections.
Recursive SQLite triggers are enabled for new, reopened, readonly and in-memory
snapshot connections so REPLACE's implicit deletion cannot bypass that protection.
This is a connection setting; no schema migration is required.

Runtime changes and their receipts commit together with one project revision
advance. A handoff or resume can append multiple linked history receipts at that
same revision. Failed operations retain the previous rows and revision.

## Claim lifetime

A claim belongs to one project, work item, session, agent label and work branch.
Only one effective claim can occupy a work item across all branches. Competing
processes first compare `expected_revision` in the write transaction; a contender
that refreshes its revision must still respect the existing claim.

An omitted TTL has no automatic expiration. A finite TTL expires when its deadline
is reached. Zero and overflowing TTLs are rejected without creating partial
sessions or claims. Acquiring an expired reservation records the expired claim
identity and the new owner atomically. Failure to append that receipt rolls the
whole acquisition back.

Inspect a claim through its session or Doctor before selecting an explicit repair:

```sh
awr --json session show SESSION_ID
awr --json doctor
awr --json doctor repair expire-claim CLAIM_ID \
  --expected-revision CURRENT_REVISION \
  --reason "The lease has elapsed"
```

Expiry repair rechecks the deadline inside the transaction. It refuses an
unexpired or indefinite claim. It does not select a different claim or end the
owning session. Claim release identifies the holding session; another session
cannot release it. Ending or explicitly interrupting a session affects only that
session's claims. Handoff and resume preserve a live claim's existing expiration
unless an explicit, supported acquisition or handoff TTL is requested; an expired
claim does not confer inherited ownership.

## Work branches

Switching the default work branch changes selection. It does not move existing
sessions, claims, checkpoints, evidence or history, and it does not change the Git
checkout. Distinct branches retain independent history, but cannot acquire simultaneous
claims for the same work item. Different work items remain independently claimable.
Source writes still use the shared-source conflict guards.

Implicit session selection uses the selected branch and reports ambiguity.
An explicit session ID selects that retained identity; supplied work and agent
filters must agree. Event queries support exact branch and session filters with
stable pagination. Shared source projections remain visible. Evidence from another
branch can be inspected as historical evidence and does not become current for
the selected branch.

Handoff requires the same work item and branch, while retaining the original
checkpoint owner. Resume requires the original branch to be current and creates
one successor. Closed branches reject new sessions and observations; retained
history remains readable.

The [isolation contract](../../../tests/concurrency/contract.json) fixes 20 conditions;
the [runner](../../../tests/concurrency/README.md) includes live competing OS processes,
API privacy checks and real CLI/MCP regression fixtures. These checks establish
local coordination and provenance, not authentication between local users,
tamper-proof database files, distributed locking, E4 or a release. Cross-platform
validation remains a separate V1 gate.

## Person responsibility and execution identity (WS-015)

A task (work item) is the stable work unit. A **person** is the responsibility unit.
An **execution instance** is either a person acting directly or an agent run that is
explicitly bound to a person. Roles on one task are distinct: sole owner, collaborators,
current executor, and independent reviewer.

Assigning responsibility, accepting responsibility, and claiming temporary execution are
separate operations with versioned events and idempotent receipts. Claiming execution
updates the current executor only; it never steals sole ownership. An unassigned pool may
have no owner. Personal mode may default owner=self while using the same underlying
semantics.

The same person may swap agents without changing ownership. Owner transfer requires
authorization by the current owner and acceptance by the receiver; group names and agent
labels are not substitute owners. Departure, disable, no-acceptor, and legacy-identity
migration remain explicit pending states. Person↔agent bindings are stored explicitly and
must not be inferred from `actor.kind`. History is retained in SQLite
(`task_responsibilities`, `responsibility_events`, `responsibility_receipts`) and Team PG
(schema 19). Coordination `claims` remain the lease surface and are linked optionally via
`coordination_claim_id` without implying ownership.


## Authorized agents and explainable claims (WS-016)

Agent runs require an explicit `AgentAuthorization` bound to a responsible person, client/session,
project/workstream/task-or-pool scope, actions, expiry, and verifiable capabilities. Platform
service accounts also record a maintainer person. Authorizations are listable and revocable.

Claim eligibility is explained from membership, person delegation, assignment policy, resources,
and host capability. Self-reported skill hints never flip eligibility. Responsibility accept,
collaborative occupancy, and start-work admission are separate decisions: unmet dependencies may
allow ownership assignment but must not admit side-effecting execution. Concurrent claims keep a
single effective executor.

Delegation is narrowing-only. Changing model, client, or session cannot bypass revoke, expiry,
scope, or independent-reviewer separation.

## Confirmed Team handoff (WS-017)

Long-term Team handoff requires receiver confirmation. **Execution handoff** and
**responsibility transfer** are separate operations (`kind=execution|responsibility`).

State machine: `propose → inspect → accept | reject | cancel | timeout`.

| Status | Responsible | May continue | Notes |
|---|---|---|---|
| proposed / inspected | original person | original | Receiver may inspect package; must re-prepare context before accept |
| accepted (execution) | original owner | successor | Ownership unchanged; successor execution only after prior stop/reconcile |
| accepted (responsibility) | receiver | receiver | Does **not** grant execution by itself |
| rejected / cancelled / timed_out | original | original | Original keeps recovery duty; **timeout ≠ stop** |

Handoff packages bind task+contract version/hash, current person+execution instance,
consumed context digest, checkpoints, artifact versions, branch/dir, dependencies,
todos, awaiting replies, and unknown side effects. Chat summaries are insufficient.

Accept is transactional: re-check identity, authorization, versions, resources, and
fence; unknown executions block accept; late writes under an old fence cannot become
the current result. Concurrent accepts keep exactly one effective outcome (request-key
receipt + row lock). Same-person agent swap is an execution handoff; cross-person
cross-agent is supported; disconnect/reject/no-receiver preserve original duty.

Authenticated Team HTTP/MCP: `handoff.propose|inspect|accept|reject|cancel|timeout`
commands and `handoff.inspect` query. Local MCP `awr_team_handoff` validates packages
and explains duty without mutating Team state; it is offered only when the
[Team runtime switch](team-switch.md) is on. Persist in SQLite (`010`) and Team PG
(schema 21).

## Versioned delivery dependencies and adoption credentials (WS-030)

Cross-stream **hard** dependencies bind concrete Work identity, contract hash,
artifact digest, completion receipt, source/environment, adoption policy and
export authorization. A WS-018 completion receipt is trusted acceptance evidence
only when `team_independent_acceptance` is true; author self-report and personal
self-review never unlock downstream execution.

Two policies are explicit:

- **fixed_delivery** — the selected receipt/contract/artifact survives unrelated
  upstream replanning; historical adoption credentials retain the original proof.
- **current_contract** — consumers revalidate when the authoritative current
  selection drifts from the adopted delivery.

Export authorizations are grant/revoke auditable. Cross-project dependencies are
refused. Persist in SQLite (`012_delivery_deps`) and Team PG (schema 26) with
tenant/project RLS matching the responsibility-table CR pattern.

## Cross-stream dependency graph and atomic cycle checks (WS-031)

Task edges form a same-project DAG across workstreams: `A1 → B1 → A2` is legal
even though stream-level arrows return to A. Hard cycles return an explainable
closed path (`A1 -> B1 -> A2 -> A1`). Concurrent graph mutations serialize on the
project coordination lock, re-validate the complete required graph against
authoritative contract ids, and refuse cyclic unions or dangling endpoints.
Readiness requires every necessary dependency to be satisfied; shared outcomes
are referenced by identity (and adopted via WS-030 credentials) rather than
copied into each consumer stream.

## Selective invalidation and execution boundary revalidation (WS-032)

Only **affected** downstream consumers re-evaluate when a provider delivery
changes. **fixed_delivery** consumers keep their pinned old version through
unrelated upstream progress or new versions; they re-evaluate only on revoke or
pinned-artifact unavailability. **current_contract** consumers revalidate when
the authoritative current selection drifts.

Prepare, dispatch and complete recheck adoption/bindings under the project lock
to prevent revoke races. Mid-execution invalidation retains real effects and
assigns recovery duty; historical receipts are not erased. Unrelated works
continue.

When implementation discovers a new required dependency, persist a scoped
planning change (old/new graph versions, acceptance contracts, cancel/split
relations, continue conditions), block affected actions first, then require
authorized confirmation of the new graph and acceptance contract. Unrelated
tasks keep advancing. Persist in Team PG schema 27.
