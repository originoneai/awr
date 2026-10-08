# Host application contract

A host can invoke a pinned native `awr` binary with an argument array and separate
stdout/stderr pipes. AWR does not require a visible terminal, model client, HTTP
service or daemon. Pass the executable's absolute path; never interpolate project
paths or user content into a shell command. A host owns its UI, client credentials,
scheduling, notifications and knowledge processing. AWR owns the source-backed work
projection, runtime records and its supported source mutations.

Hosts that need a central service can use AWR 0.4.0's
[shared MCP endpoint](mcp-service.md). One process routes explicit project keys
for multiple authenticated clients and persists host conversation/session bindings,
checkpoints, waits and request outcomes. This is an alternative transport; host UI,
model execution and wakeup scheduling remain host responsibilities. The shared
service and lifecycle extensions are included in the 0.4.0 packages.

The runnable [no-UI host example](../../../examples/host-app/README.md) demonstrates
these operations with argv, protected JSON, complete paging and explicit generic
client continuation. Its synthetic fixture checks are separate from native-client
or application business acceptance.

## Discover before opening a project

```text
awr capabilities --json --protocol-version 1 --require source.read --require context.compile
```

This command neither opens a project nor reads, creates or migrates its database.
It works before initialization and ignores the global `--project` argument. Its
JSON contains:

| Field | Meaning |
| --- | --- |
| `protocol.name`, `protocol.version` | `awr.host` capability negotiation contract, currently version 1 |
| `program` | Build's program version and native OS/architecture, not proof of other platform releases |
| `database` | Current schema, read-only compatible schemas, schemas eligible for migration and future-schema rejection policy |
| `source_adapters` | Accepted adapter IDs and actual read/write limits |
| `capabilities` | Stable IDs, availability, command entrypoints and coded limitations |
| `scope` | Build capabilities only; never project-specific permission or readiness |
| `source_write_performed`, `runtime_write_performed` | Both false for discovery |

Hosts may ignore new fields and unrequested capabilities within protocol 1. Existing
IDs keep their meaning; a materially different operation requires a new ID or protocol.
The program version remains separate: pin the executable and its published checksum
for the command schemas you consume. This negotiation version does not retroactively
make every historical CLI result a new uniform envelope.

Repeat `--require` to check all prerequisites. An unavailable required capability
fails before project access. `CapabilityUnavailable.details.unknown` contains IDs
this build does not recognize; `details.unsupported` contains recognized but
unimplemented capabilities. Both lists are deterministic and deduplicated.
`ProtocolUnsupported.details` returns the requested and supported protocol versions.
Do not parse English messages to identify either condition.

## Preserve CLI outcomes

Use `--json` on operational commands. Successful JSON goes to stdout. Typed errors
go to stderr as `{ "code": "...", "message": "...", "details": ... }`; optional
details vary by error. Exit 0 means command success, 1 means a runtime/domain error,
and 2 means usage/argument error. Help and version flags intentionally return text.

A nonzero exit may accompany a useful partial stdout result, for example source
refresh failures. Preserve both streams and the exit code. A parseable stdout object
alone never means success or complete traversal. Timeouts or a lost pipe mean the
write outcome is unknown: inspect the existing proposal/attempt/execution/session
before retrying. Do not synthesize `source_write_performed: false` from a transport
failure. Input paths and maximum sizes remain command-specific (`--input`, reviewed
drafts, or documented lifecycle JSON on stdin); there is no universal JSON RPC wrapper.

## Bind intake to the reviewed effects

`init --json` (including `--manifest` or a reviewed `--from-draft`) returns
`preview.fingerprint`, the source/configuration mapping, bounded source snapshots,
source issues, and a `writes` inventory with before/after fingerprints and exact
text for the manifest, `.gitignore` and generated intake files. Runtime database,
WAL and temporary staging effects are listed separately. This preview performs no
project writes; an explicitly requested `--write-draft` still writes its output file.

The version 1 `preview.semantic` report parses the exact captured source bytes through
the normal adapters and projection constraints in private RAM. Existing database and WAL
files are captured with bounded repeated byte/identity/mtime checks, then opened by
SQLite in an owner-only temporary directory and copied into RAM. The temporary copy
is removed after use; SQLite never opens the project database during preflight.
Concurrent writes or rollback journals reject capture for a later retry. Compatible
migrations happen only in RAM. It returns
`can_apply`, `can_execute`, organization gaps and their total/truncation, plus stable
source identity/revision/configuration bindings. A generated preview is not evidence
that it can be applied. `can_apply: true` permits intake; unresolved goals/references
can still make `can_execute: false`. Neither flag proves business acceptance. The
snapshot is bounded to 256 MiB; larger databases return a preflight issue.

Pass that fingerprint as `init --accept --expected-preview <fingerprint>` with the
same input/mapping arguments. Changed sources, mapping semantics, existing manifest
bytes or ignore contents reject the old preview. Existing clients may omit the new
flag; semantic preflight still applies. A draft file's original inventory fingerprint
is still checked; editing a draft requires previewing that edited draft before
acceptance. Multiple source candidates remain an explicit ambiguity.

Repeated initialization keeps the existing manifest, project/work identities,
events, sessions and checkpoints. Non-Git projects are supported. Preview can read
a read-only project, but initialization requiring runtime writes is rejected there.
Malformed sources and predictable identity conflicts now return `SourceStale` before
configuration or runtime writes. Failures arising after writes start can still leave a
partial index; retain the nonzero result and inspect the reported effects. It must not be shown
as a fully successful import. Initializing multiple files is not a cross-file atomic
transaction; interrupted staging may require inspection before retry.

To change an initialized project's source mapping, provide a candidate manifest:

```text
awr source configure --manifest candidate.toml --json
awr source configure --manifest candidate.toml --accept --expected-preview <fingerprint> --json
awr source configure-status <fingerprint> --json
```

