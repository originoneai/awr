# Team CLI reads

Source-development interface: these commands require a separately deployed Team
service. They do not imply that Team service support is included in the published
0.6.0 packages. Enable the [Team runtime switch](team-switch.md) before use.

## Connect to a project

Reuse a project-scoped credential issued for your actor/client. Set its value in
an environment variable through your host's secret handling, then store only the
variable's name in the remote profile:

```sh
export AWR_TEAM_ENABLED=1
awr --project /path/to/project remote add team-dev \
  --endpoint https://team.example/awr \
  --project-key demo \
  --credential-env AWR_TEAM_TOKEN
awr --project /path/to/project --json team capabilities --remote team-dev
awr --project /path/to/project team work next --remote team-dev
```

`AWR_TEAM_TOKEN` must already contain the issued Bearer credential. No credential
value is stored in the profile or accepted through a new CLI token flag.
`--project` locates the local switch and `.awr/team-remotes/NAME.toml`; the
profile's `project_key` is the server's public project alias. The service binds
tenant, project, actor, client and permissions from its registration and credential.

The endpoint is a **service base address**. This example sends requests to
`https://team.example/awr/v1/projects/demo/query`. Path prefixes are retained,
trailing slashes are normalized, and the alias is encoded as one path segment.
Use HTTPS, or HTTP on loopback for local deployments. Endpoint credentials,
query strings and fragments are rejected. Do not configure an MCP URL or a full
query route: the CLI appends the route without guessing or stripping suffixes.

`awr team capabilities` without `--remote` retains its local, network-free
capability report. With `--remote`, it now performs authenticated discovery and
reports connection/authentication failures; it no longer returns the local
capability report for a configured remote. Remote service commands advertised
by capabilities do not imply CLI write support. The existing `team command
--body JSON [--remote NAME] [--offline]` entry still validates locally or refuses
remote submission. `team command inspect` only queries the original request;
it cannot be combined with `--body` or `--offline`.

## Commands and selectors

All remote reads explicitly require `--remote NAME`. Global `--project` and
`--json` work before or after known subcommands.

| CLI after `awr team` | `awr_team_query` operation | Optional parameters |
| --- | --- | --- |
| `capabilities --remote NAME` | `capabilities` | None |
| `workstream list --remote NAME` | `workstreams.list` | `--limit`, `--cursor` |
| `work next --remote NAME` | `work.next` | `--limit`, `--cursor` |
| `work inbox --remote NAME` | `work.inbox` | `--limit`, `--cursor`, `--max-context-bytes` |
| `work list --remote NAME` | `work.list` | `--workstream`, `--limit`, `--cursor` |
| `work search TEXT --remote NAME` | `work.search` | `--workstream`, `--limit`, `--cursor` |
| `work prepare [WORK] --remote NAME` | `work.prepare` | `--session`, `--workstream`, `--max-context-bytes` |
| `work snapshot [WORK] --remote NAME` | `work.snapshot` | `--session`, `--workstream`, `--max-context-bytes` |
| `work observe [WORK] --remote NAME` | `work.observe` | `--session`, `--workstream`, `--max-context-bytes` |
| `session inspect SESSION_ID --remote NAME` | `session.inspect` | `--work`, `--workstream` |
| `claim inspect CLAIM_ID --remote NAME` | `claim.inspect` | `--work`, `--session`, `--workstream` |
| `execution inspect EXECUTION_ID --remote NAME` | `execution.inspect` | `--work`, `--session`, `--workstream` |
| `command inspect REQUEST_ID --remote NAME` | `command.inspect` | `--work`, `--session`, `--workstream` |
| `work recovery [WORK] --remote NAME` | `work.recovery` | `--session`, `--workstream` |

Prepare, snapshot, observe and recovery require WORK or `--session ID`, and allow both;
the service checks that selectors agree. A workstream alone cannot select a task.
Claim, execution and command inspection require `--work WORK` or `--session ID`
in addition to the positional record ID, and also allow both. Record IDs do not
replace task scope; the CLI never looks up a work ID in a second request. Session
inspection uses its required positional session directly, with optional
`--work WORK` to confirm task scope. All selectors remain subject to the service's
current authorization and consistency checks.

`--workstream` is a ULID, not a title or external key. If list/search cannot
uniquely select a workstream, specify one from `workstream list`; the CLI does
not pick the first visible stream or aggregate across streams. Search is a
case-insensitive **literal title substring** within the selected workstream.

Limits are 1..100; byte budgets are 1..262144, not token counts. Unspecified
options are omitted so the service remains responsible for defaults. Current
defaults are 50 items for workstream/list/search, 20 for next/inbox, and 65536
bytes for budgeted reads. Work, session, claim, execution and request selectors
are nonempty, at most 128 UTF-8 bytes without controls; they are not trimmed or
required to be ULIDs. Search is at most 512 bytes. Cursors are opaque strings
of at most 4096 bytes.

