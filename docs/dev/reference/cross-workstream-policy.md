# Cross-workstream dependency policies

**Development capability: policy declaration only.** Contract, workstream bundle,
planning and source codecs V6 can express a cross-workstream dependency. The
ordinary authenticated artifact export/adoption workflow is not available yet.
Publishing a policy does not adopt an artifact, grant access or allow execution.

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
receipt or readable body, is insufficient. The planning capability reports
`declaration_only: true` and `adoption_available: false`.

These are repository-neutral project rules. No GitHub identifier, webhook or
fabricated repository revision is part of the policy.
