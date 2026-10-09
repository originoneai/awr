# Team execution settlement contracts

**Development on `main`: explicit source contracts and caller-managed workspace
settlement, with readable artifact verification and independent Agent review.**
Published 0.5.1 packages do not provide this Team capability. Native team business
acceptance is separate from database regression checks.

A task's execution report, artifact verification and effect settlement are
different facts. A caller can report that work stopped; this does not prove
that an external process stopped, that an artifact meets its acceptance
criteria, or that a repository change was integrated.

## Explicit independent workspace agreement

Contract V3 adds one closed `execution_settlement` policy:

```json
{
  "mode": "independent_workspace_v1",
  "workspace_id": "worker-a"
}
```

This agreement is for caller-managed work with workspace-local effects and no
shared external operation. `workspace_id` is a stable, opaque identity, not a
filesystem path, sandbox proof or permission grant. It uses 1–128 ASCII bytes,
starts with a letter or digit, and otherwise allows letters, digits, `.`, `_`,
`:` and `-`. Different workspaces require different identities; an execution
must use the identity selected by its task contract.

The contract requires nonempty scoped paths and verification requirements,
with `completion_policy: caller_managed_execution_and_agent_review`. That
policy keeps caller evidence at its original trust level and requires
accessible artifact bytes and an independent Agent review before completion.
Selecting the workspace policy grants neither trusted executor authority nor
human approval, and cannot select self-review or `ordinary_confirm` instead.

The workspace policy, completion policy, scope and verification requirements
are part of the canonical contract hash. Changing them creates a new contract
basis; it cannot reinterpret a previous run or receipt.

## Publish from an authoritative source

Add the policy to the selected work item in a YAML workstream ledger:

```yaml
id: API-1
title: Implement the inventory API
workstream: api
paths:
  - src/inventory
acceptance:
  - Inventory responses satisfy the agreed API contract
verification_requirements:
  - Independently verify the delivered artifact and API checks
completion_policy: caller_managed_execution_and_agent_review
execution_settlement:
  mode: independent_workspace_v1
  workspace_id: worker-a
```

Publish preparation emits contract `awr-team-contract-v3` for this item,
bundle `awr-team-workstreams-v3`, and parser version `awr-team-workstreams/3`.
Other items preserve their existing contract codec and semantic hash. A V3
bundle can contain V1, V2 and V3 contracts; existing dependency assurance and
same-workstream restrictions still apply.

The preview's `execution_settlement_diffs` identifies the exact work item and
old/new policy. Adding, changing or removing the policy is visible. When no
policy changes exist, this field is omitted, preserving legacy preview output.
Removing the source policy returns that item's codec to the one appropriate
for its remaining fields; it does not rewrite historical execution facts.

## Report an ordinary workspace outcome

Preparation stores the source-selected policy. Admission stores the selected
execution mode and the original lease generation. A caller cannot add a policy
to a historical run, change its workspace, or obtain attestation authority by
including new report fields. Caller-managed V3 reservations use the declared
workspace as a lexical namespace; named and integration resources retain their
project-wide protection. A distinct workspace identifier is an agreement, not
evidence that two processes actually use separate clones or isolated Git data.

`execution.report` accepts an optional, closed `workspace_settlement` declaration
in its existing arguments. Replace the illustrative identities, versions and
SHA-256 values with the current preparation, admission and claim facts:

```json
{
  "session_id": "current-session",
  "expected_session_version": "1",
  "execution_id": "admitted-run",
  "expected_execution_version": "2",
  "outcome": "succeeded",
  "output_digest": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
  "observed_paths": ["src/inventory/result.json"],
  "note": "Workspace execution stopped with no external effects.",
  "workspace_settlement": {
    "workspace_id": "worker-a",
    "input_digest": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "environment_digest": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
    "claim_id": "current-claim",
    "expected_fence": "1",
    "expected_lease_version": "1",
    "executor_stopped": true,
    "no_external_effects": true
  }
}
```

The outer command still requires its current project, workstream, work,
ownership, coordinator epoch, contract and request binding. The environment
digest references the measured environment; keep credentials and environment
dumps outside the record. For a successful workspace run, `output_digest` is
the digest of the artifact bundle that will be submitted for review.

