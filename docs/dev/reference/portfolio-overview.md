# Portfolio overview read

`portfolio.overview` returns one member-scoped read summarizing every project
the current member can read. Observer pages that compare projects use this
single read instead of issuing one project-scoped read per project. The reply
carries recorded facts; derived judgments stay in the caller's derivation model
(`overview-fact-semantics.md`).

The operation is read-only, admits no task selectors, and grants no execution or
approval authority. One page carries at most 200 projects; the reply names the
truncation and a resume cursor. A project whose summary fails to read comes back
as `not_read` with a reason and leaves the other projects intact.

## Entries

| Surface | Entry | Input |
| --- | --- | --- |
| Agent tool | `awr_team_portfolio_overview` (MCP, read-only annotation) | `protocol_version`, optional `limit`, optional `cursor` |
| Browser | `POST /v1/web/portfolio/overview` (HttpOnly cookie session) | the same JSON object as the Agent tool |
| Store | `WorkstreamReadStore::portfolio_overview(tenant, bearer, registered, limit, cursor)` | as above plus the operator-registered project ids of the tenant |

The tenant comes from the transport, never from the request body. The read
covers projects that are operator-registered in the serving instance and in that
tenant; a project the operator has yet to register stays unenumerated even when
a membership exists, matching the member project list of the web entry.

## Browser session entry

The browser route follows the other POST routes under `/v1/web/*`: an exact
allowlisted Origin, an allowed Host, the `x-awr-web` guard, and a live HttpOnly
cookie session are required. Login binds the session to the credential's tenant
on the server; the browser cannot supply or change it. Session revocation,
credential revocation, and permission changes therefore affect the next read
without a bridge cache retaining the old member result.

MCP and the browser route call the same service function and serialize its
returned JSON value directly. A successful browser response is the same object
as the MCP tool's `structured_content`, with no additional `ok` or `data`
wrapper. Two separate calls can have different `data.as_of.at_unix_ms` read
clocks; every other field follows the same implementation and disclosure rules.
The browser request body accepts only `protocol_version`, `limit`, and `cursor`.

Observer exposes this read as `GET /api/team/portfolio`, forwarding the optional
`limit` and `cursor` query parameters to the POST body. It revalidates the cookie
session on every request and does not cache the portfolio result. Its response
keeps the service object unchanged under `result` and adds only the bridge read
time, coverage, and freshness metadata. Authentication or upstream failures
return no previous `result`.

## Field table

Every row names the exact stored source, the rule, and the disclosure boundary.
Works are counted only inside the member's readable scope; everything else in a
project contributes at most a count.

