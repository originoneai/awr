# Inbox pending copy and ages

The inbox is a read-only queue. Its copy comes from the composed overview facts in
`tools/observer/src/lib/inbox-model.ts`; the page states only the people, tasks, session
dates and Agent names that were read.

## Card composition

- An answer or decision card uses the recorded question as its title.
- An assignment uses `{{who}}把“{{task}}”派给了你` only when both the dispatcher's
  display name and the task name were read. If either is absent, the card keeps the
  recorded task title or the existing unnamed-item fallback, so the composed sentence
  always has every clause filled. Its from-line says the dispatcher assigned it today
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
subtracts dates instead of dividing elapsed milliseconds by 24 hours, which keeps the
result stable across daylight-saving transitions. Missing, invalid and future timestamps remain unknown.

The wording is shared with the workbench's pending cards:

- zero days: `今天`;
- an answer or decision older than zero days: `等了 N 天`;
- an assignment, breakdown or lesson older than zero days: `N 天前`;
- the latest-report basis: `Agent 最后一次报告是 N 天前` (or `今天` on the same date).

A lesson's timestamp marks when its session recorded the candidate and says nothing about
how long a question has waited. The lead's `最久的一件等了 N 天` therefore takes the maximum over
questions, decisions, assignments and breakdowns, excluding lessons. If none of those
entries has a known timestamp, the lead omits the oldest-age clause.

The queue lists waits first, longest derived wait (`durationDays`) first, unknown ages
last, with the entry key as the tie-breaker. A lesson confirmation follows every wait
whatever its age, for the reason the lead leaves it out of the oldest wait: its timestamp
marks when the session recorded the candidate. Within the confirmations the same age order
applies. Calendar display ages leave the ordering key untouched.

## Render clock instead of response-read age

The model is clock-free. Each entry carries
`clockOffsetUnixMs = input.options.nowUnixMs - read_at_unix_ms`. At render, the page uses:

```text
referenceUnixMs = Date.now() + clockOffsetUnixMs
```

On the live composed path, the derivation clock and response-read moment are the same,
so the offset is zero and the reference is the render clock. The fixed-week demo has an
explicit derivation clock different from the real response moment; the offset preserves
that declared clock without treating the response itself as the age reference. An
entry's age is always measured against that reference, because `Date.now() - read_at_unix_ms`
would make a freshly read old item appear new.

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
calendar dates Nov 17 to Nov 19 (under two full days elapsed). The prototype stores
its lead value as the hand-authored `VIEWS.me.oldest = "2 天"`; the product's deterministic
interpretation is the maximum non-lesson wait, which is the two-day question rather than
the six-day-old lesson session.

For 林悦, the prototype's hand-written queue order is answer, assignment, lesson. The rule
above yields the same order: answer (2 days), assignment (today), lesson (6 days, last).
