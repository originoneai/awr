# Unreleased

Changes shipped in 0.6.0 are listed in [0.6.0](0.6.0.md).

## Added

- Closing a session after finished work: the response of `awr work complete` and `awr work cancel`
  carries one bounded `close_out` instruction (condition, basis, next step, when to look again, under
  60 tokens) while the session holds no other active claim, and `awr work complete --end-session`
  (MCP `end_session` of `awr_work_transition`) ends the session in the same request once the
  completion is durable. The `session.ended` event records `completion_event_id`. The call without
  the flag is unchanged, and the session flow, Codex and MCP documents put the End step in the main
  flow and say what editing a status field directly costs. See
  [session workflow](../integrations/session-workflow.md#finish-the-work-and-end-the-session).
- Governed work in a Team: a snapshot may declare governance (`governance.json`, written by
  `PublishPrepOptions::governance`), the Team reuses the Core admission rules on its own facts, and
  `admission.decide` records decisions attributed to the verified caller (`planning.approve`, a
  different person for independent review). Claim, dispatch and execution prepare and start answer
  `AdmissionRequired` until the decision is current; a package without governance cannot replace a
  governed snapshot. A review round records the governed scope it opened under, so a change of the
  definition, the examples or the policy of that work needs a new review and touches no other work.
  Review decisions take findings whose severity is derived from the record: for governed work a
  return needs a blocking finding, an approval resolves the open ones, a re-review covers the open
  blocking findings and the delta, and a new non-blocking finding in a later round is deferred
  instead of reopening it. New schema version 58 (`snapshot_governance`, `work_governance`,
  append-only `work_admissions`, `review_rounds.governance_binding`, append-only `review_findings`).
  See [Team governance](../reference/team-governance.md).
- Explicit requirement items and YAML task `spec_refs`, with bounded, read-only
  `awr spec check` and MCP `awr_spec_check` views of one evaluator. Checks identify
  changed or superseded item bindings, missing task associations and contradictions
  with decision `affected_keys`; applicability never infers requirement coverage.
  References reuse the existing task-contract fingerprint and evidence rules.
  Run `awr source reindex` after upgrading to refresh the changed projections.
  Markdown task bindings and cross-document replacement traversal are deferred.
  See [specification references and coverage](../reference/specification-coverage.md).
- An outcome history over what AWR already recorded about work (`awr_runtime::outcome_history`, read-only, no
  command yet): completions, human or delegated confirmations, cancellations, reopenings, supervised executions and
  usage are counted each on its own by cohort, as of an instant the caller gives. A reopening withdraws what a
  completion said, a completion under acceptance that has since changed is not counted as a pass, a missing cost or
  duration stays missing, and the report holds no rate, score or counterfactual: it lists what it does not derive.
  One project per report; the same question gives the same hash. See [outcome history](../reference/outcome-history.md).
- Optional work definitions in YAML ledgers: a work item can record its purpose, adopted intent,
  the goal outcome it advances, responsibility, examples and how its tests will show they detect a
  wrong implementation. `awr work show` and `awr work prepare` (and the MCP work tools) report each
  obligation of the declared purpose and every gap by code, only for work that declares one. The
  check is deterministic and read-only, grants no permission and changes no completion rule. See
  [work definitions](../reference/work-definition.md).
- Opt-in governed plan admission per ledger source. Under `governed_enforce`, claiming work, a
  handoff's claim transfer, a resume's claim inheritance and registering or starting an execution
  need a recorded, approved decision that still describes the current contract, definition,
  reviewed examples and policy; `awr work admit` and the MCP tool `awr_work_admit` record
  proposals and decisions as events (no schema migration). Projects without a policy are
  unaffected, `legacy_observe` reports without blocking, and recovery paths and completion policy
  are unchanged. See [plan admission](../reference/admission.md).
- Controlled decomposition for governed sources: the children of a parent are admitted only while a
  containment decision on exactly this set is current (`awr work contain`, MCP `awr_work_contain`).
  The decision binds the parent's contract, each child's definition, paths and acceptance, and the
  dependencies between them; added or removed children, widened paths and changed dependencies make
  an approval stale by name, and a child outside the parent's paths cannot be approved. Work
  definitions gain optional `side_line` and `outline` declarations. See
  [controlled decomposition](../reference/containment.md).
- Bounded reassessment for governed sources: the program classifies what a work's own records show
  (failed methods, outcomes nobody knows, unanswered waits, declared blockers) and, only for the work
  they belong to, asks for a current decision (`awr work reassess`, MCP `awr_work_reassess`) after
  three failures of one method in a row or while an outcome is unknown. A decision binds exactly
  those facts, goes stale when they change and opens no task; failures that alternate with
  successes, waits and blockers are shown and hold nothing, and recovery paths are never held. See
  [bounded reassessment](../reference/reassessment.md).
- Rework and active-effort budgets for governed work: a definition may declare a `size` (small,
  medium or large) and a `budget`; the defaults of the size apply otherwise. AWR counts rework from
  its own reopen records and effort from the claims held on the work before it uses the counters a
  host reports, and a spent budget (a fourth rework cycle under a budget of three) holds that work
  until a recorded decision deals with it: deliver with known issues, reduce scope, replan or
  escalate. An escalation is answered by someone else. See
  [bounded reassessment](../reference/reassessment.md#budgets).
- Findings for governed sources (`awr work finding`, MCP `awr_work_finding`): the severity of a
  review finding is derived from the acceptance tiers adopted for the work, the hard rules of the
  project and what the finding shows, not claimed. Only a finding that breaks a required criterion
  or a hard rule with a reproducible counterexample, or states a security or data-loss risk with its
  reasoning, blocks: it holds the completion of that work and counts as a rework cycle. Every other
  finding is a deferred record linked to the work that blocks nothing. See
  [findings](../reference/findings.md).
- One bounded instruction for governed work: `work prepare` in its action view (and `awr_work_prepare`)
  tells the earliest material gap (a definition that is missing or incomplete, an admission that is
  missing, pending, denied or stale, a containment, a gap, a reassessment, a spent budget, a blocking
  finding) with its condition, basis, action and recheck, and tells an unknown state, a denial and a
  stale judgment apart. A measurement test takes a clear small task through the binary with and
  without governance and reports calls and returned bytes against the adopted targets (one extra
  call, 2 KiB of guidance, a median extra-call ratio of 0.2). See
  [guidance and overhead](../reference/guidance.md).
- Gaps for governed sources (`awr work gap`, MCP `awr_work_gap`): something missing or changed in a
  work's intent, acceptance, design, plan or definition is recorded against that layer and closed
  the way its kind requires, by a fact lookup with its sources, a feasibility experiment that
  really ran (repairing the plan never closes it, a failed run counts), a choice by an authorized
  agent or an observed environment change. An open gap puts in question only the admission or
  containment judgments that read its layer, on that work; it keeps every failure on record, opens
  no task and holds no unrelated work. See [gaps](../reference/gaps.md).
