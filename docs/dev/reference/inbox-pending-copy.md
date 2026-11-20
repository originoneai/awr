# Inbox pending copy and ages

The inbox is a read-only queue. Its copy comes from the composed overview facts in
`tools/observer/src/lib/inbox-model.ts`; the page does not invent missing people, tasks,
session dates or Agent names.

## Card composition

- An answer or decision card uses the recorded question as its title.
- An assignment uses `{{who}}把“{{task}}”派给了你` only when both the dispatcher's
  display name and the task name were read. If either is absent, the card keeps the
  recorded task title or the existing unnamed-item fallback; it never leaves a blank
  clause in the composed sentence. Its from-line says the dispatcher assigned it today
  (`{{who}}今天通过自己的 Agent 派的`) when the calendar age is zero, and otherwise keeps
  the existing `{{who}} 通过自己的 Agent 派的`.
- A lesson keeps its recorded title. Its source line includes the session calendar date
  when `sinceUnixMs` is a valid, non-future timestamp. Without that timestamp it keeps
  the existing `你的 Agent 记下的` fallback.
- A focused answer names the task's execution Agent in the return-to-session hint when
  that name was read. Otherwise it uses the existing any-Agent handback wording.
- A decision may carry a separately recorded focus description. The canonical demo maps
  this text from the matching pending record while retaining the short decision question
  for the card title and copyable line. Replies without the description use the existing
  deterministic question-and-task template.

## One calendar-day rule

`inboxCalendarDays(atUnixMs, nowUnixMs, timeZone)` is the single display-age calculation
for queue rows, the oldest-wait lead and the latest-report basis. It converts both
instants to calendar dates in the viewer's IANA time zone and subtracts those dates. It
does not divide elapsed milliseconds by 24 hours, so daylight-saving transitions do not
change the result. Missing, invalid and future timestamps remain unknown.

The wording is shared with the workbench's pending cards:

- zero days: `今天`;
- an answer or decision older than zero days: `等了 N 天`;
- an assignment, breakdown or lesson older than zero days: `N 天前`;
- the latest-report basis: `Agent 最后一次报告是 N 天前` (or `今天` on the same date).

A lesson's timestamp describes when its session recorded the candidate, not how long a
question has waited. The lead's `最久的一件等了 N 天` therefore takes the maximum over
questions, decisions, assignments and breakdowns, excluding lessons. If none of those
entries has a known timestamp, the lead omits the oldest-age clause.

The queue's existing ordering is unchanged: descending derived elapsed `durationDays`,
unknown ages last, with the entry key as the tie-breaker. Calendar display ages do not
silently replace that owner-pending ordering rule.

## Render clock, not response-read age

The model never reads the wall clock. Each entry carries
`clockOffsetUnixMs = input.options.nowUnixMs - read_at_unix_ms`. At render, the page uses:

```text
referenceUnixMs = Date.now() + clockOffsetUnixMs
```

On the live composed path, the derivation clock and response-read moment are the same,
so the offset is zero and the reference is the render clock. The fixed-week demo has an
explicit derivation clock different from the real response moment; the offset preserves
that declared clock without treating the response itself as the age reference. In
particular, `Date.now() - read_at_unix_ms` is never used as an entry's age, which would
make a freshly read old item appear new.

## Canonical prototype numbers

The canonical facts remain the original prototype facts; no timestamp is moved to make
the copy fit:

| Fact | Recorded instant (UTC) | Display on Nov 19, 2026 |
| --- | --- | --- |
| Empty-state question created | Nov 17, 12:00 | `等了 2 天` |
| Candidate lesson session | Nov 13, 12:00 | `6 天前`, session date `11 月 13 日` |
| Codex latest report | Nov 17, 14:00 | `Agent 最后一次报告是 2 天前` |
| Assignment updated | Nov 19, 09:40 | `今天` |

The report interval is 46 hours. Its prototype value of two days therefore follows the
calendar dates Nov 17 to Nov 19, not two completed 24-hour periods. The prototype stores
its lead value as the hand-authored `VIEWS.me.oldest = "2 天"`; the product's deterministic
interpretation is the maximum non-lesson wait, which is the two-day question rather than
the six-day-old lesson session.

For 林悦, the prototype's hand-written queue order is answer, assignment, lesson. The
unchanged product rule yields lesson (6 days), answer (2 days), assignment (today).
Changing to the prototype order remains an owner decision and is not made here.