MCP command discovery exposes this optional declaration and its closed fields.
Construct the review artifact before reporting success, hash its exact bytes,
then submit the same bytes through `artifact_text` or decoded `artifact_hex`.
Keep `payload.output_digest` and the execution report bound to that hash. The
server separately checks the submitted bytes; passing tests alone cannot supply
the stopped/effect assertions. Other execution policies retain their distinct
output and artifact digest semantics.

Settlement requires a running caller-managed V3 execution, a current matching
contract, the original current claim and fence, and the exact admitted path
reservations. The `independent_workspace_v1` policy also requires a live lease.
The admitted scope must be nonempty and unique. Unknown or shared resources,
another unresolved run, prior receipts or outbox exposure, dependency/planning
blocks, a preexisting recovery barrier, or observed paths outside admission keep
the conservative recovery path. No subset release or blanket barrier clearing
is allowed. Malformed declarations and identity/version mismatches refuse
without writing a report.

Success, failure and cancellation can release only the exact admitted workspace
resources. All remain `caller_asserted`; failure and cancellation leave the task
unfinished. An omitted, false or unknown stop/effect declaration, an unknown
outcome, or an expired V1 lease records the caller's observations without claiming
settlement. V1 checks the live lease again before the transaction finishes.
After a same-fence renewal, the report binds the current lease version while
reservations retain their original admission generation.

| Fact | Ordinary settled report | Completed artifact and review chain |
| --- | --- | --- |
| `terminal_reported` | `true` | `true` |
| `artifact_verified` | `false` | `true`, after reading and hashing the bytes |
| `effects_settled` | `true`, only for admitted workspace paths | Same bounded workspace basis |
| Task complete | `false` | Only after every current completion gate passes |
| Execution assurance | Caller assertion | `caller_asserted_workspace_settled` |
| Human approval / trusted execution | Neither is inferred | Neither is inferred |

The caller receipt binds the run, workspace policy, input/environment, session,
client, ownership, contract, epoch, fence, original admission generation, current
claim version and exact released reservations. Completion rechecks that closed
receipt, its digest and current run bindings, re-reads finalized artifact bytes,
requires their SHA-256 to match the reported bundle, and verifies the independent
Agent review. Missing, altered or unrelated artifacts, self-review, invalidated
reviews and changed contracts remain failures. The old lease need not stay live
while an already settled execution waits for review.

If a report response is lost, inspect the run/receipt or repeat the exact same
request. A replay returns the committed receipt and does not authorize another
physical execution. Do not invent a new request or rerun work to resolve an
unknown outcome.

## Explicit late terminal reports

`independent_workspace_v2` is a separate source opt-in in the same closed policy
shape. Set it before preparing and admitting an execution:

```json
{
  "mode": "independent_workspace_v2",
  "workspace_id": "worker-a"
}
```

V2 permits a terminal report after elapsed lease time only. The originating claim
must still be the active, current claim, with the same authenticated actor,
client, active session, responsibility, epoch, contract, fence and current lease
version. Explicit stop and no-external-effects assertions and every resource,
scope, dependency and recovery check above remain required. Released or replaced
claims, controlled/shared effects, old unknown reports and revoked authority
cannot use this path. The policy cannot be added to an old execution afterward.

The immutable caller receipt includes the selected V2 policy and a
`lease_observation` with `basis` (`live` or `expired_current`), `expires_at` and
`observed_at`, measured using the database clock immediately before writing the
receipt. This remains a caller assertion about workspace effects. It does not
verify process termination, upgrade artifact trust or provide approval.

V2 never renews or revives the old lease. Further execution requires a fresh
claim and normal admission. Exact-request replay retrieves the same receipt;
it never starts another run. Readable artifact verification and independent
review remain necessary before finalization. Workspace settlement still cannot
settle a push, merge, deployment or other external operation.

Claim acquisition, renewal and execution admission return a bounded
`lease_guidance`: its condition, current claim/version/deadline basis, next action
and recheck trigger. Renew before the deadline when work continues; report the
actual terminal outcome when stopped. Refresh the facts after renewal,
disconnection or ownership/contract changes. These instructions do not install
a heartbeat, mutate query state or replace the host's process supervision.

V1 behavior, wire representation and contract hashes stay unchanged. Selecting
V2 changes the contract hash, and runtimes that only know V1 reject the new mode.
No historical contract, execution or receipt is automatically upgraded.