Configuration changes preserve project name, external key and authority mode.
They require a reviewed fingerprint, leave business source bytes untouched and
reindex the same project. Before/after manifests and a durable receipt are kept in
the ignored mutation directory; the preview fingerprint locates the receipt even
if the return pipe was lost. `configure-status` reads the recorded outcome and
compares current configuration fingerprints without applying or reindexing anything.
Pending, externally changed or partially indexed results are distinct from success.
A failed projection does not mean the configuration was unwritten; inspect the
receipt and current configuration before acting. Identical configuration returns
`no_change`. Configuration setup is separate from task/document source editing.

## Read a coherent query snapshot

Status, ready work, work details, object catalogs, search, and context compilation
refresh through a shared source transition guard before reading one SQLite snapshot
in RAM. The lock waits at most five seconds. Source projection steps retry only
optimistic runtime revision conflicts, at most four attempts; source/permission/parse
failures remain failures. A pending relocation is explicitly recoverable, not a reason
to silently reuse stale facts. Runtime write commands retain their expected-revision
checks against the live database.

Query responses include version 1 `snapshot` metadata: the coherent query revision,
a `source_state_fingerprint` derived from retained source IDs, revisions, fingerprints and
parsing configuration, and the source refresh revision. Runtime-only events advance
the project revision without changing the source state fingerprint. A response is a snapshot at
its recorded revision; later events do not make its internally consistent facts false.
Context hashes already bind their source and runtime snapshot identities.

`status --cached`, `ready --cached`, `work show <key> --cached`, `work graph --cached`,
`object list <kind> --cached` and `search --cached` read the last recorded projections
without opening business files or writing project files. They explicitly return `read_only: true`,
`source_refresh_performed: false`, `freshness_basis: last_recorded_source_state` and
`snapshot.source_currentness_verified: false`. Cached facts do not establish current
progress. The temporary capture is bounded to 256 MiB and may reject an active writer;
retry after it settles. It never ignores WAL data or upgrades the live schema.

MCP read operations remain read-only and still require source files to match indexed
facts. They return their coherent revision/source state fingerprint with no source refresh revision.
A later runtime event alone does not invalidate that read; source drift still rejects it.
CLI refresh reads can update the projection cache, so `read_only` remains false for
the overall operation even though the query itself uses RAM. Negotiate this behavior
with capability `query.coherent_snapshot`.

## Relocate one source while retaining its identity

```text
awr source relocate --source <source-id> --to plans/work-ledger.yaml --json
awr source relocate --source <source-id> --to plans/work-ledger.yaml --accept --expected-preview <fingerprint> --json
awr source relocate-status <fingerprint> --json
awr source relocate-recover <fingerprint> --json
```

Version 1 relocates one explicit relative file mapping inside the same project.
The destination must already contain the exact indexed bytes. The original may be
present or already moved by the user; AWR does not move, delete or overwrite either
business file. Change content separately. The preview binds the retained source ID,
source revision/configuration, both paths, destination bytes and exact before/after
manifest text. Adapters must produce the same object keys and IDs in the semantic
preflight. Path-derived keys and directory/Git mappings are explicitly unsupported;
YAML ledgers with stable explicit keys are supported. An identical hash alone never
merges source identities. Any destination owned by another retained source conflicts.

Acceptance retains source/object identities and runtime sessions, claims, checkpoints,
dependencies and evidence. It appends `source.relocated` with before/after locators;
`source history`, `event show --full` and `source changes` expose the history. Old
checkpoint references remain immutable. Current projection references use the new
location after reindexing. Manifest formatting changes are included in the exact
preview and require that fingerprint.

The SQLite binding and manifest rename are individually durable, not a cross-file
transaction. A source transition lock coordinates refresh/configuration writers;
an interrupted relocation leaves a pending marker that prevents ordinary refreshes
from inventing a replacement source. Read-only status reports the journal and actual
bindings. Explicit recovery accepts only the recorded before/after manifest and
source states and unchanged target bytes, then completes the missing steps. Changed
user files remain untouched. A repeated completed request returns its existing receipt
and observations without applying it again. Preserve mutation journals in matching
runtime backups; do not delete a pending marker to bypass recovery.

## Traverse the complete catalog

```text
awr object list work --limit 20 --json
awr object list work --cursor '<next_cursor JSON>' --json
awr object list source --scope all --json
```

Use `object list` for `goal`, `plan`, `rule`, `work`, `decision`, `source`,
`relation`, `artifact` and `evidence`. Each page contains at most 200 objects
(default 20), an exact `total` for the selected scope, `has_more`, and a nullable
`next_cursor`. Repeat the same kind and scope. The cursor binds the project identity,
project revision and last stable object ID. Any intervening source or runtime revision
requires a new traversal (`RevisionConflict`); do not append pages from different
versions. A changed title retains identity when the source's explicit key is stable.
Title-derived keys can change identity; AWR does not guess a rename from similarity.

Scope defaults to `active`. `retired` includes projections removed from a source or
whose source was unregistered; `all` combines both. These scopes describe retained
index membership, not completion, archive status or a reporting period. `active_total`
and `retired_total` remain separate. Select the desired milestone/relations for period
reporting; never infer engineering verification from `status: completed`.

Lists preserve source references, object/source/project revisions, raw status and
owner values alongside normalized status. Owner strings are source data, not runnable
Agent identities or runtime claims. Use `work show`/`ready` for readiness diagnostics
and current claims, and `object list relation` for dependencies and groupings. Unknown
states remain unknown; custom language or code-wrapped spellings use the project's
explicit `status_map`. Original bytes remain available through the source reader.

