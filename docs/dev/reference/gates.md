# Completion check gates

Use `.awr/gates.toml` to make recorded example checks part of work completion. Start in observe mode to see each outstanding check, then choose an explicit scope for enforcement. The `gates.checks` capability identifies builds that support this policy.

## Configure a project

```toml
version = 1

[checks]
mode = "observe"
scope = { tags = ["checked"] }
small_work = "reason"
require_clean_tree = true
separate_approver = false

[hooks]
completion_reminder = true
```

`mode` accepts `off`, `observe`, or `enforce`. An absent file or checks table uses
`off`. Enforce requires `scope = "all"` or a tag scope. Observe uses all work when
scope is omitted. A tag scope applies when at least one listed tag belongs to the
work. Existing `[hooks]` settings share this versioned file.

The gate applies to implementation work with required acceptance criteria. It
uses the criterion tiers already resolved from the work definition and its
specification references. Research, document, decision, and later-tier work keep
their existing completion behavior.

## Run and inspect checks

Bind each required example to a command and the files that define its oracle.
Run its baseline before implementation, obtain admission for the current oracle,
and run acceptance against the commit being delivered:

```sh
awr work check W --session S --phase baseline --expected-revision R
awr work check W --session S --phase acceptance --write-input .awr/completion-input.json --expected-revision R
awr work complete W --session S --reason "Draft checks passed" --input .awr/completion-input.json --expected-revision R
```

Replace W and S with the work key and active session ID. Read the current project
revision for R before each write. The acceptance command produces a completion
input and registered evidence when the required checks pass.

`work prepare` returns `check_obligations`: each required example's state,
outstanding reasons, and complete baseline and acceptance commands. A bound
session supplies its ID; preparation before claiming uses a session placeholder.
`work show` and `work prepare-completion` return the same `checks` summary.
These summaries inspect current files and recorded runs. The completion input's
source SHA selects the acceptance revision; ordinary inspection uses Git HEAD.

For each required example the gate checks its binding, current oracle, passing
acceptance result at the completion SHA, and failed baseline at the same oracle.
By default the acceptance run must record a clean tracked Git tree. Set
`require_clean_tree = false` to accept a dirty observation. An unknown tree state
requires that same explicit setting. With `separate_approver = true`, use a
completion session distinct from the admission approver.

Enforce reports `EvidenceMissing` with rule `completion.checks`, an `unmet` list
containing the example, criterion, and reason, and a `next_action` command.
Observe saves the same findings and allows completion. The completion response,
`work.completed` event, and persistent completion binding carry `check_gate`,
including `applies`, `mode`, `unmet`, and `would_refuse` for later inspection.

Small work can provide a single nonempty `check_waiver` line in the completion
input when `small_work = "reason"`. The completion binding preserves this reason.
Choose `small_work = "checks"` to require ordinary checks for small work as well.

When the Stop reminder is enabled, an applicable satisfied gate prompts the
client to complete its claimed work once per commit. Missing checks allow the
ordinary checkpoint path to continue. `doctor` reports an unreadable policy as
`gates_policy_unreadable` at error severity.

## Limits

This gate evaluates recorded program observations and source bindings. It does
not assess whether an example captures the intended business behavior. Failed,
errored, or unknown acceptance results remain unmet. Mutation discrimination
currently reports `mutation_missing`; justified alternatives report
`alternative_unreviewed` while their review contracts are being added. A waiver
is accepted only for explicitly small work under the reason policy. Existing
report validation, dependency checks, and completion protections still apply.