Team schema 54 updates only the closed execution-policy and Agent completion
constraints to recognize V2. It performs no provenance backfill. Upgrade with
the matching server binary and a recoverable database snapshot; older runtimes
reject schema 54. A rollback needs the matching pre-upgrade snapshot and binary,
rather than changing the recorded schema number.

## Confirm a controlled execution

An explicitly admitted `reference_write_v1` run has a separate confirmation
path. It requires a system client with the exact attestation grant version
recorded at admission. A mode name, business role, CI result or later grant does
not establish that authority. Ordinary agents continue to report at their
original caller trust level.

A caller report may attribute its recovery barrier to that run and receipt only
while the lease, contract, epoch, fence and exact admitted directory reservations
match. Scope violations, another unresolved run, additional resources, outbox
exposure and preexisting or subsequently rewritten barriers remain operator
recovery cases. A repeated valid report may bind its new receipt; old receipts
cannot clear the current barrier.

The controlled client first inspects `execution.inspect`. If
`controlled_confirmation_available` is true, it checks the actual executor and
the latest receipt, then submits `execution.attest` with its ordinary facts and:

```json
{
  "reviewed_receipt_id": "latest-inspected-caller-receipt",
  "facts": {
    "executor_stopped": true
  }
}
```

These fields supplement the required attestation arguments; this fragment is
not a complete command. The transaction rechecks all bindings, records the
confirmation and releases only that run's resources. It clears recovery only
when the outcome is terminal and no other effects remain. An unknown outcome
or explicit `executor_stopped: false` keeps resources and recovery blocked.
Omitting either confirmation field preserves legacy
privileged settlement, without automatically clearing the work barrier.

Reads expose `terminal_reported`, `artifact_verified`, `effects_settled`,
`settlement_basis`, `settlement_scope` and `recovery_blocked` independently.
Artifact verification requires the current selected completion's bound evidence
and readable finalized bytes with matching digests. A report or attestation
alone cannot mark it verified. Altered bytes or a changed completion/contract
remove that observation; historical receipts remain available.

Migration 42 stores nullable run/receipt attribution and invalidates it on
every explicit recovery-barrier update, including `true` to `true`. Historical
barriers stay unattributed. This prevents a controlled client from clearing a
second writer's recovery cause.

## Workspace effects and repository delivery

Workspace settlement does not settle a repository push, external review, merge,
deployment or shared integration operation. Close the workspace phase before
starting a separate delivery operation, carrying its exact source revision and
artifact digest. If an external effect already occurred or its result is
unknown, do not declare `no_external_effects: true` to bypass recovery.

Existing dependency assurance modes do not accept the new workspace basis.
In particular, `agent_reviewed_caller_asserted_reconciled` still requires actual
operator reconciliation. A new producer capability does not silently broaden a
consumer's acceptance policy. Repository providers and downstream adoption have
separate contracts.

## Compatibility and execution boundaries

- V1 and V2 retain their original wire representation and canonical hashes.
  They reject `execution_settlement`, including empty and null values.
- V3 requires an explicit valid policy. Unknown modes or fields, duplicate
  policy keys, missing workspace identity and implied trust flags are rejected.
- Source publication rejects duplicate YAML mapping keys before conversion.
  A malformed explicit V3 completion policy cannot fall back to a mapping
  default. Previously valid V1/V2 source mappings retain their behavior.
- A V1 or V2 workstream bundle cannot silently carry a V3 contract.
- No existing contract or execution is automatically migrated to this policy.
  A source status such as `completed` remains a source note, not verification.
- V1/V2 caller reports retain the conservative reconciliation mechanism.
  Controlled admission cannot select ordinary settlement through report fields.
- PostgreSQL migration 41 is additive and transactional. Historical policy,
  mode and admission generation remain null, and the new observation flags
  default to false. Migration does not guess old provenance or upgrade trust.

Ordinary settlement never replaces process supervision or physical isolation.
Old contracts, V1 expired attempts, scope violations and unknown effects retain
their recovery protection. V2 relaxes only elapsed time for the explicitly
admitted, otherwise unchanged terminal workspace report described above.

A controlled adapter follows its separately delegated authority and may attest
only to runs it controls. CI can verify an artifact; it cannot prove a process
stopped. See [delegated execution authority](../integrations/delegation-execution-auth.md).
