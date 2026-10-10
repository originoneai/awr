# Work with a team through your Agent

Connect the Agent you already use to the team's remote MCP service. Give it a
task in your normal conversation. The Agent reads shared work, claims an eligible
task and keeps the project updated as it develops locally. Inspector is an
optional view of the same project; signing in or claiming through a website is
not a prerequisite.

This guide applies to the development Team service. It does not describe the
published 0.5.0 personal MCP server.

## Connect once

Get a project MCP URL and an individual credential from the project administrator,
then clone the code repository. Each participant uses their own credential.
Repository permissions remain separate from AWR permissions.

In Inspector, an authorized administrator can open **Members**, add a member,
choose their project role and workstreams, then copy the one-time personal Agent
connection instruction. The instruction includes the endpoint and that member's
credential. Give it only to the intended member through a private channel. The
administrator cannot retrieve its plaintext after clearing the issuance panel;
a lost credential can be explicitly replaced.

Use any Agent client that supports remote MCP over **Streamable HTTP** with
**Bearer authentication**. AWR does not select a default Agent or model.
Add a remote MCP server using your client's settings:

| Setting | Value |
| --- | --- |
| Transport | Streamable HTTP |
| Server URL | The project endpoint, such as `https://team.example/v1/projects/example/mcp` |
| Authentication | Bearer token in the HTTP `Authorization` header |
| Credential | Your administrator-provided personal credential |

These are connection settings, not a universal configuration-file format or
shell command. Each client has its own settings and version requirements.
Use the client's supported secret settings or a trusted private Agent input;
do not put the credential in a URL, shared conversation or repository. Reconnect after
configuration changes and open your local code checkout in the Agent.

If a client supports only local stdio MCP, this URL cannot be used directly;
use a version or integration that supports the remote transport and authentication.

Use the remote project as the shared work authority. A local checkout is for code
and tests; do not initialize a second local task ledger to replace the remote
project. Server database access is unnecessary.

## Give the Agent the outcome

For example:

> Use the connected AWR Team project to implement the issue API. Refresh the
> current tasks and resume my existing work, or claim an eligible task matching
> this request. Read the contract, dependencies and latest checkpoint before
> editing. Keep progress and evidence in AWR, submit the tested change as a PR,
> and request independent review.

The Agent handles coordination in the same conversation used for development.
It should report an actual permission, dependency or ownership conflict when
encountered. The user does not need to copy task IDs from Inspector or manually
maintain progress in a browser.

## Agent workflow

Use the discovered `awr_team_query` and `awr_team_command` tool schemas and the
[Team protocol reference](../reference/team-workstream-service.md) for exact
arguments. Each query/command rechecks the caller's current permissions.

| When | Agent action | Recheck when |
| --- | --- | --- |
| Starting or reconnecting | Query `capabilities` to confirm identity and permissions, then `work.next` to resume your own sessions or discover visible unfinished work. Follow the returned `next_query`. On older servers without `work.next`, use `workstreams.list` and scoped `work.list` / `work.search`. | Progress, resolved waits, claim conflicts, identity, project, scope or ownership changes. |
| Preparing a selected task | Consume `work.prepare`: current contract, required specifications, dependencies, recovery state and context hash. Resume only an active session owned by the current actor and client; otherwise use `session.start` when appropriate. | Contract, dependency or source changes; incomplete context. |
| Assigning existing work | A supervisor queries `task.assignees`, refreshes `work.prepare`, then uses `awr_team_command` with `op="task.assign"`, an eligible `assignee_person_id` and the current `expected_responsibility_version`. Leave unassigned work available for self-claim when appropriate. | Eligibility, responsibility, contract or permission changes. |
| Taking assigned or available work | After starting or resuming an owned session, use `awr_team_command` with `task.accept_assignment` for a current pending assignment, or `task.claim_available` for unassigned eligible work. Both take responsibility and its live coordination lease atomically. Renew with `claim.renew` before expiry. | Conflict, blocker, stale version, lease expiry or revocation. |
| Beginning effects | Use `execution.prepare` and a fresh `execution.start` response with `execution_authorized=true` for one execution within the declared scope. The Agent's host runs code and tools locally. A claim alone is not execution admission. | Permission, lease, scope or execution state changes. |
| Reporting progress | Use `session.checkpoint` with consumed context hash, next action, open loops and a short `progress` summary. Batch updates when a phase/test completes, a blocker changes, user input is needed, or delivery is ready. Declare known `client_info` at session start or the next checkpoint. | Material work changes; client/model/capability changes. |
| Finishing execution or stopping | Use `execution.report` only for a terminal outcome, then follow inspection/reconciliation. Attach version-bound evidence. Release claims and end sessions after active work and unknown outcomes are settled. | Interruption, handoff, unknown effects or changed context. |
| Delivering | Submit evidence and request review through `delivery.submit_and_request_review` or `review.open`. An authorized independent person reviews; authorized finalization follows acceptance policy. | PR head, contract or artifact changes; returned review. |

