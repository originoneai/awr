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
  was generated from recorded facts.
- `no-update-health`: when a period update is absent, the health word remains **没有更新 / No update**.
  A generated statement about waiting work is separate from health judgement. Keep this word even
  when every derived count is zero or healthy; **正常 / On track** requires a recorded update.
- `unknown-not-zero`: a null pending or waiting total selects the unknown sentence. Unread approvals
  and partial waits map to unknown instead of zero.

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
