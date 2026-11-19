# Perspective rules: four observation heights

`tools/observer/src/lib/perspective/rules.ts` implements this table. Its test reads the
rows and checks each rule against the implementation. Inputs are facts already read by
the composition layer; the perspective module performs no service reads and no semantic
inference. `null` means unknown, and an unknown fact leaves wider perspectives closed.

| Rule | Height added | Required known facts | Code |
| ---- | ------------ | -------------------- | ---- |
| 1 | My work (`我的工作`) | Always present as the narrow fallback. A member with only personal tasks, or with no established wider scope, has only this height. An empty personal work list keeps the fallback. | `me` |
| 2 | Group delivery (`小组交付`) | The member supervises at least one group, or participates in a group with at least one visible project. | `lead` |
| 3 | Department projects (`部门项目`) | The directory has both groups and departments registered, and visible projects cover at least two groups. | `dept` |
| 4 | Goals and delivery (`目标与交付`) | The directory has both groups and departments registered, and visible projects cover at least two departments or the member can explicitly view all projects. | `top` |

The heights are ordered `me`, `lead`, `dept`, `top`. Rules 2 through 4 add buttons while
keeping the narrower buttons established by the other rules. Known facts can establish
rule 4 independently when, for example, all projects are visible inside one department.

## Missing directory facts

If either groups or departments are absent from the directory, rules 3 and 4 stay closed
and at most `me` and `lead` remain. The page says `尚未登记小组` or `尚未登记部门`,
respectively. Unknown registration also closes both wider rules, and the page identifies
the unknown registration. Missing group relationships, project coverage, or all-projects
authorization are likewise named instead of being guessed.

## Selection and persistence

- The default is the narrowest available height with a known positive count of things
  waiting for the member. If none has a known positive count, the default is `me`.
- A remembered choice is used only while it remains available. Storage reads and writes
  are guarded; blocked storage leaves the in-memory switch working and a new load uses the
  default.
- The `perspective` URL parameter is only a request. An invalid, forged, stale, or newly
  unauthorized value resolves immediately to the default and is replaced with that safe
  selection while other URL parameters stay intact.
- A fact change, including revocation, recomputes the available buttons and the current
  selection in the same render. Restoring a remembered or URL height requires fresh facts
  that authorize it again.
- Switching changes only the observation height. It sends no business action to a service.