Assigning or claiming existing work keeps its published requirements unchanged.
Use a planning draft when the goal, scope, acceptance criteria or dependencies
need to change; discover that tool's schema before editing a candidate.

`work.prepare`, `work.snapshot` and `work.observe` share the inbox's current
repository-neutral candidate, check, review, integration and source-publication
facts. Each returns one short `guidance` item: condition (`when`), at most three
factual basis entries (`because`), one action and reevaluation trigger
(`recheck_on`). Keep following `action.op`; `action.query`, when present, supplies
the authorized detail selector. Read authority and each action's covering grant
are checked separately. Advice grants no rights and never changes the required
context hash. Unknown effects, expired execution and incomplete required context
retain priority; employee intake keeps its own session and responsibility chain.
Small preparation budgets may omit optional advice without removing required
context. `work.observe` bounds the complete UTF-8 JSON response with
`max_context_bytes` (default 65,536); increase the budget on `ResponseTooLarge`.
Passed external checks remain observations, never AWR approval. For an unknown
integration, inspect its original request instead of repeating the effect.

An elapsed coordination claim is separate from unresolved execution effects.
After an explicitly settled terminal execution under the current coordinator,
consume current context and follow the appropriate fresh-claim intake action.
Never renew an expired claim or resume the old run. Unknown effects, resource
barriers, changed claim bindings and required recovery still block new effects;
review, dependencies, user waits and permissions are checked independently.

## Current team inbox

Supervisors use `awr_team_query` with `op="work.inbox"` after checking
capabilities. Omit work, workstream and session selectors. The same entry serves
members, reviewers and deliverers within their actual permissions. Follow one
item's `next_query` to inspect its current condition before taking action.

The inbox projects persistent assignment, blocker, review and repository-neutral
delivery facts. Each task has at most one current primary item, with a condition,
at most three short basis entries, one action and a reevaluation trigger. Provider
events do not become approval or instructions. GitHub and webhooks are optional.

Page until `next_cursor` is null, including empty pages. Use `item_key` to deduplicate;
refresh from the first page after a relevant change. Handled conditions disappear
and changed conditions receive a new key. Cursors expire when reader, source or
authority bindings change. `limit` bounds evaluated visible tasks; optional
`max_context_bytes` bounds the complete JSON response (default 65,536 bytes).
Reduce the page size or increase the budget if the response is too large.
Display titles are limited to 160 Unicode characters with `title_truncated`
reported explicitly; the task's required context remains in `work.prepare`.

An item is advisory: reading it reserves nothing, acknowledges no unknown effect,
and grants no execution, review, repository or publication authority. Commands
check their own current permissions and versions. Source publication stages here
are recorded facts; `delivery.source.status` checks actual source synchronization.
Read-only members can inspect unresolved effects or waits, including paused work,
without receiving an action grant. An author is never offered its own review as
a reviewer decision; the selected review's remaining independence checks still
run at command admission.

Use the [feedback contract](../reference/team-session-feedback.md) when advertised
in capabilities. Batch reports with checkpoints, not every tool call or page
refresh. Lease renewal is separate: use host scheduling if available and renew
before expiry without creating extra reasoning turns. AWR cannot wake an idle
Agent. A blocked/waiting phase describes a report; it does not create a wait item
or renew a claim. Submit usage only from host data with known scope/source; an
unsupported host leaves it absent. Checkpoint summaries are shared with authorized
work readers; keep raw sensitive logs out of them.

For `evidence.submit`, discovery exposes the exact closed argument shape and
required session/version, `payload` and `dirty_tree` fields. Submit the measured
`artifact_text` bytes (or legacy `artifact_hex`); the server computes their
`artifact_digest`. Do not invent `artifact_sha256` or other argument fields.
Generic payloads remain arbitrary JSON; Agent-completion evidence additionally
needs the current successful execution's input/output bindings. An artifact digest
or reported readiness alone does not verify, approve or complete the work.
For `review.inspect`, use the actual `review_round_id`; `evidence.inspect` requires
the actual `evidence_id`. These are record selectors, not event or checkpoint IDs.

