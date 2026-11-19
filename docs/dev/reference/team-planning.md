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

## Source package limits

Publish preparation and activation validate the complete source package before it is
stored. Limits are inclusive: exactly the stated size or count is accepted, and one
more byte or file is refused. They are named constants selected by the server code
path, not configuration values and never fields in an MCP or HTTP request.

| Package origin | Files | Largest file | Whole package | Where it applies |
| --- | ---: | ---: | ---: | --- |
| `UPLOAD` | 64 | 1 MiB | 4 MiB | Client-submitted packages, including `ingest_publish_candidate` and its subsequent ingest |
| `GENERATED` | 256 | 16 MiB | 64 MiB | Packages generated from a registered server source: planning writeback and activation, generated ingest, activation's revalidation of a stored snapshot, and delivery-sync load/reparse |

The upload budget is unchanged. A request cannot select `GENERATED`, and choosing it
for a server-generated package does not relax path, UTF-8, duplicate-path, binding,
provenance, contract or graph validation.

The locked ledger has a separate byte limit, passed explicitly through read,
replacement and delivery-note patch functions. Local command-line paths retain
`YAML_READ_CAP = 4 MiB`. Server planning writeback and delivery synchronization use
`SERVER_LEDGER_READ_CAP = 32 MiB`.

For capacity planning, a detailed task adds approximately **3,047 bytes** to pretty
`workstreams.json`, **2,677 bytes** to the YAML ledger, and **5,961 bytes** to the
whole package. At 605 tasks, that shape gives approximately 897,817 bytes in
`workstreams.json`. These are estimates, not quotas or performance guarantees.

- Upload reaches the 1 MiB single-file limit at about **700 tasks**, the 4 MiB
  package limit at about **1,400 tasks**, and the old 4 MiB ledger limit at about
  **3,600 tasks**. The first two limits remain intentional for client uploads.
- Generated packages reach the 16 MiB `workstreams.json` limit at about **5,817
  tasks**. That is normally the next task-count-driven bottleneck. The 64 MiB whole
  package and 32 MiB ledger estimates are about **11,955** and **14,568 tasks**,
  respectively, but the 10,000-contract bundle limit precedes them.
- `task_definitions.json`, provenance, binding, the original ledger and referenced
  documents all count toward the whole package. The exact original ledger and every
  generated JSON file are retained in every snapshot.

### Size-refusal contract

A size refusal is `SourcePackageTooLarge` with HTTP status **413** and the same
structured body on MCP. It contains only deterministic metadata:

| Field | Meaning |
| --- | --- |
| `code` | Always `SourcePackageTooLarge` |
| `limit_kind` | `file_bytes`, `package_bytes`, `file_count` or `ledger_bytes` |
| `file` | Relative package/ledger path; `null` for `package_bytes` and `file_count` |
| `actual` | Observed bytes or file count |
| `limit` | Inclusive limit that was exceeded |
| `package_bytes` | Total input package bytes; for a ledger rejected before package generation, the observed ledger bytes |
| `files` | Package file count; `1` for a ledger rejected before package generation |
| `next_step` | Reduce plan content or ask a service operator to review the fixed limits |

`planning.outcome` uses the same public error mapping, and delivery synchronization
preserves this error instead of converting it to `SourceDivergence`. Path violations
remain `UnsafeSourcePath`. Responses do not include source bodies, absolute source
locations or request-supplied content.

### Preflight, capacity preview and interrupted publication

For a fresh `planning.publish` with `activate: true` and confirmed impact, the server
reads the registered ledger, applies the candidate in memory, generates the complete
package and runs the same generated-package and projection validation used by
activation. Both the original and changed ledger must fit `SERVER_LEDGER_READ_CAP`.
A deterministic size refusal happens before the command reservation and before the
publication commits: no publish receipt, candidate-state change or reserved command
receipt is left behind. The candidate can be edited, reviewed again and retried. An
existing reserved or completed request keeps its immutable payload and original
recovery behavior; preflight does not weaken request-idempotency checks.

`planning.preview` adds `package_capacity` without taking a source writer lock or
writing any state. It observes the same candidate and registered source as the diff.
An available observation contains:

| Field | Meaning |
| --- | --- |
| `status` | `available` when the package was generated, otherwise `unavailable` |
| `largest_file` | Package-relative `path`, `bytes` and the generated per-file `limit` |
| `package_bytes`, `package_limit` | Complete generated package bytes and its inclusive limit |
| `file_count`, `file_limit` | Generated file count and its inclusive limit |
| `ledger_bytes`, `ledger_limit` | Changed ledger bytes and `SERVER_LEDGER_READ_CAP` |
| `would_fit` | Whether all displayed generated-package and changed-ledger bounds fit |

