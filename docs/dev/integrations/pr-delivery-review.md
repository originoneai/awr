# PR delivery ∩ authorized review & completion (AWR-TMCP-031)

## Provider-neutral delivery protocol

`awr_team::delivery` defines `awr-delivery` version 1 records without network
access, a database driver or repository effects. GitHub URLs and PR numbers
are optional provider locators. `RevisionRef` explicitly distinguishes Git
SHA-1, Git SHA-256 and opaque artifact revisions; artifact-only delivery and
delivery without a change request are valid.

| Record | Meaning and binding |
| --- | --- |
| `DeliveryCandidate` | Inspectable artifact manifest and exact candidate binding. |
| `ChangeRequest` | Optional provider resource associated with that candidate. |
| `VerificationRun` | Candidate-bound outcome, result artifact and original source. |
| `ReviewDecision` | Reference to an existing AWR review round, decision and evidence bundle. |
| `IntegrationRequest` | Stable intent ID, operation, expected target and review/check references. |
| `IntegrationObservation` | Separate observed outcome, resulting target revision and candidate manifest. |
| `AdapterCapabilities` | Mechanical support; never an authenticated permission grant. |

Every candidate binding contains tenant/project/scope/workstream/work identity, candidate ID and
decimal-string version, source contract hash, complete manifest digest, optional
source revision, required check set and target precondition. Targets require an
explicit absence or exact revision expectation. Changes to any bound fact require
reevaluation of checks and review; `validate_against` verifies equality, not trust.
Manifest digests use the existing canonical hash codec with a delivery-specific
domain. Artifact locators are inspectable references; validation does not open or
rehash their contents. The authenticated storage/adapter layer must do that.

`parse_delivery_record` enforces the protocol/version, strict fields and a 64 KiB
record limit. Manifests contain 1–128 unique artifact identities; check sets have
at most 64 unique names. Counters use canonical decimal strings. Known observation
and recording times are distinct; an absent observation time remains null.
Malformed-input errors do not echo source values.

Observations retain caller-declared, operator-recorded or adapter-observed origins.
An authenticated ingress must derive/confirm those origins and assign recording
time; a parsed enum or a capability declaration cannot upgrade trust. A passed
verification describes an inspectable result, not proof that a trusted runner ran
it. A review reference must resolve to the existing AWR round, evidence and
authorized decision. An external repository approval never substitutes for that
decision. Human approval, dependency assurance and fencing policies are unchanged.

Integration intent, observed repository success and authoritative source writeback
are separate. An applied observation identifies both the actual target revision
and exact manifest; an unknown outcome remains unknown. Before retrying a mutation
with an uncertain result, reconcile its stable request and target. Webhooks may
wake a current-resource inspection; polling/reconciliation must recover missed
notifications. Neither receiving an event nor validating a record finalizes work.

This module provides pure record validation. Durable ingestion, actual provider
adapters, version-bound finalization and source-writeback confirmation are separate
integration layers; this protocol alone does not claim an operational delivery
pipeline. Existing PR operations below retain their compatibility and permission
boundaries.

## Purpose

Reuse the WS-018 evidence / review / rework / complete domain flow on the same
Team MCP management plane, with **independent** `review.decide` and
`delivery.finalize` permissions. Bind deliveries to exact repository, task
contract, PR, head SHA, test evidence, and merge SHA when applicable. Keep
GitHub PR facts visually and mechanically separate from AWR acceptance.

## Permissions

| Business action | Command ops | Who |
| --- | --- | --- |
| `delivery.submit_and_request_review` | `evidence.submit`, `review.open`, `delivery.submit_and_request_review`, `delivery.register_pr`, `delivery.observe_pr`, `work.rework` | developer / maintainer / project_admin (template). `work.rework` is author acknowledgment of a return, not independent review. |
| `review.decide` | `review.accept`, `review.return`, `review.decide` | **explicit** `independent_review` membership grant on an eligible template (developer / maintainer / project_admin). Never implied by role name, admin, or agent delegation. |
| `delivery.finalize` | `work.complete`, `delivery.finalize` | maintainer / project_admin |

