# Panorama fact semantics

This shared contract aligns Observer's AWR-PPO-010 derivation with AWR-PPD-011 fact snapshots.
Both sides use one meaning for **核验过**, **标了完成**, **等待**, and **压着**. The web presents
these facts and routes each action to the person's own Agent. All calculations are deterministic.

The core accepts canonical facts that a caller has already read. Adapters bind criteria to their own
current evidence; a task-wide test list alone leaves criterion coverage unknown. `undefined` means
“missing”; an explicit `null` records absence where absence is meaningful. Both preserve unknown
rather than zero.

## Rule table

The final column is machine-checked by `src/core/overview/__tests__/semantics.test.ts`. Rule keys
serve as developer documentation identifiers; product labels come from the UI vocabulary.

| Term | Product word | Meaning | Rule identifiers |
| --- | --- | --- | --- |
| `verified` | 核验过 / Verified | Every completion criterion has current, passing evidence bound to that criterion; the latest current review is approved by an explicitly independent reviewer whose identity differs from the author; and that reviewed result is integrated. All three are required. | `all-criteria-supported` `independent-review-approved` `integrated` |
| `claimed` | 标了完成 / Reported complete | The report declares completion but no result was submitted. A review round establishes that a submission exists. This state differs from verified completion. | `report-complete` `no-submitted-result` |
| `returned` | 复核退回 / Returned by review | The latest current review round was rejected. An open decision about rework may coexist with this state. | `latest-current-review-rejected` |
| `review` | 等复核 / Waiting for review | A result was submitted and its latest current review round is open. | `latest-current-review-open` |
| `running` | 在做 / In progress | Work has started, its Agent is active, and the latest report/activity is newer than the caller's stalled threshold. | `started` `agent-active` `activity-before-caller-threshold` |
| `stalled` | 停着 / Stalled | Started work has no qualifying report/activity for at least the caller's threshold. The caller supplies both the read time and threshold on every call; there is no default. An outside or dependency wait remains “waiting”, as in the prototype. | `started` `inactivity-at-least-caller-threshold` |
| `waiting` | 在等 / Waiting | An open decision needs attention, started work has an unsatisfied dependency, or a recorded outside party still owes a reply. An unstarted item with unfinished upstream work is “planned” and stays outside dependency waits. | `open-decision-or-unsatisfied-dependency-or-external-party` |
| `ready` | 可以开始 / Ready | Completion criteria are written, work is before its start, and all known dependencies are satisfied. | `acceptance-written` `not-started` `dependencies-satisfied` |
| `planned` | 没开始 / Not started | Work is before its start and either has no written completion criteria or still has unfinished upstream work. | `not-started` `acceptance-or-upstream-incomplete` |
| `decisionWait` | 等一个决定 / Decision wait | An open decision whose baseline still applies to the task. | `open-current-decision` |
| `dependencyWait` | 等上游 / Dependency wait | A started consumer has an unsatisfied provider dependency. | `started-consumer` `unsatisfied-dependency` |
| `reviewWait` | 等复核 / Review wait | The latest current review round is open. If no reviewer is named, the waiter remains unknown or a review-responsibility queue; person selection stays factual and unguessed. | `latest-current-review-open` |
| `externalWait` | 等项目外的一方 / Outside wait | The task carries a recorded wait for a person, group, or party outside the project. | `recorded-external-wait` |
| `behind` | 压着 / Holding up | The unique set containing the waiting task itself plus every direct and transitive dependent, matching the prototype's `behindSet`. Shared downstream tasks are counted once. Unknown nodes/edges are excluded; if any relevant graph fact is incomplete, the total is unknown rather than a partial number presented as complete. | `include-waiting-task` `include-transitive-dependents` `deduplicate` `exclude-unknown` |
| `countDetails` | 数字与明细 / Count and details | Every non-null count is constructed from the exact detail array exposed beside it: `count === items.length`. An incomplete result has `count: null`, confirmed `items`, and `unknownItems`; callers can say “未读全” without discarding known rows. | `count-from-detail-array` `unknown-count-is-null` |
| `coverage` | 覆盖说明 / Read coverage | An unread or truncated collection differs from an empty collection. Coverage is reported per input plane and separately for status and waits. A read complete with zero rows is the only empty-read case that produces a known zero. | `unread-is-not-empty` `partial-count-is-unknown` |
| `unknown` | 未知 / Unknown | Missing required fields and invalid references degrade to unknown. Unknown tasks without identities and unknown graph edges do not enter “压着” sums. A valid task whose status is unknown still counts as an existing downstream task because its identity and graph edge are known. | `missing-is-unknown` `unknown-is-not-zero` |
| `chains` | 链路 / Blocker chains | Decision, review, and outside waits form roots. Review waits share a root only for the same explicit reviewer identity. A grouped root unions its members' impact sets and sorts by known impact descending, then oldest elapsed time. Dependency waits remain visible as task relationships. | `non-dependency-roots` `group-review-by-explicit-person` `union-impact` `sort-impact-descending` |
| `commitments` | 受影响的承诺 / Affected commitments | Commitments come from tasks in the confirmed impact set. Nodes and connectors expose the root, tasks, dependencies, and commitments; footer counts reuse the same detail objects. Missing commitment data yields an unknown count with confirmed commitments retained. | `affected-task-commitments` `unknown-commitments-are-not-empty` |
| `approvals` | 批准拆解与确认经验 / Plan approvals and lesson confirmations | Plan-breakdown approvals and lesson confirmations are two more pending kinds beside waits and handoffs, so one person's pending queue can hold all five. Each item names one recipient; a missing recipient is unknown for every person, never the viewer and never somebody else. Approvals are not task-bound: the me, lead, and department queues include one only when its project lies inside the view's project scope, and a missing project keeps that count unknown instead of dropping or guessing the item. The top view's decision metric counts decisions only. | `approval-kinds-explicit` `approval-recipient-explicit` `approval-project-scoped` `decisions-exclude-approvals` |
| `people` | 按人汇总 / Per-person summary | Each explicit person has four lists: non-dependency waits, pending handoffs, and plan/lesson approvals addressed to them; prototype in-progress work; waits on their owned tasks; and dependency waits for results they own. | `explicit-person-identity` `pending-nondependency-and-handoff` `prototype-in-progress` `owned-waits` `owed-provider-results` |
| `identity` | 同一个人 / Same person | Two present person ids must match exactly. A missing id belongs to neither the viewer nor an “other person” bucket, and unknown reviewer identities remain in separate queues. | `explicit-equal-only` `missing-is-neither-self-nor-other` |
| `views` | 四个视角 / Four views | Me, lead, department, and top each expose four metrics in prototype order. Viewer ids, project scopes, and periods are caller inputs. Missing scope data produces unknown metrics and coverage rather than an empty organization. | `four-metrics-in-prototype-order` `caller-scopes-and-periods` `unknown-scope-is-not-empty` |
| `vcount` | 周期核验数 / Period verified count | Period counts include verified tasks whose explicit verification time lies inside the caller's inclusive start/end bounds: a task verified exactly at the start and exactly at the end is counted. Both bounds being inclusive, the caller must pass non-overlapping periods, keeping adjacent periods one millisecond apart. Report or approval time is a separate fact. Previous periods use the identical rule. | `verified-only` `verified-at-in-inclusive-window` `caller-passes-non-overlapping-periods` |

