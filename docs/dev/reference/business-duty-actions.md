# Business duty action wording

The `capabilities` read publishes each business duty's action ceiling in
`business_role_presets.action_ceilings`, using the same
`BusinessRole::action_ceiling` call the authorization path intersects, so the
published definitions and the enforced policy are one value. Identifiers only
travel on the wire; pages own the human wording, following the tables below.

## Publication and authority

The reply carries identifiers only; the page word lists
(`tools/observer/src/i18n/zh-CN.ts` and `en-US.ts`) turn them into product
sentences, with this document as the shared source. `role_names_grant_authority`
stays `false`: a duty name describes policy, and each request still passes its
own grant and scope checks. Every authenticated project member reads the
definitions through the existing `capabilities` query, and a revoked credential
closes that read on the next request.

## Action identifiers and page wording

| Identifier | zh-CN | en-US | Notes |
| --- | --- | --- | --- |
| `work.read` | 看项目和任务 | See projects and tasks | |
| `work.assign` | 派工 | Assign work | Supervisor duty; also needs the assignment grant. |
| `session.maintain_own` | 维持自己的工作会话 | Keep own work session | Supporting action for duties that participate in work. |
| `claim.manage_own` | 认领任务 | Claim work | |
| `execution.request_and_report_own` | 执行并汇报自己的任务 | Run and report own work | |
| `planning.propose` | 提出拆解 | Propose a plan | |
| `planning.edit_draft` | 修改拆解草稿 | Edit a plan draft | |
| `planning.approve` | 批准拆解 | Approve a plan | |
| `planning.publish` | 发布拆解 | Publish a plan | |
| `delivery.submit_and_request_review` | 提交成果并请人复核 | Submit results for review | |
| `review.decide` | 复核别人的成果 | Review someone else's results | Also needs the separate independent-review or Agent-review grant. |
| `delivery.finalize` | 确认通过复核的成果合入 | Confirm reviewed results are integrated | |
| `access.manage_project` | 管理成员和权限 | Manage members and permissions | |
| `audit.read_project` | 看项目的审计记录 | Read the project audit records | |
| `directory.manage_project` | 管理组织和部门 | Manage organization and departments | |

## Settings page rows

The settings page shows duties as ability rows; each row aggregates the identifiers
above:

| Page row (zh-CN) | Identifiers |
| --- | --- |
| 看项目和任务 | `work.read` |
| 接任务、推进、提交成果 | `claim.manage_own`, `execution.request_and_report_own`, `delivery.submit_and_request_review` |
| 复核别人的成果 | `review.decide` |
| 派工，批准和发布拆解 | `work.assign`, `planning.propose`, `planning.edit_draft`, `planning.approve`, `planning.publish` |
| 确认通过复核的成果已经合入 | `delivery.finalize` |
| 管理成员和权限 | `access.manage_project`, `directory.manage_project` |
| 看项目的审计记录 | `audit.read_project` |

## Duty names

`business_role_presets.roles` lists the six duties in declaration order. The page
wording for the names lives in the settings word list: `observer` 只读 / Read only,
`developer` 开发 / Developer, `reviewer` 复核 / Reviewer, `supervisor` 主管 /
Supervisor, `deliverer` 交付 / Deliverer, `administrator` 管理员 / Administrator.
