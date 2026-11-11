# Plan admission

Admission authorizes execution of a work item under a source-declared policy. It binds a reviewed decision to current contracts, check oracles and applicable failed baselines.

## Use it

The program checks decision provenance, approval and currency; current [definition](work-definition.md) obligations; and distinct reviewer sessions where policy requires them.

### Declare a policy

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

| Policy option | Behavior |
| --- | --- |
| `legacy_observe` | Reports state and keeps entry paths available |
| `governed_enforce` | Requires admission for the listed paths |
| `scope="all"` | Covers each work item in the source |
| `scope={tags=[...]}` | Covers work carrying a selected tag |
| `independent_review_tags` | Requires proposal and approval from different sessions |
| `supervisors` | Bounds containment approval; omission permits any attributed agent |
| Invalid version, field or missing enforce scope | Configuration rejected |
| Unusable stored policy | Fails closed with `admission_policy_invalid` |

To migrate a project, declare `legacy_observe` first and read the `admission` block of `work show`
and `work prepare` until the gaps you care about are closed, then switch to `governed_enforce`.
Switching back is an edit of the same option. The policy is part of what a decision binds, so
changing it makes earlier decisions stale.

## How it behaves

| Check | Result |
| --- | --- |
| Provenance and current scope | Bound decision |
| Definition obligations | Declared-purpose entry requirements |
| Check files and arguments | Frozen oracle fingerprint |
| Failed baseline | Evidence of pre-change discrimination where required |

### Record a decision

`awr work admit --input admission.json`, or MCP `awr_work_admit`, records one event. Supply work, observed revision, stable request key, reviewed contract fingerprint from `work prepare`/`work assess`, and decision. Identify exactly one active `session` or declared `agent`. A flow without independent review costs one extra call.

| `decision` | Needs | Result |
| --- | --- | --- |
| `proposed` | `judgment` | Pending request |
| `approved` | Inline `judgment` or `proposal_event` | Admission for the bound scope |
| `denied` | `reason` | Refusal, superseded by a later decision |

The `judgment` records, as bounded attributed text, the contribution to the goal, why the work is
necessary, the alternatives or reuse that were checked, the deliverable, the exit condition and the
responsible party. An approval answers either a proposal (`proposal_event`) or is given inline when
no independent review is required.

Each event binds what it judged: the work contract, the declared definition, the declared examples
with their discrimination method, the effective policy, and the check oracle described below.
The store reads those inside the same transaction as the write, so a decision made against a
contract that has since moved is rejected
with `SourceConflict`, and a write based on an older project revision is rejected as a stale write.
A repeated request with the same `request_key` and the same content returns the original receipt;
the same key with different content is refused.

An approval is refused, with its own reason, when:

- the work needs independent review and no other session proposed it (`admission_review_not_independent`);
- the obligations of the declared purpose have gaps (`admission_obligations_unmet`), see below;
- the proposal it answers no longer describes the current scope (`admission_stale`).

#### Freeze checks and record baselines

Admission reads each required example's `check.files` relative to the registered project root.
The binding's `oracle_fingerprint` combines stable example identifiers, command arguments, file
paths and file hashes. Each file has a 4 MiB read limit. Claim, execution, handoff and resume
checks read the current files again, so a byte change makes the approval stale with detail `oracle`.
Restore the intended checks, run their baselines, and approve the current scope again.