## Status precedence

A task has one derived status. Verified, returned, open review, and reported-complete-without-result
are resolved first, in that order. Outside waits come next. An unstarted task is then planned or
ready; a started task with an unresolved dependency waits; otherwise decision/activity facts select
running, stalled, or waiting. A completed report with a submitted but unclassifiable result is
unknown rather than being relabelled as active work. Missing facts retain their explicit unknown
state.

This precedence intentionally allows a returned task to carry a decision wait, matching the
prototype's rework example. A started consumer can continue unrelated work while remaining in the
waiting list; the list itself leaves the amount of stopped work unstated.

## Wait ordering and elapsed time

Waits are sorted by known “压着” count descending, unknown counts after known counts, then known
whole elapsed days descending, unknown elapsed times after known times. Stable input order breaks
remaining ties. Future or missing start times yield an unknown elapsed time instead of zero.

## Chain, people and view shapes

The result exposes chains, per-person summaries, and view metrics beside the original task and wait
lists. Chain nodes carry explicit root/task/commitment kinds and red-root tone data; connectors carry
block, dependency, and commitment effects. All traversal is iterative and stable on cycles.

The four view orders are:

- **Me**: current-week verified, running Agents, waits on my tasks, pending items for me. The me-view pending queue is scoped to my project's tasks, like the lead view: a reviewer or decider is waited on for other people's tasks too, not only for tasks they own.
- **Lead**: current-week verified, prototype in-progress work, waits, pending items for the lead.
- **Department**: projects, current-month verified, cross-group waits, pending items for the viewer.
- **Top**: goals, all verified work with a claimed comparison, blocker chains, decisions for the viewer.

Department project catalogs and top goal catalogs are explicit view inputs, so units without tasks
still appear with empty task details. Cross-group comparison uses explicit task/member group ids;
outside waits cross a known project group by definition. Every current/previous period and claimed
comparison retains its own count/detail invariant. Pending queues in the me, lead, and department
views include plan approvals and lesson confirmations scoped by the item's own project; the top
view's fourth metric stays decisions-only.

## Open alignment point for AWR-PPD-011

“核验过” here requires an explicitly independent review and integration; an approve decision, green
check, completion claim, or merge string alone is insufficient. If the service's fact snapshot uses
“已核验” for only a selected completion receipt, the two tables remain inconsistent and the
supervisor must resolve that difference rather than either side silently weakening this rule.