Long text is summarized and `content_included` is false. Drill down with
`object show <kind> <id> --full`, `decision show --full`, `source show --content`,
`artifact show`/`cat`, or `evidence show --content`, using their byte/version guards.
Source content reads require a current registration and the indexed fingerprint;
retired/revoked sources retain metadata and history without granting new file reads.
Explicit cached object reads expose historical projections, not fresh file content.

A failed source refresh can still return a useful page on stdout with a nonzero
`SourceStale` result. Its `total_basis` is `retained_indexed_objects` and
`total_is_current` is false; inspect `source_issues` and each source's freshness.
The retained count is not the authoritative total of an unreadable source. An empty
partial index must not be displayed as a confirmed empty project. Process stdout,
stderr and exit code together. Existing status/ready summary limits are unchanged.

## Current read/write limits

`source.read` and `object.read` operate on registered references with bounded body
reads. Status, ready and summaries do not provide a complete project catalog. Consult
`project.catalog` before relying on complete traversal. Queries that refresh source
projections can write the runtime database; `read_only` and freshness fields are part
of the result, not an inference from the command's name.

`mutation.yaml.record` and `mutation.yaml.lossless_fields` support selected YAML
fields through source-bound proposals. Field spans preserve unrelated bytes, comments,
key order and line endings. Existing scalar styles are retained when the new value can
be represented in that style; a plain string that would become a boolean is quoted.
Block style is retained with chomping adjusted to the new value. Unsupported anchors,
aliases, tags, duplicate/complex keys, comment-bearing collection replacement and
ambiguous scalar forms fail before writing. The entire result must have precisely the
expected semantics and reparse through the source adapter. See the
[YAML writer](../../../adapters/yaml-ledger/README.md) for supported shapes.

`mutation.markdown.document` supports registered heading, rule and decision documents.
Markdown work ledgers support the finite stable-ID forms below. Reviewed related batches and explicitly scoped ordinary completion have separate capabilities and guards described below. Never route an unsupported operation through a generic status patch or a second writer.

## Edit a registered document

Read the complete registered source with `source show <source-id> --content` and retain
its fingerprint. Document text is data and is never executed. Send this versioned JSON
through a protected file:

```json
{
  "version": 1,
  "request_key": "host/edit-goal/1",
  "change": {
    "operation": "edit",
    "source_id": "<source ULID>",
    "source_fingerprint": "sha256:<original source hash>",
    "edit": {"kind": "fragment", "before": "Original paragraph.", "after": "Updated paragraph."}
  }
}
```

`edit: {"kind":"replace","text":"<complete replacement source>"}` replaces the whole
document. Fragments must be nonempty and match exactly once; other bytes, including
code and line endings, are retained. Whole replacements must explicitly include the
material the host intends to keep. Both forms reparse through the registered adapter
and retain object keys, lifecycle status and rule constraints. Use explicit heading
anchors when changing titles. Conflicting metadata declarations are rejected before
writing; the host must resolve the indicated declarations. A plan source may set
`options.kind = "architecture"` to expose a typed architecture reference without AWR
interpreting or rendering the architecture.

```text
awr document change --input edit.json --json
awr document change --input edit.json --accept --expected-preview <preview-fingerprint> --expected-revision <preview-revision> --json
awr document status --key <request-key> --json
awr document recover --key <request-key> --expected-revision <current-revision> --json
```

Acceptance binds the complete request, source bytes, mapping and project revision.
The request key is project-scoped: identical delivery returns the retained outcome;
changed content conflicts. Lookup does not refresh or write the runtime. A missing
response requires lookup first; pending operations need explicit recovery. Recovery
only writes at the recorded before state or completes projection at the recorded after
state; external changes are retained. A request's `completed` phase describes that
document operation, not completion or adoption of any project work.

For a new draft, use `change: {"operation":"create_draft","path":"decisions/new.md",
"title":"Proposed approach","body":"Draft text."}`. Its parent directory must already
exist inside exactly one registered Markdown decisions directory. AWR allocates a
stable `DOC-…` key and writes source status `proposed`. Publication uses an atomic
no-clobber operation: a same-name file, including a concurrent creator, is never
overwritten. Relative visible Markdown paths are required; registration does not expand
implicitly. An unchanged document save returns `no_change` without a source write or
new business event when projections are current. No Agent session or model is created.

## Create a task with a stable request identity

```text
awr work create --title 'Prepare the travel checklist' --request-key <host-request-id> --json
awr work create --title 'Prepare the travel checklist' --request-key <same-id> --accept --expected-preview <fingerprint> --expected-revision <preview-project-revision> --json
awr work create-status --key <host-request-id> --json
awr work create-recover --key <host-request-id> --expected-revision <current-project-revision> --json
```

Alternatively, `work create --input request.json` accepts protected JSON containing
`version: 1`, `request_key`, `title` and optional `source_id`. Without an explicit source,
exactly one primary ledger must be registered. Creation currently supports ordinary
YAML lists and keyed maps, including flow/empty collections, configured field/status
names and CRLF. Existing bytes and work IDs are retained; new block records use the
existing indentation. Unsupported YAML forms fail before writing.

The request ID belongs to one project and one exact creation payload. Its stable new
`WORK-…` key is included in the preview. Acceptance binds the request, source mapping,
source fingerprint, before/after text and project revision. Repeated identical requests
return the recorded work ID and outcome, even with an old preview revision. Changed
content under an existing key is a conflict. A busy concurrent writer may return
`MutationConflict`; query the request before another attempt. A later source change
does not authorize reusing the same request to create or restore another task.

Only a title is needed from the user. The new source status is `draft` (or its explicit
mapped spelling), with missing execution facts left missing. It is a draft, never
automatically executable or completed. Draft is excluded from the execution queue; normal readiness/context/domain rules still
apply. No session, runtime claim, model call or invented acceptance is created.

