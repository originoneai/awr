# Delegation ∩ TMCP execution authority (AWR-TMCP-030)

## Purpose

Team MCP product permissions (TMCP-010/011/012) authorize what a membership
template may do. WS-015/016 own who is responsible and which agent grant is
active. This integration intersects those surfaces so a coding agent never
inherits a project admin's full template, and so claim coordination is not
confused with side-effect permission.

## Data consumed (not reimplemented)

| Source | Store | Used for |
| --- | --- | --- |
| Person responsibility / execution instance | WS-015 responsibility store | Eligibility explanations and binding identity |
| Agent authorization grants | WS-016 authorization store / `awr_team.agent_authorizations` | Explicit action set, scope, client/session/model binding |
| TMCP role templates | TMCP-010 `template_actions` | Upper bound for the intersection |

Claim acquire/renew/release state machines stay in `workstream_command/claims.rs`.

## Intersection rules

1. **Humans / system** — `ReaderAuthority.delegated_actions = None`; TMCP template
   actions apply unchanged.
2. **Agents** (`actor_kind == "agent"`) — after authenticate, `resolve_agent_delegation`
   loads active WS-016 grants for `subject_id = actor_id`, checks
   `bind_runtime_identity` (client / optional session / model), requires the grant
   to cover the work (or project for planning), maps `AuthorizedAction` → TMCP
   `Action`, then **intersects** with the actor's current membership actions,
   explicit grants and declared business duty ceiling.
3. **Admin's own coding agent** — even if membership is `admin` /
   `project_admin`, only explicitly delegated work actions remain (typically
   claim/session/execution/delivery). Planning publish, access manage, and audit
   read do not flow from membership alone.
4. **Sub-agent re-delegation** — still enforced by WS-016 `apply_delegate`
   (narrowing only). TMCP-030 selects one live grant covering the requested action, so independently
   issued work and review grants can coexist. It never unions their permissions.
   A matching child with strictly fewer effective actions supersedes its parent
   before selection, preventing fallback to a broader parent. Changing model, client, or session cannot revive a
   revoked/expired grant (`bind_runtime_identity`).
5. **Claim coordination ≠ side effects** — `AuthorizedAction::ClaimCoordination`
   maps only to `claim.manage_own`. `execution.*` commands require
   `execution.request_and_report_own` from `StartWork`. Claim inspect always
   reports `execution_authorized: false`. Eligibility is only
   `execution_eligibility_advisory`. Only a fresh `execution.start` response
   may set `execution_authorized: true`.
6. **Trusted executor** — the four product templates never grant trusted-executor
   attestation or execution reconciliation. Ordinary `execution.report` stays on
   the caller-asserted → pending-verify path. `execution.attest` requires
   `actor_kind == system` plus an explicit attest bit; agents cannot attest.
   Product roles never auto-upgrade to `trusted_executor`.

## Command / query wiring

- `workstream_command` and `workstream_read` call `resolve_agent_delegation` after
  authenticate and before command/query authorization.
- Source planning / writeback use the same resolve so an admin-bound agent cannot
   publish planning without an explicit, membership-allowed delegation. Planning
   admission, reservation and recovery select the requested action under one
   project grant; an earlier read/work grant cannot hide a valid planning grant.

## Explicit supervisor actions

| Delegated action | Product action |
| --- | --- |
| `assign_work` | `work.assign` |
| `edit_planning` | `planning.edit_draft` |
| `approve_planning` | `planning.approve` |
| `publish_planning` | `planning.publish` |
| `finalize_delivery` | `delivery.finalize` |

Each action is independent of `start_work` and `review`. Assignment additionally
requires the opt-in membership `assignment_grant`. Project planning mutations
require a project grant and current write access to every active workstream;
scoped task/workstream grants cannot be promoted to project authority. Include
`inspect` in the same grant when a mutation requires reading its candidate.

Operator provisioning and request admission use the same current membership
policy, including the business duty ceiling. Omitted assignment declarations
preserve existing policy, while explicit false revokes the grant. Membership
version and binding changes invalidate stale queries. Permission policy v2 adds
assignment without expanding any legacy template. No new action confers member
administration, executor attestation, recovery or a review-independence bypass.

## Recheck boundaries

Admission and effect phases of `authorize_command` both re-run
`authorize_domain_action` against the intersected scope. Lease/holder checks in
claims and executions continue to require the authenticated actor to be the live
claim holder; coordinating a claim never skips those checks.
