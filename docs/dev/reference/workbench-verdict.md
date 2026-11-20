# Workbench verdict rules

`tools/observer/src/components/workbench/model.ts` owns deterministic verdict selection.
`workbenchVocabulary` owns the Chinese and English sentence templates. The implementation uses only
recorded facts and fixed rules. `vocabulary.test.ts` reads the table below and checks every row,
selection expression, sentence key, and both catalogs against the implementation.

## Source and health

- `recorded-update`: when the already-read input contains a project lead's latest period update,
  show its sentence and health verbatim and name the recorded author and writing time. The reserved
  input is `latestPeriodUpdate`; AWR-PPD-012 will supply it through a later bridge read.
- `generated`: when that update is absent, select exactly one fixed sentence from the table using
  the My Work pending, waiting, and running-Agent metrics. The source line states that the sentence
  was calculated from the viewer's tasks and Agent reports.
- `generated-waiting-health`: in My Work, a known `pending > 0` selects **等你 / Waiting for you**
  and the `risk` tone. This is a deterministic waiting-for-viewer fact, not a claim that a recorded
  period update declared the project to be at risk.
- `no-update-health`: when a period update is absent and `pending` is zero or null, the health word
  remains **没有更新 / No update** with the `none` tone. Null never becomes zero; **正常 / On track**
  requires a recorded update.
- `unknown-not-zero`: a null pending or waiting total selects the unknown sentence. Unread approvals
  and partial waits map to unknown instead of zero.

## Prototype source lines

The source line is part of the prototype, not a free-form timestamp caption:

- My Work without a recorded update: `按你名下的任务和 Agent 的报告算出 · 刚刚`.
- Group lead with an update written on the read's local day: `你今天 HH:mm 在自己的 Agent 里写的 · 第 N 周`.
  The week is the ISO week of the read's viewer-local date. An update from another day uses the
  generic recorded-author line rather than claiming it was written today.
- Department: `你 M 月 D 日在自己的 Agent 里写的 · 汇总时取最差的那个，不取平均`.
- Top: `取自各部门的月更新，取最差的那个 · M 月 D 日`.

The recorded dates and times come from the update; the read's fixed timestamp supplies the lead's
current week. No wall clock is consulted. English uses the same rules and placeholders.

## My Work chain order

`orderWorkbenchChains` is the page-model ordering layer; core chain derivation is unchanged.

1. In My Work, chains whose root queue explicitly waits for the viewer come first. Identity comes
   only from `waitingOn[].person.id`; a missing viewer or person id never counts as a match.
2. Within that partition, sort by `impact.count` descending, then `root.oldestDays` descending.
   Unknown counts and ages sort after known values. Equal keys preserve the derived input order.
3. Lead, department, and top perspectives return the derived order unchanged.
4. The frame's existing first-chain fallback selects index zero, so the first viewer-waiting chain
   is the default selected chain. Invalid URL indices still clamp to that same safe default.

## Generated sentence table

Rules are evaluated in order. `pending` is the My Work pending-for-me metric, `waiting` is the
waiting-by-me metric, and `agents` is the running-Agent metric. Counts come from the same derived
objects as the visible KPI details.

| Rule | Selection | Sentence key |
| ---- | --------- | ------------ |
| `generated-unknown` | `pending === null || waiting === null || agents === null` | `unknown` |
| `generated-pending-and-waiting` | `pending > 0 && waiting > 0` | `pendingAndWaiting` |
| `generated-pending` | `pending > 0` | `pendingOnly` |
| `generated-waiting` | `waiting > 0` | `waitingOnly` |
| `generated-agents` | `agents > 0` | `agentsOnly` |
| `generated-calm` | `true` | `calm` |

## KPI note rule table

These rules produce the small factual note under a KPI. Every input comes from that metric's own
array or the current member's derived in-progress array. A note appears only when its selection is
met; incomplete facts leave the note blank.

| Rule | Selection | Sentence key |
| ---- | --------- | ------------ |
| `verified-previous` | `previousPeriod.count is a number` | `previousWeek` |
| `agents-stalled` | `stalledOwned > 0 && all activity known` | `stalledOther` |
| `waiting-oldest` | `every waiting.durationDays is known` | `waitingOldest` |
| `pending-oldest` | `every pending duration is known at read time` | `pendingOldest` |

`stalledOwned` counts the current member's stalled tasks with explicit inactive-Agent facts. Any
unknown Agent activity leaves the note blank. `waitingOldest` also names the responsible party from
the longest wait. `pending-oldest` uses wait durations when already derived and computes whole days
for handoffs and approvals from their recorded timestamps against the common read time. Future or
missing timestamps leave the note blank.

The note can carry the prototype's `.kpi .c` tone: `verified-previous` compares this week's count
with the previous week's (`good` with an up arrow when higher, `bad` with a down arrow when lower),
and `agents-stalled` is `warn` whenever it appears. The other two notes stay quiet.

The Chinese templates are:

- `pendingAndWaiting`: `有 {{pending}} 件事等你处理；你名下的 {{waiting}} 件事正在等别人。`
- `pendingOnly`: `有 {{pending}} 件事等你处理。`
- `waitingOnly`: `你名下的 {{waiting}} 件事正在等别人。`
- `agentsOnly`: `有 {{agents}} 个 Agent 正在推进你名下的工作。`
- `calm`: `现在没有等你处理的事，也没有你名下的等待。`
- `unknown`: `有些数字还没有读全；已确认的明细仍在下面。`

The English catalog carries the same keys and placeholders. The running-Agent rule requires known
zero pending and waiting totals plus a known Agent total. If any of those three totals is unknown,
the unknown sentence takes precedence over both activity and calm claims.