Creation retains a plan and before/after snapshots in the ignored mutation directory,
shares the existing source writer lock and atomic replacement primitive, and verifies
the resulting projection. `phase` describes the creation request, not work completion.
`source_write_performed`/`runtime_write_performed` describe this invocation; a replay's
historical `write_outcome` is separate. Pending results require explicit recovery.
Recovery only installs the reviewed after snapshot when current bytes match before,
or finishes projection when they already match after. Other bytes or changed registrations
are retained and reported as conflicts. Recovery never rolls back newer user edits.

`create-status` is read-only and distinguishes its historical receipt from the current
source observation (`before`, `after`, `externally_changed`, or unavailable/registration
changed). A transport failure cannot be interpreted as unwritten. Read the same request
key first; pending writes are never automatically repeated by the create command.

`work complete` retains the engineering contract: a real session/claim and evidence
covering the source's acceptance criteria at the supplied source SHA. A source's raw
completed status is a separate assertion. Registering evidence does not run its
`command` field and does not itself verify the report.

`execution.external.register` stores an external reference with an unverified outcome.
It does not adopt a PID or create a managed supervisor. Native client resumption is
not implied by `session.resume`. Generic lifecycle callbacks can save checkpoints
without installing hooks; caller-provided summaries remain caller assertions. An
artifact import copies content, while references in evidence/events keep their own
documented read and verification rules.

## Consume changes without losing failures

`source scan` observes current registrations and availability; it does not parse
pending content. `source reindex` refreshes projections. Both return a `change_window`
with `after_revision` and `through_revision`, plus `projection_complete` (false for
pending or failed sources). A fully unchanged refresh creates no new source event or
work identity. Keep stdout, stderr and exit code for partial failures.

Read immutable source receipts, including changes missed before a host restart:

```text
awr source changes --after-revision <last-successfully-processed-revision> --json
awr source changes --cursor '<next_cursor JSON>' --through-revision <returned-bound> --json
```

This command is read-only and reuses the existing event cursor/order. Pages contain
at most 100 source events (default 20); runtime events do not consume that limit.
Hold `through_revision` fixed while paging. Each receipt exposes the source's before
and after versions/freshness, content/configuration/membership changes and an exact
projection change count. `observation_only` is true when the event changes neither
content, configuration, membership nor objects; a freshness observation may still
mean the source needs reindexing. Scanning is not a knowledge-processing result.

Object change details stay in their original `source.projected`/retirement event.
Use the returned `detail_command` to read its bounded payload, including stable IDs,
added/updated/removed actions and before/after object revisions. A summary never
stands in for omitted details. Unknown historical change schemas return an explicit
partial result requiring a full refresh/rebaseline; they are not guessed compatible.

`pending_source_total` and bounded `pending_sources` describe the **current retained
index state**, independently of the historical event window. A nonzero `SourceStale`
result retains useful stdout and withholds `next_after_revision_when_processed`.
An unreadable or malformed source stays stale/unavailable; repeated failure can have
no new event while the pending source remains visible. Retry the failed refresh and
resume from the last successfully processed window. `source list` exposes all retained
active source metadata if more than 50 pending summaries were omitted. Reads alone do
not check whether previously fresh files changed since the last scan/index.

Successful directory discovery can prove a child is no longer selected, and removing
a manifest mapping explicitly retires its source. Failure to read a file/directory,
parse it or access an authorized root does not prove retirement. The `issues` from
scan/reindex preserve typed failures and affected mappings. `retired` means no longer
selected as current authority; it is not a claim that a physical file was deleted.
Events, evidence and completed checkpoints remain in runtime history. Reappearance
under the same source identity/explicit object key keeps its retained IDs. File moves
or changed source identities do not authorize guessing a rename or rewriting history.

When all pages, object changes and pending sources have been handled successfully,
the host may persist `next_after_revision_when_processed` as its own success cursor.
`consumer_checkpoint_updated` is always false: AWR never acknowledges knowledge
weaving or application processing on the host's behalf. A returned revision or an
empty change page alone is not proof that downstream work finished.

## External clients, checkpoints and continuation

One work may have several AWR sessions and several native client conversations.
Keep the host's navigation key, the work's AWR ID/key, AWR session ID, native session
namespace and execution ID separate. Source `owner`, chosen Agent and actual session
Agent are separate facts. For a client without native hooks, call the generic
receiver explicitly; no hook installation or client-private history access is needed.

1. Start an AWR session with `session start` and the actual Agent/provider/model,
   or let `client bind --client generic --external-session <namespaced-native-id>
   --work <key>` create it. Pass `--session <id>` to bind an already created session.
   Binding does not claim the work; claim ownership remains an explicit domain action.
2. `client bind` returns the current context. `context compile --session <id>` can
   return a separately budgeted context, rendered body, hash and completeness result.
   Record actual delivery/receipt in the host; a hash alone proves no model received it.
3. If the host supervises the client, register only an external execution with
   `execution register --session <id> --key <operation-key> --purpose <purpose>
   --reference <host-execution-reference>`. AWR never adopts the PID, launches or
   terminates that client. `execution run` is for commands owned by AWR's supervisor.
4. Save useful progress with `client progress --client generic --external-session
   <native-id> --digest <observed-summary> --next-action <next> --open-loop <issue>`.
   Deliver a lifecycle callback using `client hook --client generic --work <key>`
   and JSON on stdin: `session_id`, `cwd`, `hook_event_name` and optional `turn_id`.
   Events are `SessionStart`, `PostCompact`, `PreCompact`, `Stop`, `SessionEnd`,
   `Interrupt`. Start/PostCompact return context; the others persist a checkpoint.
   Stable repeated callbacks deduplicate against source state and saved progress.
   Changed progress/source facts can legitimately produce a new checkpoint even with
   the same native turn ID. Wait for `awr.checkpoint_saved` before calling a save done.
