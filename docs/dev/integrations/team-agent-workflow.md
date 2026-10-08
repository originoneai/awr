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
| Claiming execution work | Inspect existing claims with `claim.inspect`. Acquire or renew a live claim under the session using fresh preconditions. Another person's live claim must not be replaced. | Conflict, stale version, lease expiry or revocation. |
| Beginning effects | Use `execution.prepare` and a fresh `execution.start` response with `execution_authorized=true` for one execution within the declared scope. The Agent's host runs code and tools locally. A claim alone is not execution admission. | Permission, lease, scope or execution state changes. |
| Reporting progress | Use `session.checkpoint` with consumed context hash, next action, open loops and a short `progress` summary. Batch updates when a phase/test completes, a blocker changes, user input is needed, or delivery is ready. Declare known `client_info` at session start or the next checkpoint. | Material work changes; client/model/capability changes. |
| Finishing execution or stopping | Use `execution.report` only for a terminal outcome, then follow inspection/reconciliation. Attach version-bound evidence. Release claims and end sessions after active work and unknown outcomes are settled. | Interruption, handoff, unknown effects or changed context. |
| Delivering | Submit evidence and request review through `delivery.submit_and_request_review` or `review.open`. An authorized independent person reviews; authorized finalization follows acceptance policy. | PR head, contract or artifact changes; returned review. |

`work.prepare` and `work.observe` return one short `guidance` item with its
condition (`when`), factual basis (`because`), next action and reevaluation
trigger (`recheck_on`). It does not grant execution rights. Recovery and registered
deliveries take precedence over old checkpoint instructions. Small context budgets
may omit this optional advice while preserving the required context; query
`work.observe` for it when needed.

Use the [feedback contract](../reference/team-session-feedback.md) when advertised
in capabilities. Batch reports with checkpoints, not every tool call or page
refresh. Lease renewal is separate: use host scheduling if available and renew
before expiry without creating extra reasoning turns. AWR cannot wake an idle
Agent. A blocked/waiting phase describes a report; it does not create a wait item
or renew a claim. Submit usage only from host data with known scope/source; an
unsupported host leaves it absent. Checkpoint summaries are shared with authorized
work readers; keep raw sensitive logs out of them.

Persist a stable request ID and exact command envelope before a write. After a
timeout, inspect the original `command.inspect` result before an exact retry.
Replayed receipts are historical facts, not permission to run effects again.
Unknown execution effects require inspection and authorized reconciliation;
creating a new session does not bypass that requirement.

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
artifact selector. Query `handoff.inspect` with the selected work and handoff ID
to read the current proposal, package and receiver duties. Use your own active,
task-bound session for receiving commands.

The tool schema describes every nested package field. `artifact_versions` holds
objects with `artifact_id` and `version`; checkpoint, dependency and unfinished
work lists remain separate. `current_execution`, `proposed_successor` and
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
identity. Two Agents controlled by one person do not establish independent
human ownership or acceptance.

Every handoff command requires `now_ms`, the current Unix time in milliseconds.
Receiving commands additionally use the current handoff and owned session
versions. When work runtime exists, set `expected_current_fence` to the current
`runtime.last_fence` returned by `work.prepare`, even when the sender is terminal
or its claim has expired. Omit it only when no runtime exists. Before acceptance,
consume fresh `work.prepare` context and verify that the original execution has
stopped or its effects have been reconciled;
disconnection or lease expiry alone does not establish that fact. The server
rechecks these prerequisites. Acceptance does not replace the fresh claim and
execution admission required before the successor runs local effects.

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
