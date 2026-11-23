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

## Canonical directory scope and shell facts

The canonical viewer's recorded scope is all projects (`viewer.can_view_all_projects`),
so two more reads carry the organization's whole visible hierarchy (AWR-PPO-081). Both
stay isolated to `canonical`:

- `directory/read` lists every unit the visible projects cover — both departments and all
  three groups, 研发部/应用小组/平台小组/客户交付部/交付小组 in the fixture's own order —
  instead of one project's slice. Project membership (`members`,
  `project_department_ids`) stays scoped to the queried project, exactly as before.
- `GET /api/team/shell` answers the facts the existing perspective rules consume
  (`derivePerspectives`, unchanged): zero supervised groups, one member group, three
  visible groups, two departments, the explicit all-projects authorization and a
  directory with groups and departments registered. With those facts the four heights
  open by the rules themselves; no rule is relaxed. Every other variant and every live
  deployment answers `perspective: null`. Pending counts stay the composition layer's
  facts (the shell leaves them unknown).
- The same shell read carries the organization's display name (demo: 示例组织; live:
  null, and the header falls back to its authorization wording).

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

## Display clock and display zone (canonical demo)

Every displayed age and calendar label formats against one reference clock and one zone,
supplied by the reads the page already owns:

- **Reference clock.** A reply that carries a derivation clock (`input.options.nowUnixMs`,
  the overview-model replies) supplies `nowUnixMs - read_at_unix_ms` as an offset; the
  page renders against `Date.now() + offset`, so the read's clock keeps walking forward
  with real time instead of freezing at the answer moment. The reply's `page_read.at_unix_ms`
  equals its read time and never serves as "now". When a page holds no reply clock (a
  failed or absent overview read), the viewer's own clock is the fallback. An age counts
  from the read's moment on that same clock (`read_at_unix_ms + offset`, which is the
  derivation clock itself), so it is the real time since the read whatever clock the reply
  derives against; the real read time sits after a fixture clock and would read as a date
  in the future. The header's "today" is that clock's day in the display zone. The helpers
  live in `tools/observer/src/lib/display-clock.tsx` (`readClockOffsetUnixMs`,
  `readMomentUnixMs`, `referenceNowUnixMs`), and a guard test keeps every page and
  rendering component from reading the browser clock on its own.
- **Display zone.** `GET /api/team/shell` carries `displayTimeZone`. The `canonical`
  variant answers `'UTC'` because the fixture's clock times are the prototype's wall-clock
  times in UTC: with the reference clock above they make the activity ledger read exactly
  the prototype's `12 分钟前` / `今天 09:40` / `昨天 15:20` / `11 月 18 日 10:02`. Every
  other variant and every live deployment answer `null`, and pages then format in the
  viewer's own zone. The zone is a fact from the bridge, never inferred from a variant
  name in the page, and a value the runtime cannot resolve reads as unpinned.
- **Ledger wording.** The shared formatter (`compactTimeText` in
  `src/core/observation/time.ts`) keeps relative wording to the last hour, calendar
  wording (`今天` / `昨天`) to the last 24 hours of the reference clock, and reads anything
  older as the calendar fact (`11 月 18 日 10:02`, with the year for earlier years). All
  pages (activity, inbox, projects, workbench, members) format through it.

The pinned zone is demo-only presentation: it changes which wall clock the viewer reads,
never any stored fact, and the live path keeps answering `null` until a real deployment
says otherwise.