| Field | Definition | Source of truth | Why it stays inside the disclosure boundary |
| --- | --- | --- | --- |
| `project_id`, `project_key` | The project's stable id and its registered key. The key is the registered display name; a richer name fact is absent. | `awr_team.projects` (`id`, `key`) | Listed only for projects carrying the member's own membership row. Projects without that membership stay outside the candidate set. |
| `project_status`, `coordinator_epoch`, `project_revision` | The project's current service facts, as every project-scoped read reports them. | `awr_team.projects` | The same values an authorized project read already returns to this member. |
| `responsible_person` | `null` in this version. A per-project owner fact is unregistered so far; the supervisor of the latest period update appears inside `latest_update` instead. Nothing substitutes for it. | — | Absent, so there is nothing to disclose. |
| `directory.registered` | Whether the project has a saved directory plan; disclosed to every reader whose project item is `complete`. The project's grouping labels; visible to any reader who enters the project, granting no access and merging no people across projects. Owner decision of 2026-11-19: partial-scope members see the department too. | `awr_team.project_directories` (`version > 0` row) | The project's grouping labels; visible to any reader who enters the project, granting no access and merging no people across projects. Owner decision of 2026-11-19: partial-scope members see the department too. |
| `directory.group` | The leaf department the project is attached to: a department in `project_department_ids` that no other department names as `parent`. Plans recorded before the hierarchy field carry no `parent_id`, so every department is a leaf and the registered department is the group. | `project_directories.plan_json` (`departments[]`, `project_department_ids[]`) as written by `directory.apply` | The project's grouping labels; visible to any reader who enters the project, granting no access and merging no people across projects. Owner decision of 2026-11-19: partial-scope members see the department too. |
| `directory.departments` | The ancestor chain above the group, outermost first. Before the hierarchy migration lands every chain is empty — the reader leaves it empty rather than inventing one. | `plan_json` department `parent_id` chain | The project's grouping labels; visible to any reader who enters the project, granting no access and merging no people across projects. Owner decision of 2026-11-19: partial-scope members see the department too. |
| `works.total` | Count of works visible to the member in the project's active snapshot. | `work_contracts` joined `workstream_snapshot_ownership` at `projects.active_snapshot_id` — the same rows `work.list` pages | Only streams the member can read (and, for Agents, delegated tasks) are counted; see `hidden_work_count`. |
| `works.not_started` | Visible works with a contract and without a `work_runtime` row. | `work_runtime` absence | Visible subset only. |
| `works.in_progress` | Visible works with a runtime row that are neither completed, delivered-reported nor retired. | `work_runtime.state` | Visible subset only. |
| `works.delivered_unverified` | Visible works whose latest progress report says `delivered` and that stay uncompleted. A completed work stays outside this bucket. | Latest `checkpoints.progress_json->>'phase'` per work over current-ownership sessions (`session.checkpoint` producer; declared phases in `feedback::PROGRESS_PHASES`) minus `work_runtime.state='completed'` | Checkpoints are read only for the visible works of member-visible projects. |
| `works.verified` | Completed works whose selected completion carries an independent acceptance basis (rule below). | `work_runtime.state='completed'` plus the `work.completed` event payload whose `receipt_id` equals `work_runtime.selected_completion_id` | Event payloads are read only for visible works; the fields used are the independence basis, with source text left out. |
| `works.completed_without_independent_review` | Completed works whose selected completion lacks an independent acceptance basis. A payload without a basis is recorded and stays unverified. | Same rows, negated rule | Same boundary as the verified count. |
| `works.retired` | Visible works whose definition is `archived` or `cancelled`. They stay in `total` exactly as `work.list` pages them, and outside the active buckets. | `work_contracts.definition_state` | Visible subset only. |
| `works.hidden_work_count` | How many works of the project's active snapshot lie outside the member's read scope. A number only: ids, titles, states, owners and stream names stay out. | `workstream_snapshot_ownership` rows minus the member's readable scope | The count names and describes nothing hidden. |
| `waits.decision` | Open ordinary decisions on visible works, aggregated per work. | `wait_items` with `decision_key IS NOT NULL AND state='open'`, aggregated per work — the same rows `decisions.list` reads | Counted for visible works only. |
| `waits.review` | Open review rounds on visible works; a work with several open rounds counts each round while staying one work. | `review_rounds` with `state='open'`, aggregated per work — the same rows `review.rounds` reads | Counted for visible works only. |
| `waits.dependency` | Visible works whose latest progress report is `waiting_dependency`. | Latest `checkpoints.progress_json->>'phase'` per work | Same checkpoint boundary as the delivered fact. |
| `waits.oldest_open_unix_ms` | Earliest start among the open decision, review and dependency waits of the visible works; `null` when none is open. | `wait_items.created_at` (decision), `review_rounds.created_at` (review), and for a dependency wait the first checkpoint of the current contiguous `waiting_dependency` run | A timestamp of an already-counted wait. |
| `waits.external` | Unreported. Structured outside-project waits lack a recorded source so far; the reader leaves the field out rather than guessing. | — (AWR-PPD-050) | Absent. |
| `latest_update` | The current record of the project's latest period: period kind and key, dates, status, the supervisor's one-sentence summary, recording time, author, update id. `null` when the project has yet to record a period, and for Agent readers without a Project- or Workstream-scope read delegation (the same authority `updates.list` requires); humans always match `updates.list`. Nothing imitates it. | `project_period_update_current` joined `project_period_updates`, latest by (`starts_on`,`ends_on`,`period_key`,`id`) — the first row `updates.list` returns; the sentence is bounded by the record itself (200 characters, no control characters) | Disclosed exactly when `updates.list` succeeds for the same reader; the summary travels with the record the reader may already read row by row. |
| `read` | `complete`, or `not_read` with `reason` `no_read_authority`, `no_active_snapshot` or `read_failed`. A `not_read` item carries only `project_id`, `project_key` and `read`: no status, epoch or revision, which would tell a reader without read authority how busy the project is. | Entry admission per project (below) | The project id appears because the member's own membership row exists; members already know their memberships. |