For tasks with configured neutral delivery, follow `delivery.neutral.inspect`
before requesting review. Publish actual code/tests/report through the authorized
project channel, select the exact candidate, and persist every manifest entry as
evidence with its `payload.delivery_candidate_digest`. Report-only submissions
return `ReviewSubmissionIncomplete`. The review's `submission.artifacts[].query`
provides exact `artifact.content` selectors for the original version; use them
instead of discovering peer files. Readability grants no approval or repository
authority. Approval rechecks the current binding, and changed submissions need
fresh evidence and review. See [neutral submission details](pr-delivery-review.md#neutral-delivery-before-business-review).

When a supported input mismatch is rejected, HTTP and MCP return a bounded
`invalid_field` JSON pointer and `constraint`. Unknown names and submitted values
are never echoed: an unexpected evidence argument identifies `/args` and lists
the operation's known `expected_fields`. Correct the field using current
discovery; after an uncertain command outcome, inspect its original request first.
Diagnostics are advice on a rejection, not a second validator or an approval.

Persist a stable request ID and exact command envelope before a write. After a
timeout, inspect the original `command.inspect` result before an exact retry.
Replayed receipts are historical facts, not permission to run effects again.
Unknown execution effects require inspection and authorized reconciliation;
creating a new session does not bypass that requirement.

For an admitted `caller_managed` execution whose current contract selects
`independent_workspace_v1` or `independent_workspace_v2`, include the explicit
`workspace_settlement` declaration to settle a terminal `execution.report`, even
while the lease is live. `execution.start` and an owned current
`execution.inspect` return `terminal_reporting` with the recorded bindings;
its null stop/effect assertions must be filled from actual observations. V1
requires a live claim. V2 also permits only elapsed time on the unchanged original
current claim; it never allows further execution after expiry. Use the admitted
workspace and input digest, a measured non-sensitive environment digest, and the
current owned claim/fence/lease versions. Inspection advice uses a currently
covering execution delegation, even when a separate delegation grants the read;
it is omitted after that execution grant is revoked or the run binding changes.
Assert `executor_stopped` and
`no_external_effects` only when both facts hold. For success, compute
`output_digest` from the exact artifact bytes you will submit as evidence and
retain those bytes unchanged. An omitted or unknown declaration records the
outcome conservatively and requires recovery; it does not settle the workspace.
Recheck after contract, lease, execution or effect changes. See the
[ordinary workspace report](../reference/team-execution-settlement.md#report-an-ordinary-workspace-outcome).

Reconnecting MCP does not end a durable work session or renew its lease. A second
Agent belonging to the same person is not an independent reviewer. Keep
implementation, verification, GitHub merge and AWR acceptance as separate facts.

## Publish an execution workspace declaration

When `capabilities.planning.supported_candidate_codecs` advertises
`awr-team-planning-v4`, an authorized planner can include an
`execution_settlement` in a normal `awr_team_planning_draft` change. For example,
these are the execution fields of a complete `TaskDraft`, alongside its usual
identity, goals, acceptance, workstream and definition fields:

```json
{
  "completion_policy": "caller_managed_execution_and_simulated_member_review",
  "execution_settlement": {
    "mode": "independent_workspace_v1",
    "workspace_id": "workspace-backend"
  },
  "scope_paths": ["src/api"],
  "verification_requirements": ["Run the API persistence regressions"]
}
```

The other supported workspace review policy is
`caller_managed_execution_and_agent_review`. New workspace declarations require
the settlement, a nonempty scope and nonempty verification requirements.
`workspace_id` is a stable opaque identity, not a filesystem path, process ID,
member identity or permission grant. Unknown fields or modes and explicit `null`
are rejected. Older ordinary Agent-review definitions without settlement remain
valid; they do not silently opt into workspace settlement.

Use the normal **draft → preview → approve the current digest → publish** flow.
An approved source activation compiles the declaration into the current contract;
consume fresh `work.prepare` to check its policy, scope, settlement and checks
before execution. Drafts and previews alone do not change active work.

On existing work, omitting optional settlement, hard-rule or verification fields
retains their authoritative source values. To replace one, include its exact
current value in `before` and the requested value in `after`. An explicit prior
must match even when `after` omits that field. The publisher validates the final
composed contract before writing, and refuses stale priors or incomplete retained
contracts. Settlement removal is unsupported. Changing a candidate invalidates
its previous approval; inspect and approve the new digest before publishing.
Existing independent human-review policies cannot be downgraded by a planning
edit or a forged `before` value.

Planning V4 preserves older V1–V3 wire representations and digests when their
fields are omitted. An ordinary workspace contract uses contract V3; the exact
named simulated-member policy uses contract V4. These versions describe the
declaration, not extra caller authority.

**Current boundary:** planning publishes declarations; it grants no execution or
review authority. Check `capabilities.simulated_member_review` before using the
authenticated review flow below. Review approval alone does not finalize work,
execute a repository integration or certify complete business acceptance.

## Review work as distinct simulated members

Under the explicit `caller_managed_execution_and_simulated_member_review` policy,
distinct authenticated Agents may represent different simulated team members,
including members controlled by one physical operator. Each execution/reviewer
Agent must have exactly one active binding to an active member with explicit
`member_identity.kind=simulated_member`, an active Agent anchor and current project
membership. Model labels, two windows and a controller reference cannot establish
this identity or grant permissions.

`execution.prepare` captures the executor's actual member, actor, client, binding
ID and membership versions. `execution.start` checks that attribution again.
Evidence submission and review opening retain that executor origin and capture
their own origins. A human supervisor may submit or open a round with fresh human
attribution; missing member metadata remains explicitly unspecified. Changing a
binding later or opening another member's bundle cannot replace its original
author. Legacy records without these snapshots cannot enter this review flow.

Use `review.decide` with an explicit `agent_review` membership grant and a current
Review delegation. The reviewer must differ in **member, actor and client** from
every original executor, evidence submitter and round opener. The successful
decision reports `approval_basis=simulated_member_independent_review`, while
`human_approval` and `team_independent_acceptance` remain false. Ordinary Agent
review and existing human-review policies retain their own semantics.

Default action receipts contain short member attribution or a basis digest. An
authorized `review.inspect` returns the immutable round origins and full decision
basis, including the actual covering delegation and grant versions. It does not
return credentials. Check again after a binding, permission, contract, evidence or
round change.

## Finalize a reviewed simulated-member delivery

**When:** the exact artifact is available, execution succeeded with explicit
workspace settlement, and an independent simulated member approved that bundle.
**Basis:** finalization compares the stored executor, evidence submitter and round
opener with the original decision, including its member, actor, client and source
snapshot. It does not recapture those contributors from today's bindings.
An unrelated source publication may keep that review usable only when the
archived reviewed contract and the current task contract still prove the same
exact hash. Missing or corrupt history and relevant contract changes are refused;
the stored review authority retains its original snapshot.
**Action:** an authorized human or Agent uses `work.complete` or
`delivery.finalize` with the current `work.prepare` preconditions. An Agent needs
an explicit live `finalize_delivery` delegation and current delivery scope; Review
and development permissions alone cannot finalize. On an unknown result, inspect
the original command before an exact retry. Exact replay retains the receipt;
concurrent different requests cannot accept the same simulated decision twice.
**Recheck:** permission, source, contract, artifact, settlement or review changes.
Missing historical origins and mismatched original decisions remain blocked.

Completion stores the full original review basis and exposes only its short digest
and member summary by default. Its human approval flags remain false and its
execution evidence remains caller asserted. Existing human and ordinary Agent
policies retain their semantics.

`delivery.integration.prepare` can reuse the exact simulated review and settlement
for neutral **fast-forward** eligibility. Current candidate, connector, required
checks and decision versions are pinned and dispatch checks them again. Preparation
executes no Git command and grants no repository permissions. The optional adapter
must separately perform and confirm the repository effect. AWR acceptance,
observed integration and confirmed authoritative-source publication are separate
facts; this capability does not claim the complete automatic source chain,
squash/rebase content proof, cross-workstream simulated adoption or native business
acceptance.

## Use a simulated member's accepted input

**When:** a required predecessor in the same workstream was completed under the
explicit simulated-member policy, and the consumer agrees to that assurance.
**Basis:** the current selected upstream receipt must match its current contract,
evidence and original authenticated executor, submitter, opener and reviewer.
An omitted mode does not accept either Agent review or simulated-member review.
**Action:** have an authorized planner declare the edge, preview its diff, approve
the current candidate digest and publish through the normal planning tools:

```yaml
depends_on: [API-1]
dependency_acceptance:
  API-1: simulated_member_independent
```

This selects contract, bundle and source-parser V5; reviewed planning changes use
planning V5. The consumer's own completion policy stays independent: accepting
a simulated input does not replace its human-review requirement or grant rights.
V1–V4 retain their original wire semantics and hashes. Omission on an edit retains
the source map; an explicit replacement requires its exact prior map. Check
`capabilities.simulated_member_review.dependency_adoption` for support.

**Recheck:** upstream receipt selection, contract, evidence or original review
changes. `work.next`, `task.claim_available`, execution admission and finalization
use the same dependency gate. Missing original review records remain blocked;
database failures are errors, not an ordinary dependency wait. The accepted basis
keeps `human_approval=false`, `team_independent_acceptance=false` and the original
execution trust level. This capability covers same-stream inputs; cross-stream
version adoption requires its separate capability and workflow.

## Receive unfinished work

Use the actual `handoff.id` returned by the proposal receipt. An `events.list`
item ID identifies an event; it cannot be used as a session, handoff, evidence or
artifact selector. A read-only `handoff.inspect` query shows the proposal and
duties. Execute the `handoff.inspect` command with your own active, task-bound
session to receive the complete package, current prepared context and consumption
receipt. Read that response before accepting. The receipt proves delivery to
the authenticated client; it does not claim to prove the model's cognition.

The server derives the package from the sender's latest recorded checkpoint,
current contract, artifacts, dependencies, waits and unknown outcomes. A real
checkpoint is required. `package` is optional on `handoff.propose`; any supplied
factual selectors must match the server's facts. `artifact_versions` uses each
artifact's immutable content SHA256 as its version. The inspection also returns
the authorized dependency/adoption versions. Checkpoint prose and optional
branch/directory annotations remain reported information, not verified execution
or repository facts. `current_execution`, `proposed_successor` and
`successor_execution` use the same execution identity format:

```json
{"kind": "person", "person_id": "alex"}
```

An explicitly bound Agent run uses:

```json
{"kind": "agent_run", "person_id": "alex", "agent_id": "coding-agent", "binding_id": "existing-agent-binding"}
```

Use actual identities and existing bindings. Free-text plans, `kind: "agent"`,
client names and working-directory fields cannot substitute for execution
identity. Distinct, explicitly registered simulated members can collaborate with
independent credentials even under one human controller. Their simulation and
approval assurance remain explicit; no human approval is inferred.

All handoff transitions and expiry use the database clock. `now_ms`,
`prior_execution_stopped`, `prior_reconciled` and `context_reprepared` are optional
compatibility annotations; they cannot authorize transfer. Receiving commands
use the current handoff and owned session versions. Acceptance must supply
`inspection_request_id`, the original request ID of this member/client/session's
actual inspection command. When work runtime exists, set `expected_current_fence` to the current
`runtime.last_fence` returned by `work.prepare`, even when the sender is terminal
or its claim has expired. Omit it only when no runtime exists. Before acceptance,
read the complete inspection response. The server rechecks the original
execution's terminal provenance, pending effects, resource reservations and
recovery state. Disconnection, cancellation or lease expiry alone cannot prove
stopping or settlement. Reinspect after relevant context, artifact, dependency,
checkpoint, session, permission or lease changes. Unrelated audit/source cursor
movement does not invalidate unchanged selected facts. Acceptance does not replace the fresh claim and
execution admission required before the successor runs local effects.

Execution handoff preserves the original owner; responsibility handoff transfers
ownership and clears the settled predecessor's execution admission. Both fence
the predecessor and retain original execution/evidence attribution. Exact replay
returns historical facts and grants no new lease or execution. The lower-level
`HandoffStore` is a trusted operator persistence API; it is not a remote member
entry point and does not establish authenticated package consumption.

## Optional workspace view

Open Inspector to inspect task relationships, ownership, progress and recorded
results. **Connect Agent** provides client-neutral MCP settings and a project instruction;
each task also has an optional copyable brief. Copying either text creates no
session or claim. Administrator and review permissions are still enforced by the
central service, regardless of which client is used.

Administrators can manage members and project-scoped credentials in **Members**.
**Activity** separates authenticated access records from committed development
history. Ordinary members see their own authorized activity; project auditors can
filter by member or task. Audit records exclude credential values, conversations
and arbitrary tool input/output. Request metadata has bounded retention and is
not a permanent compliance archive. The connected Agent still performs task
coordination; AWR does not launch or wake arbitrary Agent applications.
