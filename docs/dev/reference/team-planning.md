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

With `activate: true`, explicitly supply `impact_proven` after inspecting the
current candidate impact. Omission returns `InvalidInput` at `/impact_proven`
before publication, activation or request reservation. Publish-only requests
keep the existing omission defaults. Explicit `false` retains the intentional
activation-refusal behavior; it does not approve impact. If an earlier request's
outcome is unknown, inspect it first and preserve its original intent. Changing
a previously recorded request's confirmation requires a new reviewed request,
rather than reusing its ID with a different payload.

## Changes and task definitions

`changes` contains closed objects with `op`, `after` and optional nullable
`before`. Operations are `create_task`, `edit_fields`, `split`, `cancel` and
`archive`. The tool's `mode` is `create` or `edit` for the planning candidate;
it is separate from each change's operation. `create` starts a new candidate,
including one that changes existing work (each such change carries the exact prior
definition in `before`); `edit` changes an existing candidate, needs the `candidate_id`
returned by the earlier `create` and replaces that candidate's changes with the list
sent. A request in `edit` mode without a `candidate_id`, or whose mode is neither
`create` nor `edit`, is refused as `InvalidInput` (`/candidate_id` with the constraint
`required_when_editing_a_draft`, or `/mode` with `create_or_edit`) before any command
is reserved or candidate created.

A `create` that names a `candidate_id` which already exists never changes that
candidate. When the request is the creation the candidate recorded (the same
`changes` it was created with, suggestion IDs, spec roots, goal keys, self-approve
policy and author), it is a retry and returns the candidate's current
`draft_revision` and `candidate_digest` with `resumed: true`. Any other request is
refused as `InvalidInput` with the reason `candidate_exists_with_different_request`
before any command is reserved: `differs_in` names what differs, and the refusal
carries the existing `candidate_id`, `draft_revision`, `candidate_digest` and `state`.
To change that candidate use `edit` with its `candidate_id`; to start a new draft use
`create` with a new `candidate_id` or none.

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

`allowed_spec_roots` bounds each task's `after.scope_paths`, including its code
and test directories. For example, work in `src/queue.js` and
`test/queue.test.js` fits roots `["src", "test"]`. A requirements document alone
does not cover those directories. Paths must remain project-relative without
empty, dot or parent segments, even when roots are empty. Declaring roots grants
no additional access or execution authority.

Scope rejection returns `InvalidInput` with the fixed constraint
`project_relative_scope_bounds` and correction guidance through both HTTP and
MCP, without echoing rejected paths. A fresh rejected draft reserves no command
and creates no candidate; correct that unused request before retrying. For an
unknown outcome, inspect the original request's outcome first.

Approved source writeback persists both `title` and an explicit `definition_state`
in each YAML task. The states `draft` and `enabled` remain distinct even when the
human progress note is `status: planned`. `status` is a source note, never an
execution or acceptance receipt; `status: completed` cannot complete Team work.
If a legacy YAML task omits `definition_state`, `status: draft`, `archived` and
`cancelled` supply that definition state; other statuses (including `planned`,
`done` and `completed`) default to `enabled`. An explicit definition state takes
precedence over the progress note and must be one of the four supported values.

Publish preparation includes complete title/state metadata in
`task_definitions.json`, keyed by work ID alongside `workstreams.json`. The
snapshot manifest binds this file; activation installs it atomically with the
contracts. Re-preparing the authoritative YAML restores definitions without a
previous database projection. Legacy raw packages that omit definition metadata
retain existing definitions, defaulting new tasks to `enabled` with their external
key as title. Do not remove this file from a package that already uses it.

Definition-state changes participate in the same server-derived impact, settlement and
pending-writeback admission gates as contract changes. An affected running or
unsettled execution refuses activation before source bytes change. Pending
writeback fences affected execution admission while unrelated work can continue;
recovery rechecks the original intent and current baseline. Inactive definitions
remain readable for history but cannot be claimed or started. Navigation exposes
`definition_state` separately from runtime `state`.

Source writeback checks the current definition state against the approved before
state, using the same legacy status mapping as publish preparation. A mismatch
refuses the writeback before source bytes change. Title-only updates preserve
prepared and running executions and do not invalidate dependent work.

PostgreSQL schema 59 adds `cancelled` through a forward migration from schema 58;
existing definitions, runtime state and completion history are preserved. Upgrade
the database using the normal migration procedure before using these binaries.
Finish or explicitly close pending planning writebacks with the old binaries
before upgrading: recovery binds the original byte fingerprints and candidate
manifest, so this version refuses an older intent if re-preparation changes its
package instead of rewriting that intent. Older binaries reject the newer schema. This upgrade cannot reconstruct definitions
that older writers already omitted from source; repair those through reviewed
planning changes backed by the authoritative source.

`work_id` is the internal identity used by `required_dependencies`,
`dependency_acceptance` keys and `split_from` / `split_children` references;
`external_key` is the display identity. Both identities must remain unchanged on
edits. The Team YAML writer stores the display key as `id` and adds `work_id`
when the identities differ. YAML rows without `work_id` use `id` for both. See
[work identity and dependency keys](workstreams.md#work-identity-and-dependency-keys).

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
