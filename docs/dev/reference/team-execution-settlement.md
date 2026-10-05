# Team execution settlement contracts

**Development on `main`: contract and source publication support. Ordinary
execution settlement is a separate implementation; publishing this policy does
not automatically settle a run.** Published 0.5.1 packages do not provide this
Team contract.

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
- Until ordinary settlement support is implemented, caller reports follow the
  existing conservative recovery mechanism. This document describes the
  contract's assurance boundary, not a new process supervisor.

The intended ordinary settlement path must verify the current contract, live
lease, exact run/workspace binding, scope and absence of unresolved shared or
external effects. A successful report cannot release another run's resources
or replace independent artifact verification. Old contracts, expired attempts,
scope violations and unknown effects retain their recovery protection.

A controlled adapter follows its separately delegated authority and may attest
only to runs it controls. CI can verify an artifact; it cannot prove a process
stopped. See [delegated execution authority](../integrations/delegation-execution-auth.md).
