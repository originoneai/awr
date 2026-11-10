# Documentation style

Write reference pages that help readers choose an action, run a small example, and understand its result. Lead with the page's purpose, explain behavior with a table, and collect limitations in one section. Use the same action-and-result language for agents.

## Use it

Run the checks from the repository root:

```sh
python3 scripts/check_docs_style.py --self-test
python3 scripts/check_docs_style.py
```

A minimal reference page follows this order:

```md
# Inspect work

Inspect a work item's current state and next action.

## Use it
Run `awr work show SAMPLE-001`.

## How it behaves
| Input | Result |
| --- | --- |
| Work key | Current state and next action |

## Limits
Results describe the recorded project state.

## Code and tests
See the implementation and its regression tests.
```

After improving an existing page, run `python3 scripts/check_docs_style.py --update-baseline` and review the diff. Initial setup uses `--write-baseline` once.

## How it behaves

| Area | Writing rule |
| --- | --- |
| Opening | Use an affirmative purpose paragraph of at most 60 words after the main heading. |
| Use it | Provide a copyable command or request and a minimal example before the rules. |
| Behavior | Use a table with one rule per row. Give each number a unit. |
| Limitations | State each boundary once, in Limits; keep each entry within 25 words. |
| Parallel negatives | Put groups of three or more in a table column. |
| Agent text | Describe the action and the result in tool descriptions, guidance, and errors. |
| CLI summary | Keep each overview description within 60 characters. |
| MCP catalog | Keep each first-level description within 200 bytes: purpose, then returned result. |
| MCP details | Put further boundaries in the second-level discovery result's `limits` array. |
| Code and tests | List the implementation and relevant test paths. |

| Check | Passing condition |
| --- | --- |
| R1 | Recorded density plus 0.5 per 1,000 units is the ceiling for existing pages. |
| R2 | New-page density stays within 8.0 per 1,000 units. English pages below 300 words skip density checks. |
| R3 | New openings stay within 60 words and zero negatives; existing openings stay within both recorded counts. |
| R4 | New pages have at most one Limits, Known limits, Boundaries, 限制, or 边界 section; existing pages use max(recorded count, 1) as their ceiling. |
| R5 | Relative Markdown links, images, reference links, and HTML targets resolve to existing local paths. |
| Baseline | Each page records density, opening words, opening negatives, and limit sections. Updates accept equal or lower values individually. |

Density uses English words or Han characters, whichever count is larger; Chinese negatives use the Han denominator. Opening length counts English words and individual Han characters.

## Limits

- Automated scope: both root READMEs and Markdown files directly in `docs/series`, `docs/dev/reference`, and `docs/dev/integrations`.
- Files named `team-*` receive the relative-link check alone.
- Link checks cover local path existence; anchors and remote destinations need separate review.
- CLI and MCP length limits use their own implementation tests.

## Code and tests

- [Checker and built-in tests](../../scripts/check_docs_style.py)
- [Style baseline](style-baseline.json)
- [CI workflow](../../.github/workflows/public-repository.yml)
