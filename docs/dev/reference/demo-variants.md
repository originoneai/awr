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
| `canonical` | The same organization with every overview fact already known, served by `GET /api/team/overview-model` per project (`P-001`, `P-002`, `P-003`). All other routes behave exactly like `full`, except `GET /api/team/projects`, which adds the project-list facts below. Use it to compare a page with the prototype number for number; it stands in for the service after the remaining reads exist. |

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

## Canonical project-list facts

The canonical variant also serves the facts the project list needs for its prototype
comparison. These are **demo-only shapes**: they stay isolated to `canonical` until the
corresponding live reads exist and supply their real response shapes.

- `GET /api/team/projects` answers four entries under `canonical`. The three ordinary
  projects keep their fixture keys and add the recorded project lead (`leadPersonId`,
  a person id the member directory resolves to 周宁/吴桐/郑岚) and the group's display
  name (`groupName`, from the organization's own departments — never a group
  identifier). Ordinary entries carry `takeover: null`, a recorded non-takeover, so
  the page's takeover count stays a known number instead of an honest unknown.
- The fourth entry is the prototype's just-taken-over `sample-cli` with a `takeover`
  fact (`resultId`, `recordedAtUnixMs` half a minute before the demo clock,
  `verifiedTasks: 0`, `workstreams: 4`, `questions: 4`). Its recorded moment is
  relative to the demo clock, not the real read time, because the page compares every
  update age against `input.options.nowUnixMs`.
- Each ordinary project's overview-model reply carries `periodUpdates`, a complete
  `ReadSlice` containing one selected update in the page model's shape (`id`, `startsOn`,
  `endsOn`, `status`, `summary`, `recordedAtUnixMs`): the current `U-001` for P-001,
  current `U-004` for P-002, and only the earlier `U-007` for P-003. Fixture dates are
  week-aligned strings that a whole-week clock offset does not move, so the server
  re-derives the selected period from the demo clock's own week: it keeps its
  distance-in-weeks from the fixture's "now" week whatever the offset. The recorded
  times ride the clock, which is what keeps the page's update column at the
  prototype's 今天 / 3 天前 / 12 天前.
- The project list does not query an overview model for the takeover entry. The existing
  shell's inbox indicators independently query every directory key, so canonical answers
  that shell read with a scoped empty known model for P-004; unknown directory keys remain
  `Forbidden`. This demo-only shell answer is not a replacement takeover read.

Which reads replace them: AWR-PPD-010/011/012 supply the phase-update reads,
AWR-PPD-052 supplies group display metadata, AWR-PPD-060b supplies the composed
project-list read including lead metadata, and AWR-RCN-009 supplies takeover results.
Until then the `full` variant keeps answering without any of these fields, and the page
shows its honest degradation.

## Freshness reports the answer moment, the derivation clock stays on the fixture week

The demo clock exists so week windows and relative ages match the prototype; it is a
derivation input (`input.options.nowUnixMs`), not a report of when anything was read.
Since the whole-week rounding keeps that clock up to half a week away from the real
present, every demo variant reports read times independently: the reply's
`read_at_unix_ms` and every freshness plane's `at_unix_ms` are the moment the demo
server answered, so a freshness badge reads `measured` on any day the demo runs
instead of honestly degrading to `stale` for data whose week was pinned on purpose.
The out-of-order and currency comparisons still run on those answer times, and the
live path never takes this branch: a live `at_unix_ms` stays whatever the upstream
service declared.

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
up to half a week; that gap is why freshness reports the answer moment separately (see
above) instead of measuring the derivation clock.
