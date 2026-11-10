# Run example checks

`awr work check` runs the checks declared in a work definition and records each result.
Use a baseline run before implementation, then an acceptance run to generate completion
reports and evidence. [Work definitions](work-definition.md) describe the example bindings.

```sh
awr work check W --session S --phase baseline --expected-revision R
awr work check W --session S --phase acceptance --write-input /tmp/w.json --expected-revision R2
awr work complete W --session S --reason "Acceptance checks passed" --input /tmp/w.json --expected-revision R3 --end-session
```

Replace the work, session and revision placeholders with current values. Each operation
returns its resulting project revision. The session must be active and bound to the work.

## Select checks and inspect results

The default selection includes every check-bound example of a required acceptance criterion.
Repeat `--example ID` to select specific bound examples. A partial selection records results;
completion evidence requires all required bound examples to pass in the same acceptance run.
`--timeout SECONDS` overrides each example's declared timeout; the fallback is 300 seconds.

The command starts each argument vector directly at the project root.
The declared command must be available on the host; shell features require an explicit shell
in the argument vector. Commands have the permissions of the calling process.

| Phase | Expected observation | Admission |
| --- | --- | --- |
| `baseline` | A nonzero exit recorded as `failed` | Can run before admission |
| `acceptance` | Exit zero recorded as `passed` | Governed work needs current admission and this session's active claim |
| `mutation` | Exit zero plus a valid report with zero survivors and timeouts | Same admission and claim requirement as acceptance |

Recording a result exits zero, including a failed check. Add `--require-met` to exit one when
any selected example misses its phase expectation. A timeout is `unknown` with reason
`timeout`; AWR terminates the run's child process and waits for it to exit. On Unix, it also
terminates the run's process group, escalating from termination to a kill after a short grace
period. A spawn error is `errored`. A signal termination is `unknown`.

Text output has one line per example and a final `Next:` line. JSON contains `ok`, `work`,
`phase`, `results`, `met`, `next_action` and `project_revision`. Inspect recorded examples and
runs through the read-only status query:

```sh
awr work check W --status --json
```

## Inspect the recorded evidence

Each `work.check` event binds the result to its example, criterion digest, phase, argument
vector digest, oracle file digests, source commit, tracked-tree state and log digests.
The command preview is limited to 256 UTF-8 bytes; events are limited to 16 KiB.
Oracle files are read from the current project tree with a 4 MiB limit per file. Changing a
listed file during execution records `errored` with reason `oracle_changed_during_run`.
A missing listed file records `errored` with reason `oracle_file_missing`.

Logs live under `.awr/artifacts/checks/WORK/SEQUENCE-PHASE-EXAMPLE/` as `stdout.log` and
`stderr.log`. Failure output points to that directory. `doctor` verifies these files against
their producer events and reports unexpected files or digest mismatches. The source commit
and tracked-tree observation are separate facts: a dirty tree is reported explicitly.
A project with an unobservable Git commit records an unknown source. Acceptance can generate
commit-bound completion evidence once a commit is available.

When all required bound checks pass during acceptance, AWR imports a completion report as a
registered artifact and registers `check_run` evidence at `locally_verified`. Its `recorded`
trust comes from the producing check events. Successful completion upgrades the record to
`verified` after matching the report bytes and binding its exact evidence snapshot. This byte
verification takes precedence over the earlier `recorded` observation. Manual registration,
including evidence whose type is `check_run`, starts as `reported`.
Evidence registration and reads share the [trust rules](trust.md) through CLI and the existing
MCP tools. Independent review and business acceptance use separate evidence.

The JSON response includes `completion_input` and `unmapped`. Every acceptance criterion
appears in the input. Criteria covered by the report reference its evidence; other criteria
have empty evidence lists and appear in `unmapped`. They need separately registered evidence
merged into the input before completion.

`--write-input PATH` writes generated input relative to the calling directory, matching
`work complete --input`. It writes when the run generates completion input.
Keep report generation, work completion, review and release as distinct steps.

## Mutation and alternatives

Declare the project's report command in `.awr/gates.toml`, then run the mutation phase:

```toml
version = 1
[checks.mutation]
run = ["python3", "examples/checks/mutants_example.py"]
```

```sh
awr work check W --session S --phase mutation --require-met --expected-revision R
```

The command runs once at the project root and writes one UTF-8 JSON report to standard
output. AWR records the report against each selected example's current oracle. The bundled
[synthetic example](../../../examples/checks/mutants_example.py) illustrates this format:

```json
{"version":1,"source_sha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","tool":"project-report 1","mutants":[{"id":"save-result","file":"src/save.rs","line":42,"status":"killed","killed_by":["keeps-input"]}]}
```

| Report field | Meaning |
| --- | --- |
| `version` | Report contract version `1` |
| `source_sha` | The current Git commit, matching the recorded check source |
| `tool` | The project's tool and version label |
| `mutants` | Entries with unique `id`, `file`, positive `line`, and `status` |
| `killed_by` | Optional list of example or test identifiers |
| `note` | Required nonblank explanation for `equivalent` entries |

Entry status is `killed`, `survived`, `equivalent`, `timeout`, or `skipped`. Events store
all five counts in `mutants` and the first surviving locations in `survivors` as
`{id,file,line}`. A successful command with zero survivors and timeouts records `passed`.
Survivors produce `mutants_survived`; mutant timeouts produce `mutation_timeout`.
Output lists surviving files and lines and suggests adding an example or recording a finding.
`--timeout SECONDS` controls the project command; its fallback is 300 seconds.

Completion with `diff_mutation` requires a passing mutation observation for the completed
source commit and each current example oracle, alongside the acceptance checks. The clean-tree
policy applies to both phases. A later valid run of the same source and oracle satisfies the gate.

For `justified_alternative`, provide `discrimination.note` with the proposed review method.
The definition report exposes `discrimination_note_missing` for an empty explanation.
Propose admission from an identified session, then approve that proposal from another session.
This approval rule applies across tags, including inherited alternative methods. Approval
receipts retain the proposer and reviewer session identities for the completion gate.

## Limits

- `--status` reads stored facts without executing commands, refreshing sources or changing state.
- Completion reports and evidence require every required bound example to pass in the same acceptance run, with an observable Git commit.
- A passing selected check leaves unmapped criteria for separate evidence and validation.
- Mutation reports contain at most 4 MiB and 5000 entries; events retain at most 20 surviving locations.
- Mutation IDs, files, and tool labels allow 128, 512, and 256 UTF-8 bytes respectively, with printable text.
- Mutation execution consumes the declared JSON contract; projects supply report commands and tool-specific adapters.
- A missing mutation command returns `Unsupported` with guidance to declare `[checks.mutation]` in `.awr/gates.toml`.
