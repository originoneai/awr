# Take over a project with `awr takeover`

`awr takeover` brings an existing project to an accepted AWR initialization in saved, resumable steps and asks no question on the way. It runs the CLI's own `init` and `intake inspect`, applies an optional reviewer's draft under fixed guards, and writes a receipt of what it mapped, created and left for the owner to confirm.

## Use it

```sh
awr --project /absolute/project takeover
awr --project /absolute/project takeover --reviewer /absolute/reviewer --review-timeout-secs 600
awr --project /absolute/project takeover status
```

Without a reviewer the draft AWR generated is applied as it is. A run that stops continues when the same command runs again; steps that finished are skipped. The project's own files stay byte-identical, `.awr/` and the `.gitignore` entries AWR lists itself are the only additions, and nothing is committed or pushed.

## How it behaves

| Step | What runs | Writes to the project |
| --- | --- | --- |
| `preflight` | The project exists, has no `.awr/` (or `--adopt` is given) and has no active session; Git `HEAD` is noted | Nothing |
| `preview` | `init` preview, then `init --write-draft` into the state directory | Nothing |
| `review` | The reviewer turns the draft into a reviewed draft | Nothing |
| `accept` | `init --from-draft` preview, a check of its writes, then `--accept --expected-preview` | `.awr/` and AWR's ignore entries |
| `organize` | `intake inspect` | Nothing |
| `receipt` | `.awr/takeover/receipt.json`, copied to the state directory | The receipt |

| Option | Meaning |
| --- | --- |
| `--reviewer <PROGRAM>` | Command that reviews the draft; `--reviewer-arg <ARG>` adds one argument per use |
| `--review-timeout-secs <N>` | Seconds the reviewer may run; default 900 |
| `--adopt` | Organize an initialized project that has no active session; `preview`, `review` and `accept` are skipped |
| `--max-rounds <N>` | Preview, review and accept rounds when the sources change meanwhile; default 3 |
| `--state-dir <DIR>` | Run records and drafts, outside the project; default `$AWR_TAKEOVER_STATE`, else `$XDG_STATE_HOME/awr/takeovers`, else `~/.local/state/awr/takeovers` |
| `--rerun` | Start a new run although the last one completed |

### Reviewer contract

The reviewer is any command; AWR starts it without a shell and sends one JSON object on stdin.

| Direction | Fields |
| --- | --- |
| stdin | `project_root`, `draft`, `draft_path`, `round`, `sources`, `ambiguous_domains` |
| stdout | `generated_files`: object of path to text under `.awr/intake/`; `notes`: short text; optional `authority_mapping` and `ambiguous_domains` to settle an ambiguity |

AWR reads the answer from plain JSON, from the last fenced `json` block, or from the outermost braces. The receipt keeps the notes and the files the reviewer changed.

### Guards on a reviewed draft

| Guard | Rule |
| --- | --- |
| Paths | Generated files sit under `.awr/intake/`, are text, and stay within 1 MiB |
| Goals | A goal status of `active` or `confirmed` is lowered to `candidate`; only the owner confirms a goal |
| Work | A ledger returned with no work items gets the draft's `work_items:` section back, so a conversation can bind to a work item |
| Authority | A draft that still lists ambiguous domains stops the run |
| Writes | An `init` preview that lists a path outside `.awr/` other than `.gitignore` stops the run before accept |
| Secrets | A stop names the category, the path and the line of the source; the value appears nowhere |

### Stops

A stopped run records `step`, `reason` and a message in `takeover.json`, prints the outcome, and exits 1. The error report carries `details.takeover` with the step, the reason and the run directory.

| Reason | Meaning | Next action |
| --- | --- | --- |
| `secret_boundary` | A source holds content that looks like a credential | Redact the line, or pass `awr init --manifest` listing the sources AWR can read |
| `source_unreadable` | AWR cannot read a source | Fix the file named in `detail`, then run again |
| `ambiguous_authority` | Several candidates own one domain | Name the authoritative source in the reviewed draft; the next run reviews again |
| `inventory_changed` | The sources changed during review for more than `--max-rounds` rounds | Run again once the project is quiet |
| `active_session` / `already_initialized` | The project already runs under AWR | Close the session, or use `--adopt` |
| `review_failed` / `review_rejected` | The reviewer failed, timed out, or broke a guard | Fix the reviewer and run again |
| `unexpected_write` | The preview lists a path outside `.awr/` besides `.gitignore`; nothing was written | Change the reviewed draft; the next run reviews again |
| `accept_failed_partial` | `accept` failed after it began writing | Read `detail` and the `.awr/` left behind before running again |

### Output

`--json` prints `{outcome, record, receipt, run_dir}`; plain output prints a summary. `takeover status` shows the record and receipt of the last run. Records use snake_case keys. The receipt lists `mapped` sources, `created` writes, `reviewed` notes, the `organization` state, goals in `awaiting_confirmation`, and `git` with `committed` and `pushed` set to `false`.

## Limits

- AWR ships no reviewer; any command that follows the contract works.
- Organization is one `intake inspect`; goals stay candidates until the owner confirms them.
- A lock from a stopped process is replaced, and any lock expires after two hours.
- Resuming reuses the saved draft; a source that changed since the preview starts a new round, up to `--max-rounds`.

## Code and tests

- [Implementation](../../../crates/awr-cli/src/takeover.rs)
- [CLI tests](../../../tests/intake/takeover_cli.rs)
- [Intake overview](../TAKEOVER.md) and [secret boundaries](secret-boundaries.md)