5. After reopening the host, use `client show` to recover the binding and
   `recovery inspect --session <id>` to inspect the last completed checkpoint,
   pending runtime writes and external/managed execution observations. This does not
   refresh sources, release claims, create a successor, restart or stop a process.
   Combine it with `doctor --json` for read-only current-source/artifact diagnostics;
   preserve nonzero diagnostics. Investigate unfinished writes and unknown effects.
6. Explicitly continue with `session resume` at the current project revision, or
   bind a new native conversation with `client bind --from-session <old-id>`.
   Continuation refreshes sources and compiles new context before work proceeds;
   stale checkpoints do not authorize replay. Repeating a binding keeps its session.
   A raw resume that already created a successor must be inspected, not retried as a
   new continuation. Native resumption remains a separate, client-supported host action.

### Durable external reports

`execution report --input report.json --expected-revision <revision>` records a
version 1 `ExternalExecutionReport`. For example, a synthetic host might send:

```json
{
  "version": 1,
  "request_key": "guide-execution/stage-2",
  "execution_id": "<registered AWR execution ULID>",
  "host_id": "example-host",
  "host_work_key": "workspace/guide",
  "native_session": "provider/conversation-id",
  "agent_id": "guide-author",
  "origin": "host_observed",
  "phase": "waiting_user",
  "observed_at": 1,
  "summary": "The draft needs the user's choice of examples.",
  "detail_references": ["host://example/logs/guide"]
}
```

Use the actual ULID and observation time (Unix milliseconds). Supported phases are
`started`, `progress`, `waiting_user`, `succeeded`, `failed`, `interrupted`, `unknown`.
These are report classifications, not another work/execution state machine. `origin`
is `caller_reported` or `host_observed`; both are host-supplied provenance assertions.
Neither is authentication or an AWR-owned observation. Reports reject managed or
foreign-project executions and cannot overwrite their execution snapshot, create a
worker, verify business completion or change source work status.

The project-scoped `request_key` is immutable: identical content returns the same
event (even with the original revision after a lost response); different content
returns `SourceConflict`. Concurrent identical submissions make one receipt.
First-time stale revisions still fail. Use `execution report-status --key <key>`
before retrying a timeout; `found: false` means no retained receipt was found at that
read, not proof an in-flight writer cannot finish. Report lookup is read-only.

Reports remain recordable after the originating session ends or source parsing fails.
The file is capped at 1 MiB, identifiers at 512 bytes, summary at 8192 bytes, and
references at 32 entries of 4096 bytes each. References are retained, never opened,
copied or authenticated by this operation. Use the host's controlled detail viewer
for original logs. No token streams, client secrets or fabricated native IDs are needed.

The `execution.external_reported` event binds the work, original session, execution
and immutable report. Read its history with the existing event cursor and its full
body with `event show --full`. `execution show`/`inspect` return the latest report
separately; `recovery inspect` includes latest reports and project runtime findings.
An external host's reported `succeeded` still leaves AWR's observation `unknown` and
unverified. Interpret the host report using its actual supporting records before
retrying or completing work. Generic fixture checks and actual Kimi/Grok receiving
context are separate acceptance evidence.

## Compatibility and recovery

The schema declaration describes what this build can open, subject to ownership and
integrity checks. A read-only open requires the current schema. A write-capable open
may migrate eligible older schemas; discovery never does. A newer schema is rejected.
Back up matching program version, original sources, manifest and SQLite runtime
before an upgrade. Source projections are rebuildable; events and checkpoints are
not reconstructed by reindexing source documents. A binary downgrade alone is not a
database rollback procedure.

This development build writes schema 4, which adds the non-executable `draft` work
state. The transactional migration preserves existing rows and references, checks
foreign keys, and retains additional indexes and triggers. Builds that only support
schema 3 must reject this database; restore a matching database/program snapshot
when rolling back. Newly created drafts need an explicit source declaration before
execution; creation itself never supplies missing execution or completion facts.

## One Save from a host

`host save` provides one CLI invocation for a user's explicit edit. It reuses existing
YAML proposals and document recovery journals; it does not create an Agent session,
claim or model call. The trusted local host remains responsible for authenticating its
user and showing the actual edit. The `actor` fields record caller-supplied provenance;
they are not authentication, and every origin retains source registration, filesystem
permissions, version checks and domain rules.

```json
{
  "version": 1,
  "request_key": "host/edit-task/1",
  "actor": {"host": "desktop-app", "subject": "local-user", "origin": "human"},
  "reason": "Save the user's explicit next-action edit",
  "change": {
    "operation": "fields",
    "kind": "work_item",
    "target": "WORK-EXAMPLE",
    "source_fingerprint": "sha256:<source hash from the editor's original read>",
    "fields": {"next_action": "Review the reading notes"}
  }
}
```

```text
awr host save --input save.json --expected-revision <editor-revision> --json
awr host status --key <same-request-key> --json
awr host recover --key <same-request-key> --expected-revision <current-revision> --json
```

Request keys contain 1–512 UTF-8 bytes; the combined host and subject plus separator
must fit 256 bytes. Reasons contain 1–4096 bytes. Field saves support the existing YAML
goal, plan, work and evidence writer's fields. Work completion, ownership and evidence
levels remain protected. The proposal and final event retain the exact request hash
and actor; the saved host journal also binds their source preview and operation.

For AI edits, set `origin` to `ai_accepted`, call `host preview --input save.json`, show
the complete `preview.plan` change to the user, and pass its fingerprint to `host save
--expected-preview <fingerprint> --expected-revision <preview-revision>`. The fingerprint
binds the request, actor, exact patch, target, source and project versions. A changed
patch, target or source requires a new preview. A provenance label alone cannot mark
work completed or bypass a claim-dependent Agent action.

