# Team collaboration

AWR lets several people — and the Agents they work with — coordinate on one
shared project: everyone sees the same tasks, claims the work they are doing,
reports progress as they go, and hands finished work to an independent reviewer.
You drive all of this from your normal Agent conversation; you do not need to
copy task IDs around in a browser.

> **Availability:** this article describes the Team service, which is still in
> development. It is not part of the published 0.5.0 personal MCP server. The
> capabilities below apply to a team project hosted by your project
> administrator, not to a personal local setup.

For the underlying ideas (work items, contracts, evidence, sessions), read
[Concepts](concepts.md) first. For the single-person day-to-day loop, see
[Your daily workflow](daily-workflow.md).

## The collaboration model at a glance

Four ideas carry the whole model:

- **Workstreams and shared work.** The remote project is the shared work
  authority — the one place where tasks, dependencies, progress and results
  live. Your local checkout is for code and tests only; do not initialize a
  second local task ledger to replace the remote project.
- **Claiming.** Before an Agent starts execution work, it takes a live claim on
  the task, under its session. Another person's live claim must not be
  replaced, so two Agents cannot silently work on the same thing.
- **Review.** Delivery and acceptance are separate facts. An authorized,
  independent person reviews submitted work before it is accepted. A second
  Agent belonging to you is not an independent reviewer.
- **Handoff.** Sessions are durable: they survive MCP reconnects, carry
  checkpoints and recovery state, and can be picked up again — by you after a
  break, or inspected by a teammate — without losing context.

## Connect once

Ask your project administrator for a project MCP URL and your individual
credential. Every participant uses their own credential, and repository
permissions stay separate from AWR permissions — joining the AWR project does
not grant Git access, and vice versa.

An authorized administrator adds you and sets your project role and workstreams
through the service's access tools. They generate a personal credential in a
protected local operator channel and register its hash. You receive the endpoint
and credential through a private channel. If you lose the credential, it can be
explicitly replaced. See the [Agent workflow](../dev/integrations/team-agent-workflow.md)
for the administrator's supported entry points.

Add the remote MCP server in any Agent client that supports remote MCP over
**Streamable HTTP** with **Bearer authentication**:

| Setting | Value |
| --- | --- |
| Transport | Streamable HTTP |
| Server URL | Your project endpoint, for example `https://team.example/v1/projects/example/mcp` |
| Authentication | Bearer token in the HTTP `Authorization` header |
| Credential | Your administrator-provided personal credential |

These are connection settings, not a configuration-file format or shell
command; each client has its own settings and version requirements. If your
client supports only local stdio MCP, this URL cannot be used directly — use a
client version or integration that supports the remote transport.

A few safety rules for the credential:

- Use your client's supported secret settings or a trusted private Agent input.
- Never put the credential in a URL, a shared conversation, or the repository.
- Reconnect after changing configuration, then open your local code checkout in
  the Agent.

## Give your Agent the outcome

You work in the same conversation you use for development. Describe the outcome
and let the Agent coordinate, for example:

> Use the connected AWR Team project to implement the issue API. Refresh the
> current tasks and resume my existing work, or claim an eligible task matching
> this request. Read the contract, dependencies and latest checkpoint before
> editing. Keep progress and evidence in AWR, submit the tested change as a PR,
> and request independent review.

The Agent reads shared work, claims an eligible task and keeps the project
updated as it develops locally. If it hits a real permission, dependency or
ownership conflict, it reports that to you instead of working around it.

## What the Agent does for you

The Agent talks to the Team service through two discovered MCP tools,
`awr_team_query` for reads and `awr_team_command` for writes. Every query and
command rechecks your current permissions, so a role change takes effect
without anyone editing local config.

A typical session flows like this:

1. **Start or reconnect.** The Agent queries `capabilities` to confirm its
   identity and permissions, then `work.next` to resume your own sessions or
   discover visible unfinished work, following the returned `next_query`. (On
   older servers without `work.next`, it falls back to `workstreams.list` and
   scoped `work.list` / `work.search`.)
2. **Prepare.** For a selected task, `work.prepare` returns the current
   contract, required specifications, dependencies, recovery state and a
   context hash — one snapshot the Agent must actually consume before editing.
   `work.prepare` and `work.observe` may also return one short `guidance`
   item (its condition, factual basis, next action and reevaluation trigger);
   it is advice, not execution rights.
3. **Claim.** The Agent inspects existing claims with `claim.inspect`, then
   acquires or renews a live claim under its session.
4. **Execute.** With `execution.prepare` and a fresh `execution.start` response
   carrying `execution_authorized=true`, the Agent may perform one execution
   within the declared scope. Your machine runs the code and tools — the
   service never executes anything itself. A claim alone is not execution
   admission.
5. **Report progress.** The Agent sends `session.checkpoint` with the consumed
   context hash, next action, open loops and a short `progress` summary,
   batching updates when a phase or test completes, a blocker changes, your
   input is needed, or delivery is ready — not on every tool call. Keep in mind
   that checkpoint summaries are shared with authorized work readers, so raw
   sensitive logs do not belong in them.
6. **Finish or stop.** `execution.report` records a terminal outcome with
   version-bound evidence. Claims are released and sessions ended only after
   active work and unknown outcomes are settled.

## Review and acceptance

When work is ready, the Agent submits evidence and requests review through
`delivery.submit_and_request_review` or `review.open`. An authorized,
independent person reviews the delivery, and authorized finalization follows
the project's acceptance policy.

Keep these as separate facts: implementation, verification, the GitHub merge,
and AWR acceptance. Merging a PR is not the same as AWR accepting the task, and
your own second Agent does not count as an independent reviewer.

## Handoff and recovery

Handoff works because sessions and checkpoints live on the shared project, not
in one person's chat history:

- Reconnecting MCP does not end a durable work session or renew its lease.
- Recovery state and registered deliveries take precedence over old checkpoint
  instructions, so a returning Agent trusts what actually happened.
- If a write times out, the Agent inspects the original command result before
  retrying exactly; replayed receipts are historical facts, not permission to
  run effects again.
- If execution effects are unknown (an interruption mid-task), they require
  inspection and authorized reconciliation. Creating a new session does not
  bypass that requirement.

Two practical limits to know: AWR cannot wake an idle Agent (lease renewal uses
your host's scheduling, separate from checkpoints), and a blocked or waiting
phase is a report — it does not create a wait item or renew a claim by itself.

## Observer workspace

Observer shows the same project's goals, work and evidence. Your connected Agent
coordinates the work. Use Observer to:

- Inspect project goals, ownership, progress and recorded results.
- Copy the service address and inspect effective permissions in **Connect Agent**.
  Copying the address creates no session or claim.
- Review **Activity**, which separates authenticated access records from
  committed development history. Ordinary members see their own authorized
  activity; project auditors can filter by member or task. Audit records
  exclude credential values, conversations and arbitrary tool input/output,
  and request metadata has bounded retention — it is not a permanent
  compliance archive.

Member administration uses authorized access tools. The current **Members**
browser page indicates its availability explicitly. The central service enforces
administrator and review permissions for every client.

For local progress maps and standalone exports, use
[Project Progress Overview](../../tools/project-progress-overview/README.md).

## Next steps

- New to AWR? Start with [What is AWR](what-is-awr.md) and the
  [Quickstart](quickstart.md).
- Connect your own tools via MCP: [MCP](mcp.md).
- Working with Agents day to day: [Agents](agents.md) and
  [Your daily workflow](daily-workflow.md).
- Something not working? See [Troubleshooting](troubleshooting.md).
