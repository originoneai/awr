# Cross-workstream dependency policies

**Development capability: approved-artifact disclosure and current-contract adoption.**
Contract, workstream bundle, planning and source codecs V6 can express a
cross-workstream dependency. Ordinary authenticated MCP commands can publish a
reviewed artifact to an explicit consumer, which can discover and read it under
its own work permission. The consumer explicitly selects a valid current-contract
export through `delivery.adopt`. Reading or publishing alone does not satisfy a
dependency; adoption never grants permission to run an execution.

For example, a frontend integration task can require a backend delivery:

```yaml
work_items:
  - id: UI-INTEGRATION
    title: Integrate the accepted backend API
    workstream: frontend
    depends_on: [BACKEND-API]
    acceptance: [Integration passes the agreed API checks]
    dependency_acceptance:
      BACKEND-API:
        cross_workstream:
          review_assurance: simulated_member_independent
          version_policy: current_contract
```

Both tasks must exist in the same project's source-owned workstream catalog,
with different owning workstreams. The example is a task fragment; it requires
the project's catalog and backend definition. A V6 bundle validates its complete
required-dependency graph, including duplicate edges, missing nodes, self-dependencies, cycles and
cross-stream edges without an explicit policy. Same-stream assurance modes keep
their previous meaning and cannot be used to bypass cross-stream adoption.

The policy separates two choices:

| Field | Value | Required meaning for an adoption implementation |
| --- | --- | --- |
| `review_assurance` | `team_independent` | Verify the original independent Team review; do not infer it from a status or caller claim. |
| `review_assurance` | `simulated_member_independent` | Verify explicitly simulated, independently authenticated member/actor/client review. The number of physical operators is not a gate; human approval is not implied. |
| `version_policy` | `fixed_delivery` | Bind an exact accepted historical delivery and its original contract, review and artifact. A newer upstream definition alone does not replace it. Revocation, unavailable bytes or invalid original approval still block it. |
| `version_policy` | `current_contract` | Require the adopted exact delivery to satisfy the current upstream contract and selection. Related changes require revalidation and adoption of a suitable version. |

Review assurance is separate from actual execution assurance and the consumer's
own completion policy. A caller-managed execution remains caller asserted; a
policy declaration cannot elevate it to verified execution.

The new `cross_workstream` object requires V6. Nulls, unknown fields or values,
duplicate policy keys, policies for non-required predecessors and same-stream
misuse are rejected. V1–V5 serialization and hashes retain their original meaning.
V6 consumers may also include existing same-stream assurance modes.

Planning selects V6 when either the before or after definition contains the new
mode. Omitting a policy map retains its existing source value. An explicit
replacement must include the exact prior map; changing the candidate invalidates
its approval. Source preview shows the full review and version policy so a
reviewer can assess the change before preserving writeback and publication.

Ordinary authenticated intake, navigation, execution admission and completion
share the same live adoption gate. An upstream task saying "completed", or
possession of its receipt or readable body, is insufficient. Authenticated
`capabilities.planning.cross_workstream_policy` and `approved_artifact_exports`
report current-contract adoption and explicitly exclude fixed historical
delivery. The planning-only declaration probe does not itself perform adoption.

These are repository-neutral project rules. No GitHub identifier, webhook or
fabricated repository revision is part of the policy.

## Publish and read an approved artifact

1. The publisher consumes its provider task's `work.prepare` and uses the common
   command envelope with a stable request ID. `delivery.export.publish` requires
   current `delivery.finalize` permission; an Agent also needs a live
   `finalize_delivery` delegation. Its arguments are:

   ```json
   {
     "session_id": "provider-session",
     "expected_session_version": "1",
     "consumer_work_id": "UI-INTEGRATION",
     "expected_consumer_contract_hash": "<current consumer contract SHA-256>",
     "expected_consumer_ownership_version": "1",
     "receipt_id": "<selected provider completion receipt>",
     "expected_artifact_sha256": "<approved artifact SHA-256>"
   }
   ```

   The consumer's current contract and ownership selectors come from its
   authorized `work.prepare`, not from guessed versions. The server checks the
   required cross-stream edge, selected completion, successful execution,
   canonical evidence, exact original review and stored artifact bytes. Old
   receipts without original review IDs cannot be exported; a newer approval
   cannot repair or impersonate the missing original approval.
