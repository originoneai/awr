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

Settlement requires a running caller-managed V3 execution, a current matching
contract, the live claim and fence, and the exact admitted path reservations.
The admitted scope must be nonempty and unique. Unknown or shared resources,
another unresolved run, prior receipts or outbox exposure, dependency/planning
blocks, a preexisting recovery barrier, or observed paths outside admission keep
the conservative recovery path. No subset release or blanket barrier clearing
is allowed. Malformed declarations and identity/version mismatches refuse
without writing a report.

Success, failure and cancellation can release only the exact admitted workspace
resources. All remain `caller_asserted`; failure and cancellation leave the task
unfinished. An omitted, false or unknown stop/effect declaration, an unknown
outcome, or an expired lease records the caller's observations without claiming
settlement. The live lease is checked again before the transaction finishes.
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
Old contracts, expired attempts, scope violations and unknown effects retain
their recovery protection.

A controlled adapter follows its separately delegated authority and may attest
only to runs it controls. CI can verify an artifact; it cannot prove a process
stopped. See [delegated execution authority](../integrations/delegation-execution-auth.md).