For `fail_before_pass_after`, including a method inherited through `inherits`, each required
example with a check owes a baseline classified as `failed_as_expected` at its current
`example_oracle`. Older records retain outcome-based semantics with `classification: legacy`.
See [baseline classifications](checks.md#classify-baseline-failures). Start a work-bound
session before claiming and run the baseline with its session ID and the current project revision:

```sh
awr work check W --example save-failure --phase baseline --session SESSION_ID --expected-revision REVISION
```

`work show` and `work prepare` expose `admission.current.oracle_fingerprint` and a `baselines`
entry per applicable example, including its state, oracle, event, log directory and recovery detail.

| Baseline state | Result and action |
| --- | --- |
| `baseline_recorded` | A recognized behavior failure or a legacy failed baseline matches this example's current oracle; its obligation is satisfied. |
| `baseline_missing` | Run the baseline command in the detail using a work-bound session. |
| `baseline_passed` | The check passed before the change. Rewrite the example or check so the baseline demonstrates the change's effect. |
| `baseline_inconclusive` | Read the indicated logs and restore readable check files within the size limit, then rerun. |
| `baseline_unrunnable` | Fix the check command, then run the baseline command in the detail. |
| `baseline_unclassified` | Review the logs and record an explained `justified_alternative` for independent admission approval. |

Baseline gaps appear under `admission_obligations_unmet`. `governed_enforce` requires them to be
satisfied before approval; `legacy_observe` reports them while keeping decisions and claims
available. Each example keeps its own baseline identity: adding a counterexample requires a
baseline for the added example, and preserves matching records for existing examples.

Legacy records with an absent or null `oracle_fingerprint` retain their original binding rules.
Work with plain examples retains that behavior. An unreadable, oversized or outside-root check file
makes the current oracle unknown and a previously frozen approval stale.

### What the state means

`awr work show`, `awr work prepare` and the MCP tools `awr_work_get` and `awr_work_prepare` add an `admission` block
for governed work (work outside the scope gains no key). It lists `state`, the `diagnostics` and four
facts that stay separate: `recorded`, `adopted`, `admitted` and `completed`.

| Diagnostic | Meaning |
| --- | --- |
| `admission_policy_invalid` | the source's policy cannot be used; every governed entry is refused |
| `admission_missing` | no decision is recorded for the work |
| `admission_pending` | a proposal is recorded and no decision answers it |
| `admission_denied` | the latest decision refuses admission; the reason is in the detail |
| `admission_stale` | the approval no longer matches the current contract, definition, examples, policy or oracle; the detail names which |
| `admission_obligations_unmet` | the definition lacks what the declared purpose owes; the detail lists the gap codes |
| `admission_unknown` | the state could not be established, for example because a recorded decision cannot be read; it blocks like the others and is never reported as a pass |

Decisions are branch-specific, like management assessments. A session on another branch needs its own decision.

Purpose determines entry obligations. Research and document work can investigate unknowns or deliver explicit drafts. Undeclared definitions use implementation obligations. Decision-purpose work may be admitted to collect a decision.

### Where it is enforced

| Path | Governed work awaiting admission |
| --- | --- |
| Claim: CLI, MCP or runtime | `AdmissionRequired` |
| `ready` and `work prepare` | Ineligible; `resolve_admission_before_claiming` |
| Register execution | Refused |
| Start registered execution | Rechecked and refused before dispatch |
| Read, checkpoint, release, end, finish/report execution, `doctor` | Available |

CLI, MCP and direct runtime claim paths share the same readiness check.

When several gaps hold a work, `work prepare` in its action view tells the earliest one as a single
bounded instruction, see [guidance and overhead](guidance.md).

The same check also holds governed work whose own records show a method failing again and again or
an outcome nobody knows, until a current decision answers those facts; see
[bounded reassessment](reassessment.md).

## Limits

- Admission, recorded definitions, adopted intent and completed work are separate facts. Source status alone grants no execution admission.
- Admission is opt-in per ledger source; projects retain existing behavior until a policy is declared.
- Admission checks provenance and currency rather than reasoning quality or actual agent identity. Completion rules remain unchanged.
- An absent scope governs nothing. Independent-review checks distinguish sessions; reasoning independence remains a reviewer responsibility.
- Agent declarations are attributed rather than authenticated. Independent review requires sessions for both proposal and approval.
- Proposals request a decision; admission requires approval.
- Completed and cancelled work is ineligible for claims and therefore carries no admission diagnostics.
- Research/document purposes have their stated obligations rather than implementation example/responsibility requirements.
- Decision-purpose completion requires the obtained decision; a request alone is insufficient.
- Registration and dispatch both check admission because scope can change between them.
- Governed dispatch requires its decision. Proposal import and source status edits preserve this requirement; unsupported hosts fail closed instead of downgrading.

## Code and tests

- [Admission runtime tests](../../../crates/awr-runtime/tests/admission.rs)
- [Check execution reference](checks.md)
- [Definition model](../../../crates/awr-core/src/work_definition.rs)