2. The consumer calls `delivery.exports` with its own `work_id`; `limit` and
   `cursor` bound discovery. It reads a selected entry using `artifact.content`
   with `export_id` and `expected_sha256`, optionally setting
   `max_context_bytes`. `export_id` and the existing `artifact_id` selector are
   mutually exclusive. The consumer gains access to those exact bytes, never
   the provider's sources, private session or full review provenance.
3. The publisher can withdraw access with `delivery.export.revoke`, passing
   `session_id`, `expected_session_version`, `export_id` and
   `expected_export_version`. The manifest and original proof remain archived.
   Discovery marks revoked or invalid entries unavailable. If a command's result
   is unknown, inspect the original request ID with `command.inspect` before an
   exact retry. Replays retain the original receipt and grant no execution rights.

Every read rechecks both tasks' current contract/ownership, policy, selected
receipt, original review and actual bytes. This first disclosure implementation
is **current-contract-only**, including when the declared eventual adoption
policy is `fixed_delivery`. Historical fixed-version access and adoption are
not available yet. Changing either relevant contract requires a suitable new
publication; unrelated source changes do not grant extra access.

Default responses carry bounded review summaries. Explicit simulated members
can satisfy the declared simulated assurance; their human-approval flags remain
false and caller-managed execution remains caller asserted. Human Team review
retains its independent-person and trusted-execution requirements. A repository
revision is unknown unless separately verified; no GitHub dependency or inferred
SHA is introduced here.

## Adopt a current-contract input

Use the consumer's own permission and active session. An Agent requires current
development authority and a live `start_work` delegation. The member can adopt
for an unassigned task, its own task, or its own pending supervisor assignment.
Another member's assignment or execution, terminal work, unfinished execution
intents and unknown effects cannot be replaced. Cancel an undispatched prepared
intent before changing its input; dispatched or unknown effects require recovery. This lets both intake paths follow the same
sequence: select the input, then claim the pool task or accept the assignment.

1. Consume the consumer's fresh `work.prepare`. Its bounded
   `adopted_dependencies` entries show the current adoption generation, validity
   and supported version policy. Discover suitable exports with
   `delivery.exports` and consume their exact bytes with `artifact.content`.
2. Send `delivery.adopt` with the normal command preconditions and a stable
   request ID. Use the actual returned selectors:

   ```json
   {
     "session_id": "consumer-session",
     "expected_session_version": "1",
     "expected_responsibility_version": "<current responsibility version>",
     "export_id": "<discovered export ID>",
     "expected_export_version": "<discovered export version>",
     "expected_disclosure_sha256": "<exact disclosure SHA-256>",
     "expected_adoption_version": "<current adoption generation, initially 0>"
   }
   ```

   The server rechecks both source-owned tasks, the explicit V6 edge, the selected
   provider completion, original approval and actual artifact bytes. It archives
   previous adoption generations and selects the new one in the same project
   transaction as the command receipt. Responsibility and action versions
   prevent competing assignments or adoptions from overwriting each other.
3. Re-prepare, then use `task.claim_available` or `task.accept_assignment`. Only
   a fresh successful `execution.start` with `execution_authorized: true`
   permits a run. Finalization records the exact predecessor completion receipt.

Discovery and content reads report whether this exact live export is selected.
They grant no upstream source/session access and no execution authority. Every
gate revalidates the adoption: withdrawal, missing bytes, invalid original review
or relevant contract/ownership/selection changes block new work and completion.
Completion also matches the dependency receipts persisted by the evidence's
original `execution.start`. A later adoption cannot relabel an older result or
approval as using new inputs. Missing or different admission receipts require a
fresh execution, evidence and review before delivery.
For an unknown command outcome, inspect its original request ID before an exact
retry. Historical replay returns the original receipt; it cannot restore a
revoked export or grant execution rights.

`fixed_delivery` remains a valid declaration for the subsequent historical
capability, but `delivery.adopt` explicitly rejects it in this implementation.
Fixed historical proof and selective source activation are still pending; do not
silently replace that policy with `current_contract`.