Legacy membership label `reviewer` maps to the reader template and **cannot**
receive `independent_review`. Grant the developer (or higher) template plus the
flag via project-admin access apply (`independent_review: true`).

A `Review` delegation alone does not grant an Agent finalization authority.
An Agent additionally needs explicit live `finalize_delivery` delegation and a
supported current completion contract. Otherwise an authorized human/system
maintainer or project administrator finalizes through the same completion gates.

## Review command discovery and returned work

`awr_team_command` publishes a closed argument schema for each review action.
Every action below includes `session_id` and the current
`expected_session_version` as a positive JSON decimal string.

| Operation | Additional required arguments | Optional arguments |
| --- | --- | --- |
| `review.open`, `delivery.submit_and_request_review` | `evidence_id` | — |
| `review.accept`, `review.return` | `round_id`, `reason` | — |
| `review.decide` | `round_id`, `decision` (`approve` or `reject`), `reason` | — |
| `work.rework` | `round_id`, `note` | — |
| `work.complete`, `delivery.finalize` | `evidence_id`, `context_complete` | `requested_policy` (string or null) |

Unknown fields fail. Identity fields permit at most 128 UTF-8 bytes without
control characters; `note` and `reason` must be nonblank and at most 4096 UTF-8
bytes. Discovery describes request shape; authorization, artifact verification
and exact-version review remain server-side gates.

`work.rework` acknowledges a review already returned by an authorized reviewer
(`state: rejected`). An open, approved or invalidated round returns HTTP 409
`ReworkRequiresReturnedReview`; an unavailable round returns HTTP 404
`ReviewUnavailable`. MCP exposes the same structured errors. These are definite
rejections without business-state changes and carry bounded condition, basis,
next-action and recheck guidance. They do not reveal submitted values or internal
parser details. Malformed fields retain separate `InvalidInput` diagnostics.

Inspect `review.inspect` using `review_round_id` from the original `review.open`
receipt's `round_id`. Await the current authorized independent decision. When
results change before a return, submit newly verified evidence and open a new
round. After a return, acknowledge it with `note` and continue through ordinary
execution and fresh evidence/review. Recheck after the review decision, evidence
or contract changes. Acknowledgment preserves history and grants no execution,
approval or finalization authority.

## Submitting inspectable evidence

`evidence.submit` requires `session_id`, `expected_session_version`, `payload`
and the boolean `dirty_tree`. Generic evidence accepts any JSON payload. For
Agent-reviewed caller completion, provide `execution_id`, the `input_digest`
from `execution.prepare`, and a payload object with `passed: true` and
`output_digest` matching the successful `execution.report`. These bindings are
checked again at completion against the reconciled execution and review.

For a text report, send `artifact_text` directly. The service stores the exact
UTF-8 bytes after JSON decoding, including whitespace, line endings and Unicode
normalization; it does not reformat JSON reports. Binary callers may continue
using mechanically generated `artifact_hex`. Supply at most one non-null
encoding. Both produce identical evidence digests for identical bytes and
payloads, with the existing 1 MiB artifact byte limit. The whole MCP request
still has a 64 KiB limit, so keep inline reports compact.

The receipt returns `artifact_id` and `artifact_digest` (both null without an
artifact). `evidence.inspect` also returns this locator. Before opening review,
read `artifact.content` with the same `work_id`, returned `artifact_id` and
`expected_sha256: artifact_digest` to check the submitted content. This read
retains existing work access and response-size limits. The artifact digest
hashes the report bytes; it is distinct from the execution's output digest.
Encoding changes never upgrade caller evidence trust or grant approval.

## PR version evidence (v1)

`delivery.register_pr` records:

- `repository`, `pr_number`, `pr_url`, `head_sha` (40-char lowercase hex)
- optional `merge_sha`, `test_evidence_id`
- attribution: `author_actor_id`, `owner_person_id`, `executor_actor_id`
- `fact_source`: `authorized_human_github_verification` or `operator_recorded_observation`
- `observed_at`: RFC3339 timestamp of the observation

This is **not** webhook auto-sync. A URL alone is insufficient; green CI, admin
role, or `gh_merged=true` never complete the work.

`delivery.observe_pr` updates GitHub approved/merged observations while rechecking
`expected_head_sha`. A head or live-contract mismatch **invalidates** the delivery
and open/approved AWR review rounds bound to the old contract.

When a rework changes the PR head, use `delivery.register_pr` to bind the new
head and evidence before requesting a fresh review. `delivery.observe_pr` only
updates observations for its already-bound head.

## Status surfaces

`delivery.inspect` returns separate blocks:

- `github.submitted` / `github.approved` / `github.merged`
- `awr_acceptance.complete` / runtime state / selected completion id
- `cannot_skip_acceptance_via`: `pr_url_alone`, `green_ci`, `admin_role`, `already_merged`
- `webhook_auto_sync: false`

Both `delivery.inspect` and the legacy `ReviewStore::delivery_status` retain
every existing PR field and add `neutral_observation`. This compact
`awr-legacy-delivery-observation-v1` view preserves the recorded source, original
timestamp string/offset and external submitted/approved/merged assertions. It
lists the missing candidate identity/version, full scope binding, manifest,
target precondition, required checks, version-bound verification, AWR decision
and integration content proof. `acceptance_ready` is always false; existing AWR
runtime acceptance remains a separate block. No active PR yields null on both
surfaces, including after head invalidation.
The legacy store remains available only in legacy project mode; enabling
workstreams still requires the authenticated scoped query. Compatibility does
not bypass that admission rule.

Historical PR revisions support SHA-1 only. Invalid or unsupported data stays
readable in the old fields; the additive view leaves its reported revision
missing and emits a value-safe issue. A source name containing `human` or
`trusted`, a test evidence ID, or an approved/merged flag supplies no missing
proof. The view is deliberately not a `DeliveryRecord` and cannot be ingested
as a review decision or applied integration.

`LegacyPrSnapshot::change_request` can associate an existing provider locator
only when supplied with a complete current candidate/manifest and explicit
provenance. It reuses the neutral binding and envelope validators and rejects
different source revisions, contracts, targets, scopes, versions or check sets.
It never creates verification, approval or content-integration evidence.

The public `delivery-facts-v1.json` corpus covers the pure reference wire and
historical hosted PR shape. SHA-256 and no-PR/artifact-only candidates are valid
neutral cases; the old hosted shape explicitly reports unsupported/absent cases
instead of inventing a SHA-1 or PR. These are conformance boundaries, not live
repository adapters. Durable ingestion/source writeback, optional GitHub
query/poll/webhook support, local Git/artifact effects and version-bound
finalization are subsequent integration work. Webhook delivery is not required
for correctness. Pure conformance and SQL regressions do not count as complete
native-client business acceptance.

## Independence & attribution

Independence follows WS-015/018 person relations: two agents of the same person
are not team-independent. Existing human-review contracts reject Agent approvals.

Completion receipts attribute **author**, **owner**, **executor**, **reviewer**,
and **final submitter** separately (`approved_by_json` plus dedicated columns).
Failed / rejected / invalidated history is retained; only mismatched open rounds
are invalidated on head/contract change.

## Explicit Agent review

### Explicit simulated-member contract (V4)

The kernel and source publisher support the closed
`awr-team-contract-v4` policy below. Distinct authenticated simulated members
may share a physical operator; neither a model name nor a controller count
establishes their identities or grants review authority.

```yaml
completion_policy: caller_managed_execution_and_simulated_member_review
paths: [src/api.rs]
verification_requirements: [verify the delivered API artifact]
execution_settlement:
  mode: independent_workspace_v1
  workspace_id: developer-workspace
```

