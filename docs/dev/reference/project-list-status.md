# Project list status rules

The project list presents two kinds of evidence side by side. A phase update supplies the responsible person's current judgement and sentence. The existing overview derivation supplies deterministic task counts and completion segments. This separation keeps the spoken health assessment with its author and keeps arithmetic with the program.

The page reads the project collection and the composed overview model for each ordinary project. It uses the existing read times and coverage information throughout. Every displayed result belongs to one of three groups: a recorded judgement, a deterministic fact, or an explicit statement that the available evidence is insufficient.

## Status table

| Recorded situation | Status label | One-sentence column | Risk row emphasis | Update column |
| --- | --- | --- | --- | --- |
| A phase update covers the current local date and records `ok` | Normal (`正常`) | The recorded update sentence | No | Its recorded time |
| A phase update covers the current local date and records `risk` | At risk (`有风险`) | The recorded update sentence | Yes, the pale risk tint | Its recorded time |
| A phase update covers the current local date and records `off` | Off track (`偏离`) | The recorded update sentence | No | Its recorded time |
| No update covers the current local date, but an earlier recorded update exists | No update (`没有更新`) | Only the fact `上一次更新是 N 天前`; the earlier health judgement stays in its own period | No | The earlier recorded time; add `该更新了` when it is more than seven whole days old |
| The read model has no phase-update source, or the available list is empty | No update (`没有更新`) | Only deterministic task facts; no health claim | No | `还没有这项记录` |
| The phase-update slice is partial, unread, failed, or has an unrecognised value | Outcome unknown (`结果未知`) | Explain that only confirmed program facts remain | No | `还没有这项记录` |
| An explicit takeover record exists | Recently taken over (`刚接管`) | Deterministic takeover facts | No | The takeover record time |
| An explicit takeover record says zero tasks have passed acceptance verification | Recently taken over (`刚接管`) | Deterministic takeover facts | No | Show `接管后还没有按验收核验过的任务` in the completion column and link it to the takeover result |

The first three grades come directly from the recorded update. A waiting task, a stalled Agent, or a report containing alarming prose does not independently promote a project to at risk. This preserves the boundary between human judgement and deterministic observation. It also means that a pale risk row always has a matching recorded risk update behind it.

The no-update grade describes update coverage rather than project health. It can accompany useful task facts and an accurate completion bar. When the update source itself is unavailable, the same presentation makes the missing source visible in the sentence and update columns rather than suggesting that somebody has assessed the project.

## Current period selection and update age

The selector first orders recorded updates by end date, start date, and recorded time. It then chooses the latest update whose recorded period contains the viewer's local calendar date. The comparison uses `options.nowUnixMs` from the composed read. The browser's wall clock does not participate in this decision.

Recorded period boundaries take precedence over a `current` flag. This matters for older records that remain marked current in a fixture or a service response. For example, an update for 16–22 November is current on 19 November. An update for 2–8 November belongs to history on that same day, even when its flag says current.

ISO week periods begin on Monday, matching the prototype. The implementation compares recorded dates rather than manufacturing a replacement period. Local calendar dates handle the boundary correctly: an instant on 18 November in UTC can already be 19 November in the viewer's zone. Tests pin that distinction using a fixed instant and an explicit time zone.

Update age uses whole elapsed days from the fixed read time. Zero days is shown as today, and positive values use the `N 天前` vocabulary. A recent takeover can use `刚刚`. Future or missing timestamps use the recorded-time fallback rather than a negative age. The late marker begins after seven complete days, so seven days exactly and more than seven days are separate cases.

## Deterministic completion facts

The four completion segments come from `deriveOverview` and its `CountDetails` arrays:

- **Verified:** every acceptance criterion has current supporting evidence, an independent review approved the result, and the result is integrated.
- **Reported complete:** the report claims completion while the task has no submitted result.
- **In progress:** returned, review, stalled, running, or waiting tasks.
- **Not started:** ready or planned tasks.

The segments use these arrays directly. Their accessible labels expose the same values as the visible summary. A project with five tasks can therefore have one verified task, one reported-complete task, one running task, and two ready or planned tasks. The visible total is five because the underlying array contains five entries.

Unknown task facts remain visible as uncertainty. An unread task slice produces an empty confirmed-detail array and an unknown count. The completion area then explains that progress has only been partly read. Empty confirmed details and a known zero are different observations and receive different presentations.

## Counts, details, and unknown values

Every summary category is built from one filtered array. Its count equals that array's length whenever all relevant statuses are known. Opening a count applies the corresponding detail filter and shows exactly those entries. The section count and the number of visible rows therefore agree.

A known zero appears as `0` and leads to an empty detail category. An unknown count appears as `—` and has no link. This rule applies to risk, no-update, takeover, task totals, and completion segments. A direct URL still selects only a recognised detail key. Unrecognised keys return to the full list, including values that resemble negative indexes, large indexes, or arbitrary strings.

The risk and no-update summary counts become unknown when any contributing project has an unknown status. The takeover count becomes unknown when the collection leaves any takeover marker unspecified. A recorded ordinary project can carry an explicit empty marker, while an explicit takeover record carries the takeover result destination and its deterministic facts.

## Identity and display boundaries

A project lead is resolved through the recorded `leadPersonId` and the member directory's `personId`. Viewer scopes, actor identifiers, task owners, and report authors serve other purposes and do not supply a project lead. This keeps person identity comparisons inside one numbering space.

A group appears through its recorded display name. Group identifiers stay out of visible text. Missing lead or group metadata produces `还没有这项记录`. The same fallback principle applies to project names: the established project-label vocabulary can use a readable recorded project key, while diagnostic or opaque values receive the safe unnamed-project treatment.

An explicit takeover result identifier supplies the takeover-result link. Project keys and takeover result identifiers have different meanings and are encoded independently. Project links, summary-detail links, and takeover links all use their shared path functions, including escaping for slashes, question marks, spaces, and ampersands.

## Read-model gaps and honest degradation

The composed read model currently leaves phase updates, project lead/group display metadata, and takeover facts unavailable for some projects. The list handles those gaps through the rules above and adds no replacement read. A missing phase-update source leads to the facts-only no-update row. Missing ownership metadata leads to the recorded-metadata fallback. An unspecified takeover marker leads to an unknown takeover count.

Task-plane gaps remain separate from update-plane gaps. A project can have a valid recorded health update while its completion total is unknown. It can also have a complete completion bar while its current update is unavailable. Coverage text and the existing freshness badge describe those read conditions independently of the health label.

This composition gives each row a precise interpretation. Recorded judgements remain attributable to their authors, deterministic counts remain linked to equal-sized detail arrays, and every uncertain value has a visible explanation. The page can add future sources through the same model without changing its presentation rules or guessing at values that are currently absent.