If the active snapshot has no usable registered source, the ledger cannot be read
within its limit, or the candidate package cannot be generated, the existing preview
and diff still succeed. `package_capacity.status` is `unavailable`, `would_fit` is
null and `reason` gives a bounded explanation without exposing source contents or an
absolute source location. Capacity is an observation, not activation admission.

For a reserved `planning.activate` receipt with no writeback journal,
`planning.outcome` now returns `status: reserved`, `writeback: absent` and a null
result rather than `SourceDivergence`/`Unavailable`. Its `next_step` says to retry the
identical operation and payload, then gives the owner `writeback-inspect` command if
the same failure persists. After an owner closes that unactivated publication, the
completed original receipt contains the closure and its reviewed reason; an exact
retry returns that closure without activating or writing source bytes. See
[Team operator access management](team-operator-access.md#planning-writeback-recovery).

### Other fixed limits on the publish/read path

This inventory distinguishes project-wide limits from per-request, per-document and
per-work bounds. A small request body does **not** limit total project tasks when the
caller uses reviewed batches.

| Limit | When it becomes effective | Disposition in this change |
| --- | --- | --- |
| Controlled `source.content` | Default 64 KiB, maximum 1 MiB per response; the maximum can no longer return all of `workstreams.json` at about 700 detailed tasks | Unchanged; activated projections remain readable through scoped reads. Whole-source paging is a separate follow-up |
| `MAX_ARTIFACT_BYTES` | 1 MiB for one artifact, independent of project task count | Unchanged; this is not the generated-package budget |
| Generated file count | At most 250 unique referenced documents with governance, or 251 without, after the five generated files and optional governance file; one more document is refused | Unchanged at 256 files |
| Workstream references | At most 256 goal/spec references per workstream; the 257th is refused, but package file count normally refuses document-heavy packages first | Unchanged |
| Referenced-document reads | Publish preparation reads a referenced document before package validation; there is no separate pre-read byte cap in this path. The generated 16 MiB per-file limit applies afterwards. The local adapter's separate `MARKDOWN_READ_CAP` is 2 MiB | Unchanged; streaming or earlier bounded reads are outside this fix |
| Workstream bundle contracts | 10,000 accepted; task 10,001 is refused | After the estimated 16 MiB file bottleneck |
| Governance `MAX_WORKS` | 20,000 governed works accepted; work 20,001 is refused | Unchanged |
| `MAX_WORKSTREAMS` | 4,096 accepted; stream 4,097 is refused. A design with one stream per task reaches this before 5,000 tasks | Unchanged and explicitly left for a separate capacity decision |
| Graph nodes | 16,384 accepted; node 16,385 is refused | Unchanged |
| Graph edges | 65,536 accepted. One dependency per task reaches the refusal at task 65,537; a dense DAG reaches it at 363 tasks | Unchanged; dense graphs need a separate capacity decision |
| Planning request body | 64 KiB on HTTP and MCP, roughly 22 detailed changes at 3,047 bytes each | Unchanged; use reviewed batches rather than one project-wide draft |
| Draft changes | MCP schema allows 256 per draft; the domain candidate allows 1,024 | Unchanged per candidate; not a project-wide task cap |
| Delivery source note | 16 KiB per note and 32 observations/facts per work | Unchanged per work; independent of total project tasks |
| Delivery/read responses | Several envelopes are bounded at 256 KiB and list queries are paged | Unchanged; not a whole-project activation budget |
| PostgreSQL bind parameters | Protocol maximum 65,535 per statement. Activation binds at most 8 and delivery publication at most 21 per statement; parameters are not accumulated across tasks | No task-count cliff in the current implementation |
| PostgreSQL JSONB/BYTEA values | Approximately 1 GiB per value. At the byte estimates above this is roughly 180,000 tasks for a snapshot JSON value or 400,000 for one ledger BYTEA, well after the explicit package/ledger limits | Unchanged |

Every activation still stores a full snapshot, including full source text and a new
projection. Sharding, snapshot deduplication and compact JSON remain separate work;
capacity preview, pre-publication size preflight and no-journal recovery do not change
that storage model.

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

An authorized, reviewed candidate can change `independent_review` to
`caller_managed_execution_and_simulated_member_review`. Include the current
definition in `before`, an explicit workspace declaration and verification
requirements in `after`, then approve and publish the exact previewed digest.
Planning protects both policies against a later change to ordinary Agent review
or author completion. The simulated-member policy token must match exactly.

Simulated-member acceptance requires distinct active member, actor and client
identities, explicit Agent review permission, successful settled execution and
the immutable artifact and review basis. A controller may operate both simulated
members. Receipts record simulated acceptance and retain false human-approval
and team-independent-acceptance flags. Previously captured origins remain
unchanged, and existing executions need verification against the activated
contract. Each dependent task must explicitly select the applicable
`simulated_member_independent` assurance.

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