A per-project summary appears only for projects the member could enter: live
credential, active tenant, actor and membership, workstreams enabled, a valid
catalog, the membership ceiling containing `work.read`, at least one current
read grant on a catalog stream, and — for Agent actors — one live covering read
delegation (project, workstream or task scope) resolving to a nonempty
navigation scope. These are exactly the checks of the project entry admission
(`authorize_project_entry`), evaluated in one batched statement. A member
project failing them returns as `not_read`; a project without the member's
membership stays unenumerated.

## Independent acceptance

The verified rule is the one defined for period fact snapshots (AWR-PPD-011,
branch `kimi/ppd-011-fact-snapshots`), read in this order from the selected
`work.completed` payload as the current producers write it:

1. `independence_kind` — written by `work.complete` (`reviews.rs`) with
   `team_independent`, `agent_review` or `simulated_member_independent` from the
   review decision, `personal_self_review`, `ordinary_confirm` or `unspecified`
   otherwise. Only the first three verify.
2. else `team_independent_acceptance` — boolean the same producer writes (false
   for agent and simulated-member reviews, which step 1 already classified).
3. else the legacy payload of `review.rs` (`receipt_id`, `evidence_id`,
   `policy`): the policy names one of the two independent review policies.
4. an empty basis stays unverified while remaining recorded.

The function lives in one shared module (`completion_independence`) so the
period snapshot writer and this reader stay aligned. Its tests seed payloads
copied from both producers, with wording of the ruling left out.

## Persistence and isolation

The read adds no tables. Schema 66 adds one permissive `SELECT` policy to each
table the two batched statements aggregate — the project, membership, mode,
catalog, grant, authorization, binding, person, contract, ownership, runtime,
session, event, checkpoint, wait, review-round and period-update tables: rows
are readable when the transaction set `awr.tenant_id` and
`awr.portfolio_read`, both transaction-local. Existing policies stay
unchanged: with the marker unset every table behaves exactly as before, and
only this read sets the marker, inside the same repeatable-read transaction
whose admission statement computes the member-visible project list. Every
batched statement then filters that list itself (`project_id = ANY(visible)`
or the registered-project filter); the policies only release the
single-project pin of the isolation policies. Write paths stay untouched, and
a fresh database without the marker sees no behavior change on any other code
path.

Isolation reaches the data a reader can touch: the only value cast from stored
payload text (the acceptance flag) is type-checked before the cast, so a
corrupt flag reads as "no stated flag" and falls through to the policy instead
of failing the read. Every other conversion in the two statements runs on
strongly typed columns. A statement that still fails — a genuine database
error — fails the whole read; corrupt data the reader can reach must not.

## Read budget

One call is five client-server statement exchanges for any number of member
projects up to the page bound: `BEGIN`, one statement binding the two
transaction-local scope markers, one batched statement resolving credential,
memberships, projects, modes, catalogs, grants and Agent delegations, one
batched statement aggregating works, completions, waits and period updates for
all visible projects at once, and `COMMIT`. The markers need their own
statement because row-level policies read the settings at execution time,
after the statement started. The counting test measures statement initiations
on the wire; the driver runs every string query of every read in this crate as
prepare-then-execute, which is a driver constant outside this read's control.
Per-project loops are a defect: the counting test pins the exchange count at
forty projects to at most five.

## Pagination and ordering

Projects are ordered by `project_id` and returned one page at a time (default
and maximum 200). The cursor binds the reader's identity, grant versions and
page parameters, so a revoked or changed grant expires it. `truncated` and
`total_projects` describe the whole visible set, and consecutive pages neither
overlap nor drop a project — both properties are pinned by tests.