```sh
awr team work list --remote team-dev --workstream WORKSTREAM_ULID --limit 10
awr team work search 'Observer' --remote team-dev --workstream WORKSTREAM_ULID
awr --json team work prepare WORK_ID --remote team-dev
awr --json team work snapshot --session SESSION_ID --remote team-dev \
  --max-context-bytes 131072
awr --json team work observe WORK_ID --remote team-dev
awr --json team session inspect SESSION_ID --work WORK_ID --remote team-dev
awr --json team claim inspect CLAIM_ID --work WORK_ID --remote team-dev
awr --json team execution inspect EXECUTION_ID --work WORK_ID --remote team-dev
awr --json team command inspect ORIGINAL_REQUEST_ID --work WORK_ID --remote team-dev
awr --json team work recovery WORK_ID --remote team-dev
```

These are business reads. They do not claim work, start execution, report
progress, grant authority, or initialize Personal SQLite state. Normal Team
access audit records are still written. Session/claim/execution, handoff,
planning, access and delivery/review writes continue to use existing interfaces.

## Results, completeness and pagination

JSON mode emits one original service result object on stdout. Keep stdout and
stderr separate. There is no added `result` wrapper: capabilities and
workstream lists are flat, while other operations use their service envelopes.
Unknown fields, nulls, versions, context hashes, permissions and gaps are retained.
Ordinary output summarizes navigation pages; detail reads, including runtime
inspection and recovery, retain the full returned object with terminal controls
escaped. Missing observations are not converted to zero usage,
successful completion, or proof of a running process.

**Exit 0 means a successful read.** A valid Team prepare/snapshot result with
`data.context_complete=false` still exits 0, matching the Team MCP read. JSON
stderr is empty; the flag, `completeness_reasons` and guidance remain in stdout.
Human output explicitly warns about incomplete context. A script that needs
complete context must also check `data.context_complete == true`. This differs
from Personal `work prepare`'s incomplete-context exit convention. A server
error such as HTTP 409 `ContextIncomplete` exits 1 with no result on stdout.
Neither complete context nor a successful read grants execution admission.

Runtime inspection also exits 0 for ended sessions, expired/released claims and
failed/settled executions. `lease_live`, `effects_settled`, `artifact_verified`
and `settlement_basis=caller_asserted` describe different facts; successful
inspection does not renew a lease, resume execution or prove independent
acceptance. Permission-hidden receipt details remain null.

`command inspect` accepts the **original request ID** and preserves the service's
`data.state`. Both `committed` and `unknown` are successful reads (exit 0).
`unknown` means no committed receipt is currently visible; it does not establish
that the original command failed, never ran or cannot still commit. Human output
states this uncertainty. The CLI does not generate a new request ID, replay a
command or repeat external effects. Planning and delivery outcome protocols
retain their separate entry points.

`work recovery` returns at most two session/checkpoint candidates belonging to
the current ownership binding. It is not full session history. Empty `items`
is a successful read. Checkpoint actions and context hashes are historical;
follow current guidance and read fresh `work prepare` before continuing. The CLI
does not automatically resume work. Session/claim/execution/command inspection
and recovery do not accept `--limit`, `--cursor` or `--max-context-bytes`.

Each invocation issues one query. For paginated operations, pass `next_cursor`
unchanged with the same operation and filters to continue. The CLI does not
preflight capabilities, automatically retry, follow hints, or restart an expired
cursor. After source/permission changes, start a fresh traversal explicitly.

An inbox page can have no items **and still have a continuation cursor**; it
does not establish that the project has no coordination items. Continue until
the cursor is null. Next's resume list is limited to 20 independently of item
pagination; `resume_truncated` remains visible. A `next_query` may name an
operation outside the fourteen CLI reads; use MCP for unsupported operations.
Snapshot uses one service transaction, rather than combining separate reads.

## Failures and verification

Domain/transport failures exit 1; usage errors exit 2. In JSON mode errors are
one object on stderr. Server codes and diagnostic extensions are retained,
including unknown codes and errors such as `{"code":"Busy"}` without a
message. Consumers must use `code`, not require an error message. Existing
Personal errors retain their previous format.

Client failures use `TeamTransport`, `TeamTimeout`, `TeamRedirect`,
`TeamHttpError`, `TeamResponseInvalid`, or `TeamResponseTooLarge`, with a message
and structured details. Requests have a 35-second total deadline, never follow
redirects, and accept at most 2 MiB of response bytes. This transport limit is
separate from the service context budget. No truncated result or raw proxy HTML
is printed as a successful read; HTTP client diagnostics and credentials are
not echoed.

`team_query_cli` tests invoke the native CLI against a mock service. The server's
`workstream_cli` tests additionally compare all fourteen reads against
authenticated MCP with a process-owned PostgreSQL fixture. Runtime records are
created through domain commands, inspected before and after termination, and
checked for unknown results and scope mismatches. A second client verifies that
existing execution receipts are hidden from other readers, committed
command outcomes remain client-bound, and all five runtime reads reject a client
after its target workstream read grant is revoked, including through an existing
MCP connection. These tests preserve domain facts and check normal audit writes
separately:

```sh
cargo test -p awr-cli --test team_query_cli --test team_cli --test team_switch --locked
cargo test -p awr-server --features pg-tests --test workstream_cli --locked
```

The PostgreSQL fixture uses only `AWR_TEAM_TEST_DATABASE_URL` (or its documented
loopback default) and creates a uniquely named test database. It never adopts
the runtime database. It builds the native CLI through Cargo to locate the
correct executable. Interface/fixture coverage does not claim live Agent
workflow acceptance or released package support.
