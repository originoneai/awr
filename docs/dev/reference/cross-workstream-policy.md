# Cross-workstream dependency policies

**Development capability: approved-artifact disclosure; adoption is pending.**
Contract, workstream bundle, planning and source codecs V6 can express a
cross-workstream dependency. Ordinary authenticated MCP commands can publish a
reviewed artifact to an explicit consumer, which can discover and read it under
its own work permission. Reading or publishing does not adopt an artifact,
satisfy a dependency or allow execution.

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
          version_policy: fixed_delivery
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

Until authenticated adoption is connected, the ordinary dependency receipt gate
rejects this new mode. An upstream task saying "completed", or possession of its
receipt or readable body, is insufficient. The planning capability continues to report
`declaration_only: true` and `adoption_available: false`; the separate
`approved_artifact_exports` capability describes disclosure.

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
