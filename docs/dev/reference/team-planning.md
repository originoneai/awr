# Team planning through MCP

Use `awr_team_planning_draft` for changes to task definitions. Assigning existing
work uses `awr_team_command` with `task.assign`; accepting an assignment and
self-claiming available work use the same command tool. See the
[team workflow](../integrations/team-agent-workflow.md).

Planning follows draft → preview → approve the current candidate digest → publish.
Editing a candidate invalidates its prior approval. Use one stable `request_id`
for each mutation; inspect `awr_team_planning_outcome` after an unknown outcome
before an exact retry. Publication and confirmed source writeback remain distinct
facts. Discovery describes inputs and does not grant authority.

## Changes and task definitions

`changes` contains closed objects with `op`, `after` and optional nullable
`before`. Operations are `create_task`, `edit_fields`, `split`, `cancel` and
`archive`. The tool's `mode` is `create` or `edit` for the planning candidate;
it is separate from each change's operation.

Each `after` is a complete `TaskDraft`, not a field patch. Existing-work changes
need the exact prior definition in `before`; creation omits it or supplies null.
Get current definitions from authorized project sources rather than client
history. Preview identifies domain, dependency and publication requirements.

| Required task field | Shape |
| --- | --- |
| `work_id`, `external_key`, `title` | Strings; preserve existing identities on edits |
| `goals`, `scope_paths`, `acceptance`, `required_dependencies` | Arrays of strings |
| `completion_policy` | Existing supported policy label, including legacy labels |
| `definition_state` | `draft`, `enabled`, `archived` or `cancelled` |

Definition states cannot forge runtime completion. Empty or incompatible values
still fail domain validation; the schema does not replace those checks.

| Optional task field | Present value | Omission | Null |
| --- | --- | --- | --- |
| `dependency_acceptance` | Map from required predecessors to acceptance modes | Retains source policy | Rejected |
| `hard_rules`, `verification_requirements` | Arrays of strings | Retains source constraints | Rejected |
| `execution_settlement` | Closed workspace policy object | Retains source declaration | Rejected |
| `workstream`, `split_from` | String | Existing optional behavior | Accepted |
| `split_children` | Array of strings | Empty default | Rejected |

Replacing optional contract fields requires their exact prior values in `before`.
For list fields, an explicit empty array requests replacement; domain validation
can reject an empty verification list for a workspace task. New source-backed
tasks also need their owning `workstream` external key.

## Dependency and workspace policies

Each `dependency_acceptance` value is one of:

- `agent_reviewed_caller_asserted_reconciled`.
- `simulated_member_independent`.
- `{"cross_workstream":{"review_assurance":"team_independent","version_policy":"current_contract"}}`.

The cross-workstream assurance also accepts `simulated_member_independent`;
the version policy also accepts `fixed_delivery`. The map must be nonempty and
refer to unique, existing required predecessors. These declarations do not
adopt an upstream delivery, satisfy a dependency or grant access to another scope.

`execution_settlement` requires `mode` and `workspace_id`. Modes are
`independent_workspace_v1` and `independent_workspace_v2`. Workspace identity is
1..128 ASCII bytes: alphanumeric first, then alphanumeric or `_ . : -`.
It is an opaque identity, not a path or an execution grant.

V2 permits only the original unchanged current claim's late terminal report.
It does not admit execution or renew a lease. New workspace or simulated-member
tasks require an explicit settlement declaration, nonempty scope and verification
requirements. Edits may retain existing declarations; publication checks them.

## Ordinary planning self-approval

The optional nullable `self_approve_policy` object has exactly two required fields:

```json
{
  "allow_self_approve_ordinary": true,
  "delivery_completion_policy": "independent_review"
}
```

This policy can permit ordinary planning self-approval for a planner who already
has approval authority. It cannot grant that authority or weaken independent
delivery review. Candidate digest binding, explicit publication and guarded
source writeback continue to apply.
