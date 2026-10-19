# Using the CLI

The `awr` command-line tool is the full management entry point for an AWR
project: everything an agent can do through MCP, you can do from your terminal
— plus the project administration that MCP does not expose.

## Install

Both package channels install the same Rust CLI and MCP server; there is no
separate JavaScript or Python SDK:

```sh
npm install -g @originoneai/agent-work-runtime@0.6.0
# or, in a Python virtual environment
python -m pip install agent-work-runtime==0.6.0
```

Verify with `awr --version` and `awr-mcp --version`.

Supported targets are macOS 15+ (arm64 and Intel), Linux with glibc 2.39+ (the
Ubuntu 24.04 baseline, x64 and arm64), and Windows x64. The npm launcher
requires Node 22.14+ and relies on optional native packages, so keep optional
dependencies enabled; the PyPI launcher requires Python 3.9+. On Linux, check
glibc with `ldd --version | head -n 1`. Git is required only for Git-bound
operations; SQLite is bundled. For a first-project walkthrough, see
[Quickstart](quickstart.md).

## The shape of every command

CLI examples throughout the documentation share a common prefix:

```sh
awr --project /absolute/project --json <command>
```

`--project` points at the project root; `--json` switches stdout to a single
machine-readable JSON object (details below). Drop `--json` when reading
output yourself — the default human output is deliberately bounded: `status`
shows at most one current suggestion and `ready` lists at most 10 items.

## Checking where the project stands

`status` is your first stop. Its default `action` view shows the current
continuation, claimable work, waits, actual blockers, and a history summary:

```sh
awr --project /absolute/project status --view full --branch main
```

- `--view action/full/summary` — default is `action`; `full` gives the legacy
  complete structure.
- `--branch NAME_OR_ID` — read a branch by name, internal ID, or `main`
  instead of the default branch. Selecting a branch for a read never changes
  the default branch, sessions, claims, or your Git checkout.

`ready` lists work items that can be picked up, with diagnostics, claim
information, and truncation hints. `--limit` defaults to 10 and accepts 1 to
100; `--branch` works here too:

```sh
awr --project /absolute/project ready --limit 25
```

## Reading work items

`work show` gives one item in full — task, acceptance criteria, dependencies,
decisions, evidence, and provenance. `--source-sha SHA` reads it as of a
specific source revision; `--branch` reads it on another branch:

```sh
awr --project /absolute/project work show W-123 --source-sha abc123
```

`search` finds items across the project. The query text is positional;
`--type`, `--status`, `--work`, and `--limit` narrow the results:

```sh
awr --project /absolute/project search "payment retry" --type work --limit 20
```

## Compiling context for a task

`context compile` assembles the L1 execution context — the budgeted working
set for a task — with completeness, gaps, provenance, and the Context hash:

```sh
awr --project /absolute/project context compile --work W-123
```

All of these are optional: `--session` and `--detached` control session
binding; `--agent`, `--intent`, and `--budget` steer the compilation;
`--goal`, `--path`, and `--tag` are repeatable; `--source-sha` compiles
against a specific source version; `--checkpoint` and `--after-revision` set
custom baselines and are mutually exclusive. `--branch ID` uses the ordinary
context baseline on another branch — a different meaning from `branch
context`, which compiles a named branch's increment since it forked without
switching your default branch:

```sh
awr --project /absolute/project branch context feature-x --work W-123
```

The default text output is the budget-constrained execution context with
complete acceptance criteria and hard rules preserved. If required gaps exist,
the JSON result has `ok=false` with `error.code=ContextIncomplete`; if hard
facts exceed the budget you get `BudgetExceeded` — never silently truncated
facts.

## Moving work forward

Work items change state through `work` subcommands:

```sh
awr --project /absolute/project work progress W-123 \
  --session S-1 --reason "starting implementation" --expected-revision 10
```

The same shape applies to `block`, `unblock`, `cancel`, and `reopen`, with
`--next-action`, `--summary`, and `--blocker` carrying the corresponding
details. Completion binds acceptance criteria and evidence from a JSON file
and releases this session's claim on success:

```sh
awr --project /absolute/project work complete W-123 \
  --session S-1 --reason "all checks green" \
  --input /absolute/completion.json --expected-revision 10
```

