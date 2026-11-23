Team goal reads connect registered outcomes to the tasks supporting them. Use
`goals.list` for discovery and `goal.inspect` for one goal's task pages through
`awr_team_query` or the authenticated web query endpoint.

# Goal facts and coverage

The active source snapshot's `source_binding.json` selects the authoritative
ledger. The existing YAML adapter reads the byte-verified ledger; goal keys,
titles, status and success criteria retain their recorded values. Task contracts
provide goal references and display keys. Current task runtime state takes
precedence over the draft or planned default; cancelled and archived definitions
retain their lifecycle meaning.

Coverage follows the personal goal coverage vocabulary:

- An active or confirmed goal is uncovered when every supporting task is
  cancelled or archived, or the goal has zero supporting tasks.
- An effective task referencing an absent goal produces `unknown_goal`.
- An effective task referencing a retired goal produces `cancelled_goal` or
  `archived_goal`.
- Archived tasks stay outside the report. Cancelled tasks remain in a goal's
  task list and contribute zero coverage and orphan references.
- Other goal status values preserve their source spelling and remain outside
  uncovered and retired classifications.

These are deterministic reference checks. People and their Agents interpret
their business meaning. Goal status always comes from the ledger.

# Scope and bounded pages

Both operations accept an optional `workstream_id`, `limit` (1–100, default 20),
`cursor` and `max_context_bytes` (1–262144). `goal.inspect` requires `goal_id`;
`goals.list` uses the selector-free project entry. Project identity comes from
the authenticated transport binding.

`goals.list.data.items` is one ordered, paged collection: goal records sorted by
key, followed by visible orphan reference records sorted by task and goal.
`kind` distinguishes `goal` from `orphan_work`. Each goal includes `goal_id`,
`key`, `title`, `status`, `success_criteria`, `tasks`, `hidden_task_count` and
`uncovered`. Each task carries `work_id`, `key`, `title` and `status`.

Goal records preview up to 20 visible tasks. `task_total` and
`tasks_next_cursor` describe this separate task collection; continue with
`goal.inspect`, the same goal and workstream, and that cursor. Inspect pages
return `data.goal.tasks`, `total`, `next_cursor`, `truncated` and a separate
`orphan_work` list restricted to tasks on that page. Outer collection totals
count goal and orphan records together.

A full project reader discovers all registered goals. Narrower readers discover
goals explicitly associated with their readable workstreams or visible tasks.
Hidden tasks contribute `hidden_task_count` and coverage facts only. Their
identities and titles remain protected. Explicit workstream selection limits
both tasks and coverage to that workstream. Delegated task reads retain their
exact task scope.

The current directory schema registers project departments. Goal-specific
department registration is a separate capability: current replies carry an empty
`departments` list and `department_reason: goal_department_not_registered`.
Project grouping stays distinct from goal ownership.

# Currency and Observer routes

Responses carry the existing snapshot, epoch, revision and `as_of` envelope.
Cursors bind the reader, grants, source snapshot, catalog, selected workstream,
operation and returned facts. Source, permission or task fact changes require a
fresh first page. The byte budget covers the complete serialized envelope.
Reads preserve the existing error categories, including `Forbidden`,
`CursorExpired`, `SourceDivergence`, `SnapshotDrift` and `ResponseTooLarge`.

Observer exposes `GET /api/team/goals?project=...` and
`GET /api/team/goal?project=...&goal_id=...`. Both forward the member's session
through the existing read route. Response validation checks shapes, attribution,
page bounds and currency before releasing data. Revocation takes effect on the
next upstream read. The bridge supplies the existing freshness planes.

Goal reads expose structured goal and task facts. Source bodies, binding
locators, raw progress reports and credential fields stay within their existing
protected storage. Responses set `read_only: true` and
`execution_authorized: false`.