V4 requires that exact policy, explicit supported settlement, scoped paths and
nonempty verification requirements. V1/V2/V3 reject the new policy and retain
their existing serialized bytes, hashes and human/ordinary Agent review meaning.
Publishing the declaration selects `awr-team-workstreams-v4` and source parser
`awr-team-workstreams/4`; mixed bundles preserve every nested older contract hash.
The preview exposes `completion_policy_diffs` with the prior and proposed policy.
Missing or malformed settlement refuses preparation without changing the source.

This is contract expression support. The current Agent command path does not
yet accept simulated-member approval or Agent finalization under this policy.
Parsing or activating it does not authenticate a member, attest execution,
create review/finalization/merge permissions, or satisfy a downstream acceptance
rule. Legacy dependency policies retain their existing scope and assurance.

### Ordinary Agent review

For a contract with `completion_policy: caller_managed_execution_and_agent_review`,
use `review.decide` with an authenticated Agent identity. An administrator must
explicitly grant `agent_review: true`; an active WS-016 `Review` delegation must
also cover that Agent, client, session and work. Role names alone grant nothing.
Model names remain descriptive metadata and do not establish identity or authority.

The reviewer must differ from the round author in both actor and client, and
cannot be the evidence creator or execution actor. Two Agents may have the same
responsible person. Decisions persist `approval_basis: agent_review`, actual
actor/client attribution, `human_approval: false`, and
`team_independent_acceptance: false`. `review.inspect` exposes the same facts.

`review.accept` and `review.return` remain human-review aliases. The legacy
ReviewStore does not support Agent review. The unified command path supports
completion with the reconciled caller-execution chain below. Agent-reviewed
completion cannot unlock human-independent dependencies.
Existing human policies are unchanged and cannot be downgraded through planning.

## Recheck boundaries

Admission and effect phases re-run TMCP action auth. Completion still requires
WS-018 evidence gates. An active PR delivery must match the live contract hash;
GitHub merge is recorded but never substitutes for AWR acceptance.

### Caller-managed completion with Agent review

An explicit `caller_managed_execution_and_agent_review` contract may complete
with the developer Agent's `caller_asserted` evidence after an authorized
operator reconciles the matching successful caller receipt. The evidence must
include retrievable artifact bytes and the execution input and output digests.
A distinct authorized Agent must approve the exact evidence and current contract.
Unsettled effects, modified artifacts, stale review or contradictory receipts
refuse completion. Reconciliation does not upgrade evidence trust.

The completion receipt records `execution_basis: caller_asserted_reconciled`,
`approval_basis: agent_review`, `human_approval: false` and
`team_independent_acceptance: false`. Such a completion does not release required
downstream work by default, even within the same workstream. Existing same-stream
ordinary and human completion policies retain their behavior; cross-workstream
delivery adoption still requires human-independent acceptance. Existing contracts
are never converted automatically.

### Explicit same-workstream dependency acceptance

A consumer may explicitly accept one predecessor's Agent-reviewed, reconciled
caller result in its source ledger:

```yaml
depends_on: [API-1]
dependency_acceptance:
  API-1: agent_reviewed_caller_asserted_reconciled
```

Publishing this field produces `awr-team-contract-v2` for that consumer and an
`awr-team-workstreams-v2` bundle. Unchanged contracts retain V1 bytes and hashes.
The publish preview shows the policy before and after; it needs the same source
review and activation as other contract changes. Planning V1 preserves the map
on unrelated edits and refuses dependency edits that would orphan it.

Each mapped predecessor must belong to the same workstream. The selected receipt
must match its current contract and the exact Agent-review/caller-reconciliation
basis, with both human and team-independent acceptance false. Missing entries
retain the existing dependency rule. This does not extend WS-030 cross-workstream
adoption or authorize execution by itself.

Completion binds the exact predecessor receipt IDs. Source activation reopens
completed work whose contract changed and downstream completions that consumed
an invalidated receipt. Historical receipts remain inspectable. Clients must
prepare fresh context and repeat applicable work and review after such changes.
