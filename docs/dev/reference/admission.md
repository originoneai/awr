# Plan admission

Admission decides whether a work item may be *executed*: claimed, or have an execution registered
and started. It is separate from the other facts about a work item. A work item's definition can be
recorded and its intent adopted without anyone having admitted it, an admitted work item is not
completed, and the source calling an item `ready` or `in_progress` grants nothing. By default no
project is affected: admission is opt-in per ledger source and every existing project behaves as
before.

What the program checks is provenance and currency: that a decision exists, is approved and still
describes the current scope, that the obligations of the declared [work definition](work-definition.md)
are present, and, where the policy asks for it, that the review came from a session other than the
proposer's. It does not judge whether the reasoning behind a decision is sound, and it does not
verify who an agent is. Admission changes no completion rule.

## Declare a policy

A ledger source declares the policy in its options. Only YAML ledgers can, because work definitions
are YAML-only.

```toml
[[sources]]
domain = "ledger"
role = "primary"
path = "work.yaml"
adapter = "yaml-ledger-v1"

[sources.options.admission_policy]
version = 1
mode = "governed_enforce"          # or "legacy_observe" (the default when no policy is declared)
scope = "all"                      # required for governed_enforce; or { tags = ["core"] }
independent_review_tags = ["security", "permission"]
supervisors = ["lead"]             # optional: agents allowed to approve a containment decision
```

- `legacy_observe` reports the admission state of work in scope and never blocks.
- `governed_enforce` blocks the entries listed below for work in scope until it is admitted.
- `scope` must be explicit. `"all"` governs every work item of the source; `{ tags = [...] }` governs
  only work carrying one of those tags. A missing scope governs nothing.
- Work carrying one of `independent_review_tags` needs its approval from another session than the one
  that proposed it. The program proves only that the sessions differ, not that the reasoning is independent.
- `supervisors` limits who may approve the containment of a parent's children, see
  [controlled decomposition](containment.md); without it any attributed agent may.
- An unknown version, an unknown field or a scope-less `governed_enforce` is refused when the
  source is configured. If a stored policy later becomes unusable, work of that source fails closed
  with `admission_policy_invalid` instead of silently becoming ungoverned.

To migrate a project, declare `legacy_observe` first and read the `admission` block of `work show`
and `work prepare` until the gaps you care about are closed, then switch to `governed_enforce`.
Switching back is an edit of the same option. The policy is part of what a decision binds, so
changing it makes earlier decisions stale.

## Record a decision

`awr work admit --input admission.json` (or the MCP tool `awr_work_admit`, which takes the same
fields as arguments) records one event. The request names the work, who decides, the revision the
caller read, a stable request key, the contract fingerprint the decider reviewed (from `work
prepare` or `work assess`), and the decision. Who decides is exactly one of an active `session` or a
declaring `agent`, which is attributed like a checkpoint's agent and not verified. A governed
flow therefore costs one extra call for work that needs no independent review. Only sessions can be
shown to differ, so work that needs independent review is proposed and approved by sessions.
The decisions are:

| `decision` | needs | meaning |
| --- | --- | --- |
| `proposed` | `judgment` | a request for a decision; it is not a decision and does not admit |
| `approved` | `judgment` (inline) or `proposal_event` | admits the work for the scope it binds |
| `denied` | `reason` | refuses admission; a later decision replaces it |

The `judgment` records, as bounded attributed text, the contribution to the goal, why the work is
necessary, the alternatives or reuse that were checked, the deliverable, the exit condition and the
responsible party. An approval answers either a proposal (`proposal_event`) or is given inline when
no independent review is required.

Each event binds what it judged: the work contract, the declared definition, the declared examples
with their discrimination method, and the effective policy. The store reads those inside the same
transaction as the write, so a decision made against a contract that has since moved is rejected
with `SourceConflict`, and a write based on an older project revision is rejected as a stale write.
A repeated request with the same `request_key` and the same content returns the original receipt;
the same key with different content is refused.

An approval is refused, with its own reason, when:

- the work needs independent review and no other session proposed it (`admission_review_not_independent`);
- the obligations of the declared purpose have gaps (`admission_obligations_unmet`), see below;
- the proposal it answers no longer describes the current scope (`admission_stale`).

## What the state means

`awr work show`, `awr work prepare` and the MCP tools `awr_work_get` and `awr_work_prepare` add an `admission` block
for governed work (work outside the scope gains no key). It lists `state`, the `diagnostics` and four
facts that stay separate: `recorded`, `adopted`, `admitted` and `completed`.

| Diagnostic | Meaning |
| --- | --- |
| `admission_policy_invalid` | the source's policy cannot be used; every governed entry is refused |
| `admission_missing` | no decision is recorded for the work |
| `admission_pending` | a proposal is recorded and no decision answers it |
| `admission_denied` | the latest decision refuses admission; the reason is in the detail |
| `admission_stale` | the approval no longer matches the current contract, definition, examples or policy; the detail names which |
| `admission_obligations_unmet` | the definition lacks what the declared purpose owes; the detail lists the gap codes |
| `admission_unknown` | the state could not be established, for example because a recorded decision cannot be read; it blocks like the others and is never reported as a pass |

Decisions are recorded per branch, like management assessments: a session on another branch needs
its own decision. Completed and cancelled work owes no diagnostics because it cannot be claimed.

Entry obligations follow the declared purpose. Research may investigate an unknown and a document may
be an explicitly incomplete draft, so neither owes the examples or responsibility an implementation
owes. Work that declares no definition is judged as an implementation, so it cannot lower its own bar
by saying nothing. A decision request is not an obtained decision: a work whose purpose is a decision
becomes admissible for collecting it, and completing it still needs the decision itself.

## Where it is enforced

| Path | With governed work that is not admitted |
| --- | --- |
| claiming (`session start --claim`, `work claim`, MCP session start) | refused with `AdmissionRequired` |
| `ready` and `work prepare` | the work is not ready; `next_action` is `resolve_admission_before_claiming` |
| registering an execution | refused |
| starting a registered execution | refused again, because the scope can move in between; nothing is dispatched |
| reading, checkpointing, releasing a claim, ending a session, finishing or reporting an execution, `doctor` | never gated |

All claim paths share one readiness check, so the CLI, MCP and direct runtime use cannot differ, and
importing proposals or editing the source status to `ready` or `in_progress` changes nothing: the
decision is the only thing that admits. A host that cannot present the decision cannot be
downgraded to ungoverned dispatch.
