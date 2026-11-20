# Demo variants

The observer ships a synthetic organization so pages, reads and failures can be exercised
without a live Team service. One variant is chosen per server start: `--demo-variant <name>`
on `node server.js`, or `AWR_OBSERVER_DEMO_VARIANT` in the environment.

Every variant serves the same public fixture through the same read paths the live service
uses; only the shape of what the service would honestly return changes. The variant list
lives in `tools/observer/server/demo-organization.js` and an unknown name fails startup.

| Variant | Purpose |
| --- | --- |
| `full` | The whole organization through real-shaped reads. Pages show the honest gaps the live service still has: facts with no read behind them yet arrive as unknown, and the numbers say so. |
| `partial` | Reads fail mid-composition (`Unavailable`). Pages keep what was confirmed and declare the failed part with its reason and resume cursor. |
| `single` | One project only. Exercises single-scope navigation and directory behavior. |
| `restricted` | Permission walls: a denied project and denied read kinds surface as `Forbidden` envelopes. |
| `empty` | An organization with nothing configured. Pages render their empty states. |
| `canonical` | The same organization with every overview fact already known, served by `GET /api/team/overview-model` per project (`P-001`, `P-002`, `P-003`). All other routes behave exactly like `full`. Use it to compare a page with the prototype number for number; it stands in for the service after the remaining reads exist. |

## The canonical input

`canonicalOverviewInput(data)` in `tools/observer/server/demo-organization.js` builds the
fully-known model input: structured external waits, acceptance evidence, submitted-result and
verification times, commitments and the pending approvals, next to the planes the composed
path already reads. Each project's reply scopes its planes to that project; the member
directory spans projects because cross-group waits and the wide-view personas resolve through
it, and the union of the three replies is the whole canonical input. The reply keeps the
composed path's envelope (`project`, `read_at_unix_ms`, complete `coverage`, `freshness`) and
its `options`: the demo clock's fixed "now", the viewer's `me` scope, the calendar periods
and the stall threshold that reproduce the prototype statuses. The derivation tests in
`tools/observer/test/demo-organization.test.js` pin the prototype numbers against this input.

## Demo clock

Demo timestamps describe a fixed week in late 2026. The startup offset moves that week to
the present as a whole number of weeks, so the fixture's "now" keeps its weekday and time of
day and calendar-week windows keep catching the same events the prototype shows. The offset
is rounded to whole weeks by default; an explicit `timestampShiftMs` (tests use `0` or any
value) still wins, and relative time survives any offset while part-week drift can move an
event across a week boundary.

Two consequences are deliberate. Month windows follow the viewer's calendar, and a month is
a whole number of weeks only by luck: as the rounded offset grows, a verification near a
month edge slides into the neighbouring month, so a month-based figure can drift by a task
while every week-based figure stays put. And the rounded "now" trails the real read time by
up to half a week, so an authoritative freshness reading can honestly say `stale` for demo
data whose week was pinned to align with the prototype.