For an Agent acting under authority already delegated by the user, record
`origin: "delegated_agent"` and the actual Agent subject. This also requires the exact
preview fingerprint. The host must enforce the scope of that delegation; the label
does not grant it and does not assert that a human individually reviewed the patch.

Use `change: {"operation":"document","change":<document change>}` for one registered
Markdown edit or new draft through the same host flow. A supported unchanged save
returns `no_change` without creating a proposal or business event. Existing proposal
no-op errors remain unchanged. `host status` is read-only and reports retained outcomes
separately from current source observations and the underlying proposal/document.
An identical request replay returns the retained identity; changed content under the
same key conflicts. A lost response requires lookup before retry. `pending_recovery`
requires explicit recovery; `requires_review` means a terminal proposal stopped, and
its outcome must be inspected before creating a new request. File effects and runtime
effects remain separate; neither a transport error nor a stopped proposal proves the
source was never written. Recovery retains external edits.

After filling a work draft's goal, acceptance and next action, explicitly use
`change: {"operation":"activate_draft","work":"<key>","source_fingerprint":"<hash>"}`.
Activation requires a current, declared primary goal, valid structure, resolved
dependencies and no blocker or existing claim. Standard projects retain rule/milestone
requirements. It changes only `draft` to `planned`, with a protected
`work.draft_activated` receipt; it never asserts completion or invents execution facts.

One project keeps one source ledger. When replacing an existing host writer, switch
only operations AWR actually supports, retain old runtime history as history, and
compare stable references and counts before cutover. Do not turn imported completion
claims into newly verified work. Host integration examples and local component checks
are separate from actual native-client or full application acceptance.


## Explicit ordinary work policy

The default remains strict engineering completion. A ledger can opt specific external
work keys into ordinary confirmations through reviewed source configuration. Use the
existing exact manifest preview/accept or `source configure` contract to select the
configuration; an ordinary `host save` cannot change policy. Policy metadata records
owner authorization, not authentication. The embedding host must enforce the actual
user authorization; do not let an execution Agent rewrite policy to finish its task.

```toml
[project]
name = "Reading"
context_profile = "minimal"
# Existing sources remain registered. In the selected ledger's options:
[sources.options.ordinary_work_policy]
version = 1
policy_id = "reading-v1"
authorized_by = "project-owner"
authorized_at = 1 # replace with the actual authorization time in epoch milliseconds
reason = "Use user confirmation for the selected reading work"
work_items = ["READ-1"]
```

`minimal` explicitly omits separate rule/milestone requirements; configured rules,
real goals, acceptance, next actions and dependencies remain required everywhere.
The policy and its fingerprint are returned by `intake inspect` and included in work
context. Source configuration changes invalidate old previews. Work outside the exact
scope retains strict engineering rules. No source status is upgraded by opting in.

Use the existing host envelope with `change.operation = "confirm_ordinary"`, `work`,
`source_fingerprint`, `policy_fingerprint`, `kind`, `basis`, `confirmed_at` and
`artifacts`. `kind` is `user_confirmation` (human origin required) or `business_check`
(actual local artifacts required). Each artifact contains `locator` and `sha256`.
AWR checks authorized path/read bounds and current bytes, never runs a report command.
Business checks are attributable host assertions with checked artifact references;
they do not imply independent objective verification. All nonhuman edits still need
the exact host preview fingerprint. Release active execution claims before confirmation.

The source stores a typed `ordinary_completion` with actor, authorization policy,
criteria, basis, time and artifact references. The protected applied host receipt must
match before intake counts it. `user_confirmed_completed`, `business_checked_completed`
and `verified_completed` remain separate. Ordinary closure reports
`completed_under_policy`; stale policy, criteria, artifacts or forged source receipts
cannot count as current confirmation. A normal explicit `work reopen` clears the
current ordinary confirmation, retaining its immutable proposal/event history.
The engineering `work complete` input and all existing evidence gates remain unchanged.


## Markdown ledger writes

`markdown-ledger-v1` now exposes adapter revision 3. Reindexing preserves stable
object IDs and runtime history. Tables with explicit ID columns use stable key
pointers; unnumbered legacy tables/checklists remain readable and require an explicit
ID before writing. Lines alone are never sufficient authorization to target a record.

Use the same `host preview/save/status/recover`, proposal lifecycle, work actions and
`work create/create-status/create-recover` as for YAML. The source adapter selects the
writer and reparses before any write. Draft creation requires one unambiguous table
with ID/title/status columns, or one contiguous checklist. Multiple candidate groups,
nested or multiline list targets and unequal table rows require manual editing.

Table cells retain surrounding whitespace, other cells/rows, escaped pipes, code
examples and line endings. Existing code-span cells keep their code style when the
replacement is representable. Array values can be explicit JSON in a cell. A field
without a visible column is stored as a typed, individual inline metadata comment in
the title cell. A checklist uses the same metadata after its visible title:

```markdown
- [ ] Read the article <!-- awr:id="READ-1" --> <!-- awr:status="draft" -->
```

Each `<!-- awr:field=JSON -->` has one authority and one exact value span. Conflicting
or duplicate declarations fail. Only real HTML comments are metadata; escaped text,
inline code and fenced examples remain content. Metadata may carry goal, acceptance,
next action, blockers and protected completion receipts without changing the visible
title. The checkbox must agree with the explicit completion state. New drafts remain
unchecked and unclaimable until explicit activation passes the existing domain gates.

Generic field saves cannot set status, ordinary completion or engineering evidence.
Ordinary confirmation requires the same explicit scoped policy and applied receipt;
engineering completion uses the existing acceptance/evidence contract. The single-file
journal and source reservation rules also cover Markdown, including lost receipts and
external edits during recovery. Internal snapshot filenames are opaque implementation
details; the registered adapter determines their format.


