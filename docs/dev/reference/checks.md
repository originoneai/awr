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
| `baseline` | A nonzero exit classified as `failed_as_expected` | Can run before admission |
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

## Classify baseline failures

Baseline receipts and the check summaries in `work show` expose `classification` alongside
the recorded outcome and log location. AWR classifies captured output and the exit result
using built-in rules and that example's declared `check.files` and `check.run`:

| Classification | Observed features | Admission action |
| --- | --- | --- |
| `failed_as_expected` | Assertions, real unittest test-method `ERROR:`/`FAIL:` lines, pytest `FAILED path::test`, or missing symbols imported from implementation modules | Satisfies the current example's baseline obligation |
| `failed_unrunnable` | Missing check modules or their parent packages, unittest collection failing to import a check module because of a missing module, syntax errors in declared check files, missing launchers or entry points, or timeouts | Refuses governed approval with `baseline_unrunnable`, the recognized cause, and a baseline retry command |
| `failed_unclassified` | Missing other modules, compiler diagnostics, syntax errors outside check files, or otherwise inconclusive failures | Refuses governed approval with `baseline_unclassified` and a path to independent review |
| `passed` | Successful exit | Leaves `baseline_passed` unmet |
| `legacy` | A stored baseline from before classifications were recorded | Retains the original outcome-based baseline semantics |

Explicit check loading or startup failures take precedence over behavior failures and
ambiguous diagnostics, even across stdout and stderr. Compiler diagnostics and missing
non-check modules remain ambiguous when mixed with assertions. A missing input file
mentioned inside an assertion remains a behavior failure. Missing-command classification
uses launcher diagnostics or a recorded spawn failure; entry-point diagnostics must name
an argument in `check.run`. Classification performs no extra execution or file reads.

For Python, `tests/test_promo_code.py` identifies `tests.test_promo_code` and its parent
package `tests`. Syntax and indentation errors identify the last traceback file in the
same output stream. A unittest `unittest.loader._FailedTest` identifies a loader placeholder;
classification uses its underlying diagnostic. A missing-symbol `ImportError` from an
implementation module counts as a behavior failure, including during test collection.
For example, this import can fail while a new implementation symbol is still pending:

```python
from shop.cart import PromoCodeAlreadyAppliedError
```

An incompatible dependency version can produce the same missing-symbol error: a baseline
proves only that the check failed before implementation. Acceptance still requires the
same check to pass.

For an unrunnable baseline, `reason` records the recognized loading diagnostic or the
existing startup/timeout code. The receipt and admission refusal use it to explain the
specific failure before giving the retry command.

For an unclassified behavior failure, inspect the logs and record `justified_alternative`
with a `discrimination.note` explaining the evidence. Propose admission, then have another
session approve the alternative. The existing independent-review requirement applies.
Changing the method changes the contract and requires admission on the updated definition.
Legacy records keep their classifications visible without reinterpreting old log files.

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
