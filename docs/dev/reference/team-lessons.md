# Team lessons

Team lessons preserve a member’s practical guidance with authenticated attribution,
explicit confirmation and reversible sharing. People act through their own Agents;
the observation workspace reads the resulting records.

## Trust and provenance

A task-bound active session supplies the source task and source session. The
current covering Agent delegation supplies its active person binding. The server
sets the attributed person from that binding. Each Agent proposes guidance for
its own person. Recording another member’s guidance goes through that member’s
own Agent.

`original_words` contains the exact text submitted by the attributed person’s
Agent, including whitespace and line endings. Reads label this provenance as
`recorded_by_authors_agent`: “original words recorded by the author’s Agent.”
This authenticates the submitting relationship and task/session provenance.
It records the Agent’s account of the person’s words rather than a verified
transcript. The person confirms the candidate through their own Agent.

The server checks format, UTF-8 byte limits, current authority, person binding,
session ownership, task binding and record versions. Semantic judgments remain
with people and their Agents. The service performs deterministic validation and
storage with the existing command gate.

## Record

Migration 67 adds `lessons` and `lesson_feedback` under tenant/project row security.
A lesson stores its source task/session, attributed person, recording actor/client,
stage, status, version and database creation/update times. Its typed `content_json`
contains these fields:

| Field | Meaning | UTF-8 byte limit |
| --- | --- | --- |
| `title` | Short lesson title | 200 |
| `original_words` | Exact text recorded by the author’s Agent | 8192 |
| `when_to_use` | Declared applicable circumstances | 2048 |
| `not_applicable` | Declared exceptions | 2048 |
| `observed_result` | Result reported at the time | 2048 |
| `scope.path_prefixes` | Canonical relative path prefixes | 20 entries, 1024 each |
| `scope.task_tags` | Declared task tags | 20 entries, 128 each |
| `dependencies.documents` | `{path, sha256}` document digest references | 20 entries |
| `dependencies.decision_ids` | Recorded decision references | 20 entries, 128 each |

Text fields require meaningful length and accept line breaks. Paths use relative
slash-separated components. Document digests use 64 lowercase hexadecimal
characters. Scope contains at least one prefix or tag. Dependencies are typed
references declared by the Agent; automatic applicability matching and dependency
freshness evaluation belong to subsequent lesson consumption work.

Stages are `candidate`, `project` and `team_rule`. Status is `active` or `revoked`.
Withdrawal is terminal. Every applied transition increments the lesson version.

## Commands through MCP

Use `awr_team_command` with the usual workstream command header from current
`work.prepare`, a stable `request_id`, and one of the operations below. All five
operations require an own active Agent session bound to the selected source task,
with `session_id` and `expected_session_version` in `args`.

| Operation | Purpose and additional arguments | Authority |
| --- | --- | --- |
| `lesson.propose` | Record a candidate using `content` | Own person and task session, session maintenance |
| `lesson.confirm` | Confirm a candidate, optionally replace complete `content` first | Attributed person through their own Agent |
| `lesson.revoke` | Withdraw a lesson at any stage | Attributed person through their own Agent |
| `lesson.mark_not_applicable` | Record feedback with a nonblank `reason` up to 2048 bytes | Current member’s Agent with task/session authority |
| `lesson.promote` | Promote a project lesson to a group rule | Explicit supervisor duty plus assignment authority/delegation |

The last four operations also require `lesson_id` and `expected_lesson_version`.
The command parser rejects supplied person/source fields and extra fields at every
nested level. Confirmation applies only to active candidates; promotion applies
only to active confirmed project lessons. Concurrent edits require a refreshed
version. Exact retries return the original metadata receipt. Reusing the same
request ID with changed intent returns `IdempotencyConflict`. Current authority
is checked again before replay.

## Reads and member agreements

Use `awr_team_query` with `op: "lessons.list"` and an explicit `work_id`. Optional
`workstream_id` must match that task. `limit` (1–100), `cursor` and
`max_context_bytes` provide bounded pagination. Every read rechecks project and
workstream authority; a cursor is bound to the reader and selected task.
`work.observe` includes a compact `lessons` collection with continuation guidance.

Candidates are visible to their attributed person first. Their `work.inbox`
contains an individual `confirm_lesson` item with type “确认经验”, lesson identity,
version and the existing work inspection selector. Existing recovery/review items
retain their own slots. Each page obeys its item bound, including tasks with many
candidates. Confirmation removes the candidate’s waiting item and shares the
lesson with authorized readers of its source task. Stage describes maturity;
current project/workstream permission still controls disclosure.

The four member agreements are represented directly:

- Attribution comes from the authenticated person binding and persists on reads.
- The contributor benefits first through private candidate inspection and confirmation.
- Reads carry `use_for_personnel_evaluation: false`; feedback counts describe lesson applicability.
- Members can question applicability; the attributed person can withdraw the lesson.

Withdrawn records expose identity, state and timestamps while clearing `content`,
including original words and dependency references. Feedback reasons stay in
protected storage; reads expose the count. Audit events and replay receipts hold
bounded identifiers, versions, stage, status and provenance labels. Lesson text,
feedback reasons and tool payloads remain outside audit records. Error responses
use the existing bounded error categories.

## Verification boundary

PostgreSQL regressions exercise authenticated synthetic Agent bindings, session
and project isolation, candidate visibility, confirmation edits, promotion,
withdrawal, feedback, exact replay, conflicts and paged inbox reads. These are
server contract tests; native-client and product acceptance remain separate.