## Same-source batches and archival

`batch change --input request.json` previews up to 100 operations against one registered YAML or finite Markdown ledger. Acceptance requires `--accept --expected-preview <fingerprint> --expected-revision <revision>`. The version 1 envelope contains `request_key`, `actor` (the same provenance fields as host save), `reason`, and `change`:

```json
{"kind":"ledger","source_id":"<source ID>","source_fingerprint":"sha256:<current hash>","operations":[
  {"operation":"fields","target":"READ-1","fields":{"next_action":"Review the note"}},
  {"operation":"import","external_key":"READ-2","title":"Read the follow-up","fields":{"acceptance":["Useful notes"]},"duplicate":"fail"},
  {"operation":"archive","target":"OLD-1","archived":true}
]}
```

Use each stable key once per batch; combine fields in one operation. All operations and the final dependency graph are checked in memory before any source write. Import requires an explicit stable external key and produces a draft. `duplicate: "skip_exact"` only skips an existing unarchived draft with the same title and requested field values; unrelated existing fields remain intact. Conflicts never overwrite an existing entry. Status, verification, identity and completion metadata remain domain guarded.

Archive is an independent `archived` boolean. It retains the original lifecycle status, ID, source record and runtime history. Archived work stays in explicit catalogs and `work show`, is absent from ready selection and current-scope completion counts, and cannot execute lifecycle actions until restored. `source_archived` is reported separately. Archive/restore requires no active executor across any branch. Remove incoming dependencies explicitly or archive the associated dependent scope in the same batch; restoring dependents requires restoring their dependencies too. Cancelled work remains separately counted; moving work out of current scope proves no completion.

`batch status --key <key>` reads the durable outcome without writing. `batch recover --key <key> --expected-revision <revision>` explicitly resumes interrupted writes/indexing. Recovery binds the project, full manifest, reviewed bytes and dependency sources. A pending intent prevents another AWR writer from changing the same source; external edits are preserved and reported as conflicts. A duplicate request returns its historical receipt without new writes. No-change batches write no business events. The actor is provenance supplied by the host, not authentication. Raw journals are local runtime state.


## Adopt and supersede a reviewed decision

Create a proposed decision with `document change` first. An explicit adoption uses the version 1 batch envelope above with `change.kind: "related"` and `changes`:

```json
[{"operation":"adopt","candidate":{"source_id":"<new source ID>","external_key":"ADR-NEW","source_fingerprint":"sha256:<reviewed new bytes>"},"supersedes":{"source_id":"<old source ID>","external_key":"ADR-OLD","source_fingerprint":"sha256:<reviewed old bytes>"}}]
```

`supersedes: null` adopts a proposed version without replacing an earlier one. Both references bind explicit registered decision IDs and exact source versions. A timestamp or a newer filename never selects an accepted document. The finite writer requires canonical `id` and `status` in YAML front matter of a registered `markdown-directory-v1` document. Ambiguous header declarations and other adapters refuse adoption explicitly. Text, title, unrelated metadata and the old document remain in place. The old document gets `superseded_by`; the new document gets `adoption` with the actor, request key, reason, reviewed candidate version, superseded version and a fingerprint of declared content and body. `decision show` exposes these links; `--full` includes the retained decision and rationale.

A later content or identity change invalidates a recorded approval: an externally modified accepted document projects as `unknown` until that exact version is explicitly reviewed again. Generic document edits cannot carry an approval across changed content. Legacy source-declared accepted decisions without an adoption receipt keep their original source authority and do not become runtime-verified approvals.

## Recoverable related changes

The same related batch may include `operation: "document"` with a registered `source_id`, `source_fingerprint` and `edit` (the fragment/replace document contract), and one `operation: "ledger"` with the same ledger fields as a single-source batch. Each physical source occurs once; edit a candidate's body before reviewing its adoption. New draft creation remains the separate no-clobber operation. The total bound is 100 files/changes and 64 MiB of before/after snapshots.

A single reviewed fingerprint covers all targets. Every target is preflighted before writing; each completed file step is durably recorded. Supersession writes the old decision first, then its replacement. `filesystem_atomic: false` is explicit: an interrupted multi-file write can have some files applied and others pending. `applied_files`, `file_total`, `partial_apply`, and read-only `current_sources` distinguish those states. A nonzero exit can accompany the retained partial receipt on stdout; hosts must inspect it instead of reporting success. `batch recover` checks every actual file against its saved before/after bytes before resuming. It neither repeats already applied writes nor rolls back third-party changes. Local journals and source intents must travel with matched runtime backups.


## Embed and upgrade the native host payload

Version `0.4.0` exposes the host integration capabilities with schema 4. Use the immutable host payload produced by `scripts/release/build_host_bundle.py`; it includes both native executables, exact capability metadata, file/archive SHA256 values, source/version/platform identity and licenses. Invoke the bundled binary by absolute path with an explicit project root. The native runtime does not require Node, Python, Rust or PATH setup. Development-time packaging and upgrade fixtures run separately.