## Recording events and evidence

`event append` writes to the project log:

```sh
awr --project /absolute/project event append \
  --type note --summary "reviewer asked for retry tests" \
  --expected-revision 11
```

Optional flags: `--work`, `--session`, `--branch`, `--importance` (default
`normal`), and `--payload FILE` (a JSON value file, `{}` when omitted). The
default output shows the ID, type, short summary, and version — it does not
echo the payload; use `event show --full` to expand an event explicitly. You
cannot forge reserved domain events such as `work.completed`; those come only
from the state-change commands.

`evidence add` records evidence from a JSON input file:

```sh
awr --project /absolute/project evidence add \
  --input /absolute/evidence.json --expected-revision 11
```

Input fields include `work_item_key`, `branch_id`, `external_key`,
`evidence_type`, `level`, `summary`, `locator`, `sha256`, `source_sha`,
`command`, `scope`, and `verified_at`. A successful write returns the saved
record and an `event_id`, but recording is not execution:
`validation_basis=caller_supplied_bindings` means AWR stored the bindings you
submitted — not that it ran your command or that business acceptance passed.
`evidence show` gives the summary read view of a record; it is not a write
receipt.

Path and size rules: relative paths in evidence input and event payloads
resolve against the `--project` root; relative paths in a completion input
resolve against the process's current directory, so use absolute paths in
automation. Evidence input and event payload files are capped at 1 MiB,
completion inputs at 64 KiB.

## Revisions and optimistic concurrency

Writes take `--expected-revision R`, where `R` is the most recent
`project_revision` you observed. If the project has moved on, the write fails
with `RevisionConflict` instead of silently overwriting someone else's work —
re-read with `status` and retry against the new revision. Three version
numbers appear in output and are not interchangeable: `revision` is an
object's version, `source_revision` is the source's version, and
`project_revision` is the project state version.

If a write is interrupted or partially applied, check the proposal, events,
session, and current revision before deciding what to do next — do not blindly
retry after a process failure. Two workspace errors deserve mention:
`WorkspaceConflict` (from `awr workspace`) means both sides edited the same
tracked file and nothing is auto-merged; `WorkspaceContended` means a
`publish` or `drop` was preempted three commits in a row, and the remedy is
simply to run the same command again.

## JSON output, errors, and exit codes

With `--json`, a successful command prints exactly one JSON object on stdout.
Errors are typed JSON on stderr:

```json
{
  "code": "RevisionConflict",
  "message": "revision conflict: expected 10, actual 11",
  "details": {"expected": 10, "actual": 11}
}
```

Parse the two streams separately — never merge them with `2>&1` and parse the
combined text. `code` and `details` are the machine-readable basis for
decisions; `message` is for humans. `--json` may sit before or after a known
subcommand; put it before the command to get JSON errors for unknown top-level
commands. Exit codes:

- `0` — success (also `--help` and `--version`).
- `1` — a domain error (typed error on stderr), possibly with a partial,
  inspectable body on stdout; also `Unsupported` for unimplemented top-level
  commands.
- `2` — missing or invalid arguments or unknown nested subcommands;
  `InvalidInput` in `--json` mode.

## Where the CLI ends and MCP begins

The CLI and the MCP server expose the same domain state: for the shared work
and context tools, both return the same identifiers, versions, acceptance
criteria, dependencies, diagnostics, and gaps under the same project, branch
selection, indexed source, and parameters. The difference is posture:

- The CLI is the full project management entry point, and its reads may
  refresh rebuildable database projections (`source_refresh`).
- MCP read tools are strictly read-only (`read_only=true`). If the source has
  changed, an MCP read refuses the stale snapshot with `SourceStale` instead
  of refreshing; run `awr source reindex` and read again.
- MCP stdio exposes a fixed tool catalog for agent clients (the shared HTTP
  service adds project directory tools); administration beyond that catalog
  stays in the CLI.

In practice, you drive setup, administration, reindexing, and ad-hoc
investigation from the terminal, while your agent client calls MCP tools
during a session. See [MCP tools](mcp.md) for the agent side and
[Troubleshooting](troubleshooting.md) when something goes wrong.
