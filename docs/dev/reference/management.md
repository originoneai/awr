# Choosing management intensity

AWR keeps one task identity, source contract, event history and completion policy.
`lightweight` and `continuous` describe how much coordination to maintain. They do
not select different permission, source-write or completion rules. `undetermined`
means facts needed for classification have not been supplied.

Start with `awr work prepare WORK` or `awr_work_prepare`. Required context and
management requirements are returned together. `awr work assess WORK` and
`awr_work_assess` inspect the full assessment without recording a new decision.
CLI refreshes the projection; MCP verifies current sources without persistent writes.

The Agent assesses these observable properties once, and again at meaningful changes:

| Property | Lightweight condition | Continuous trigger |
| --- | --- | --- |
| Outcome | One independently understandable outcome | Multiple outcomes need coordination |
| Scope | Known boundaries and completion basis | Scope requires further planning |
| Executor | One responsible executor | Handoff or collaboration is required |
| Continuation | No deferred user/external/session wait | Work must persist across a wait |
| Work units | One independently schedulable unit | At least two independently schedulable units |
| Plan | Current plan remains valid | Scope or verification plan is invalidated |
| Execution result | Dispatched actions have known outcomes | Query/recovery is needed before another attempt |

An independently schedulable unit has its own result and dependency boundary. Two
tool calls, two files or two acceptance bullets do not automatically make two units.
An unresolved research question is not an unknown execution result: an exploration
can finish by reporting observations, limits and remaining questions. Its acceptance
must say what useful result will be delivered.

Missing observations remain null/unknown. They never count as an absence of risk.
AWR also uses source dependencies and its own wait, handoff, resume and nonterminal
dispatched-execution records. Those references are distinguished from host assertions.
The host must query an executor to establish its actual result; classification does
not probe processes, replay commands or schedule another turn.

Thirty minutes of **active execution** or three completed
**verification → rework → verification** cycles request reassessment only. They
do not automatically promote a still bounded task. Wall time spent waiting is not
active execution. Missing counters stay unknown; AWR does not infer them from chat age.

Record the observations with `awr work manage --input assessment.json` or
`awr_work_manage`. Use the current `contract_fingerprint` returned by preparation
or assessment, the owned session, current project revision and a stable request key.
For example, after examining a small bounded task:

```json
{
  "work": "WORK",
  "session": "replace-with-the-actual-session-id",
  "expected_revision": 12,
  "request_key": "bounded-plan-1",
  "contract_fingerprint": "replace-with-the-returned-fingerprint",
  "observation": {
    "observed_at": 1,
    "note": "Explain the actual scope and why these observations hold.",
    "single_outcome": true,
    "bounded_scope": true,
    "single_executor": true,
    "no_deferred_wait": true,
    "independently_schedulable_units": 1,
    "plan_valid": true,
    "outcome_known": true
  }
}
```

Replace the timestamp and all fixture values with actual observations. Do not fill
unknown properties with `true`. AWR attributes the record to the bound session's
agent and marks the observations as host assertions, not independently verified facts.
HTTP additionally requires a stable `request_id`; query an uncertain operation before
retrying. Reusing a domain request key with identical content returns the original
receipt. Different content under that key is rejected.

After scope changes, the old observation is not silently reused. The fingerprint
binds the task identity, title, summary, kind, acceptance, paths, tags, goal links,
dependencies and ordinary-completion policy. It also binds the selected goals'
identities, titles, summaries, statuses and success criteria. Keeping the same goal
ID while changing its intended result therefore requires a new assessment. Missing
linked goals remain explicit gaps even when unrelated goals exist.

Work without explicit goal links retains the existing inherited-goal selection;
it does not need duplicate goal documents. With explicit links, changes to unlinked
goals do not invalidate the observation. Reordering goal declarations or links,
and task status, next action or evidence additions alone, do not invalidate it.
The fingerprint does not bind the whole source file or global project revision.
Older receipts remain in history; a receipt with the previous binding format is
reassessed once rather than silently treated as current. When the former task
contract still matches, upgrading the binding alone does not permanently promote
lightweight work to continuous management. Fresh observations are still required:
the previous format did not establish which goal contents were assessed.

A current fingerprint establishes which contract an observation describes, not
whether the Agent's reasoning is correct. `plan_valid` remains an attributed host
assertion; the Agent must explain its reasoning against the current goal. This
binding check does not introduce execution permission or weaken completion checks.
Once continuous management is recorded, later observations retain it. Actual wait,
handoff and resume records also retain that requirement after a reply or reconnect.
The original task ID and source remain unchanged; child tasks are assessed separately.

| Maintenance | Every task | Additional continuous work |
| --- | --- | --- |
| Contract | Identity, intent, scope, current state, completion basis | Track independent work and dependency changes |
| Responsibility | Existing session/claim and permission rules | Maintain owners and handoff boundaries |
| Context | Consume required facts, source versions and hard rules | Checkpoint at waits, handoffs, interruptions and phase changes |
| Result | Preserve the actual outcome and required evidence | Track open loops, wait conditions and recovery entry points |
| Retry | Stable request identity and outcome checks | Query/reconcile unknown results before dispatching further work |

Lightweight work does not require a graph with no dependencies, per-tool checkpoints,
duplicated goals/rules, or copies of report bodies. Inherited goals must still be
explicitly resolvable. All configured hard rules still apply. A five-line production
change still needs its existing authorization and verification. A month-long wait
does not acquire engineering-test requirements merely because it is continuous.

For the frozen AssessmentEnvelope, layer mapping, explain compatibility and
read consumers, see [engineering assessment](assessment.md).

Management classification is not execution admission. Missing goals, acceptance,
source freshness or authorization must still be repaired through the existing
preparation/claim workflow. Completion continues to use the source's engineering
or explicitly configured ordinary-confirmation policy. Checkpoint and wait operations
retain their existing durable semantics; the host must follow the returned required
actions at execution boundaries.