Follow the [matched upgrade and rollback procedure](../release/DISTRIBUTIONS.md#matched-upgrade-and-rollback-checklist). Preserve all runtime history and source authority, validate old-schema migration with IDs/record inventories, retain read-only legacy APP records, and switch to one AWR writer. A rollback restores a matching program/database/configuration/source set while keeping new user files and the superseded runtime snapshot. Old programs must reject newer databases before writing; host signing, full APP integration and other deployment platforms require their own evidence.

### Compact scoped status (version 1)

`awr status --view summary [--work KEY ...] [--goal KEY] [--milestone KEY]`
returns compact JSON under `--json`. The same selectors are available in
`awr_project_status` as `view: "summary"`, `work: ["KEY"]`, `goal`, and `milestone`.
Selectors intersect exact source-declared associations; unknown references are errors.
No selectors means all projected work. The earlier full view remains available
explicitly; selectors on that view are rejected rather than ignored. The current
source defaults to the [action queue](daily-work.md), which distinguishes
continuation, claimable work, waits and actual blockers. Pin `view:"full"` in
integrations requiring the previous default shape.

The summary includes source status counts, current work, the next action, readiness
and blocking codes, all-source freshness, project organization gaps and registered
pending mutation/checkpoint findings. Pending findings are project-wide, even for a
narrow scope. Lists show at most five entries and carry total/omitted counts; text
uses a 240-character public summary. `omissions` identifies excluded work, truncated
organization scans, and details left for `work show`, `source list`, full `status`,
`recovery inspect`, or a specific operation's status command. Filesystem-only and
host-private journals are outside the snapshot's pending inventory and explicitly
listed as not evaluated. Zero registered pending findings is not a claim that an
external host has no uncertain operations.

Counts describe source states; they do not certify acceptance or releases. Full
organization evidence assessment runs only against an explicitly supplied
`source_sha`, as before. Snapshot and cached-currentness metadata retain the query
contract above. A summary reduces transport size, not the source verification scope.

### Explicit project organization metadata

`organization show --source SOURCE_ID --mapping fields.json` reads source annotations.
A mapping is an object from `phase`, `scope`, `focus`, and/or `next_action` to exact
JSON pointers, for example `{"phase":"/current/stage","focus":"/current/work"}`.
Parents must already be YAML mappings. Escaped pointer segments, array ancestors,
overlapping paths, entity collections, lifecycle, release, contract and count fields
are outside this finite writer. Unmapped values and surrounding YAML bytes remain
untouched. Hosts can consume the returned focus explicitly; reading it never starts,
claims, activates, reopens or completes work.

`organization preview --input change.json` is a read-only full preflight. The input
has `version: 1`, `request_key`, `actor: {host, subject, origin}`, `reason`,
`source_id`, `source_fingerprint`, `mapping`, and `values`. Actor origins reuse the
host contract (`human`, `ai_accepted`, `delegated_agent`); they record provenance,
not permission. Values use a phase plan key, a scope array of exact work keys, a
nonterminal focus work key, and next-action text. Null can clear optional metadata.
Unknown references, focus/phase or focus/scope conflicts, stale sources and attempts
to alter projected work, dependencies, status, acceptance or evidence are rejected.
The preview binds every changed field, source/configuration fingerprints and project
revision. No project-specific completion formula or denominator is embedded in AWR.

Apply with `organization change --input change.json --expected-preview HASH
--expected-revision N`. Inspect with `organization status --key KEY`; recover an
interruption with `organization recover --key KEY --expected-revision N` after
reviewing its receipt. Recovery accepts only the recorded before/after source bytes
and original configuration, and never overwrites external edits. The source write
and subsequent projection refresh are separate durable steps. If refresh fails,
the retained receipt identifies the possible saved source; source history records
the refreshed fingerprint. Repeated completed requests return the historical receipt
without applying the metadata again. Private receipts live under `.awr/mutations`.


## Five-layer result semantics and host integration modes (AWR-EVO-010)

This section **freezes** contract semantics for AWR-EVO-010. It does not claim new
runtime commands beyond what existing surfaces already expose. Machine-checkable
companions:

- `.local/awr-evolution-20260919/semantic-contract-matrix.json` (local working copy)
- `tests/fixtures/evolution/AWR-EVO-010/` (checked-in mirror + counterexamples)
- `scripts/evolution/verify_evo_010_semantic_contract.py`

### Five result layers

Layers reuse existing objects. They are **not** five independently advancing state
machines. Unknown never defaults to success. Readable never implies writable.

| Layer id | Meaning | Tip authority (reuse) | Independent counterexample |
| --- | --- | --- | --- |
| `work_readiness` | 工作准备 — structure/deps/claim eligibility | status/ready/prepare diagnostics | `CX-WR-01` |
| `execution_admission` | 执行准入 — session/claim/write authority | claim/session/revision gates; management never grants | `CX-EA-01` |
| `context_completeness` | 上下文完整 — required context under budget | context compile completeness | `CX-CC-01` |
| `delivery_observation` | 投递 — host-attested delivery/ack/report | client progress, execution report, continuity waits | `CX-DO-01` |
| `completion_validity` | 完成有效性 — evidence/acceptance gates | work complete / evidence / ordinary vs verified | `CX-CV-01` |

**Separations that must remain expressible:**

1. Context may be complete while dependencies are incomplete (`CX-SEP-01`).
2. Results may be readable without write/admission rights (`CX-SEP-02`).
3. Entry points must not mix layer semantics (`CX-MIX-01`).

### Two host integration modes

| Mode id | Unique write owner | May create Work/Run/owner? | Independent counterexample |
| --- | --- | --- | --- |
| `runtime_delegated` | `awr_runtime` | AWR projections/sessions/claims/executions via domain actions | `CX-HM-RD-01` |
| `component_only` | `embedding_host` | **No** — must not create host Work/Run/owner or hold a second work state | `CX-HM-CO-01` |

Component mode may read/verify/project and may perform **explicit** host-requested
AWR writes. It must not open a shadow claim lifecycle or treat component checks as
completion.

### Compatibility and negotiation

Keep old outputs. New fields/views require explicit capability or protocol
negotiation. Missing required capabilities fail closed
(`CapabilityUnavailable` / `ProtocolUnsupported`). Silent degrade when a required
capability is missing is forbidden (`CX-COMPAT-01`). Defaulting an unknown layer
status to success is forbidden.

Validate:

```sh
python3 scripts/evolution/verify_evo_010_semantic_contract.py
python3 scripts/check_public_tree.py
```
