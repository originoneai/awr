# Workstream contract

This document specifies the workstream extension being implemented. It is not a
release announcement. A capability is available only when the running service
advertises it and enforces it at the relevant operation boundary. A source field
or a database column alone does not establish support.

The current domain module validates identity, ownership, scope selection and
explicit grants. Source import and SQLite projections retain the scope contract.
Session attribution, work-wide claims and reviewed ownership movement are wired
through the personal runtime. Explicit scoped read snapshots and registered-file
read APIs are available to trusted Rust integrations. Local L0/L1 context,
required-fact completeness and context delta use scoped facts when sources
explicitly enable workstreams. The shared personal MCP service has an explicit
authenticated read boundary; unsupported shared operations are rejected for
enabled workstreams. Team PostgreSQL has authenticated HTTP/MCP queries,
session journaling, claims, execution admission and authorized recovery, with a
bounded local reference runner. Versioned cross-stream hard dependencies and adoption credentials are available
in core/store/team-pg (WS-030). File/dir/workspace versus shared external/integration resource bounds are enforced
at reservation and admission; physical strong isolation still requires a verified
host sandbox or OS boundary, not AWR metadata alone.
These paths do not establish complete isolation for every CLI, MCP or Team operation.

The Team source coordinator also accepts an explicit multi-work source bundle
and commits its catalog, ownership, contracts and required graph together. This
operator API establishes source identity, not authenticated Team access or
versioned delivery adoption. See the development boundary below before enabling
it; legacy execution entrypoints refuse enabled projects.

## Identity and authority

A project has one authoritative work graph and one or more workstreams. Each
work belongs to exactly one workstream. A goal explains intent, a milestone
groups stages, a work branch retains an execution alternative, and a Git
worktree isolates an editing directory; none substitutes for workstream identity.

Workstreams have immutable IDs, stable project-local keys, display titles,
explicit goal and acceptance-contract references, and lifecycle state. Renaming
a title does not change identity. Moving work requires an explicit transition
that accounts for claims, executions, dependencies and historical attribution.
Shared work is owned once and referenced by its consumers.

Source-first projects retain authoritative sources. Team source snapshots define
approved contracts while the coordinator owns execution facts. The extension
does not introduce a second ledger or require a server for personal use.

## Selection and authorization

Resolve scope from the exact work or persistent session. Explicit selectors
must agree with that binding. Without a bound work/session, a unique authorized
scope may be selected; multiple scopes require an explicit selection. Never
choose another client's most recently active scope. Conversation defaults are
client-scoped and cannot change existing session ownership.

All reads and writes enforce the same authorization, including search, counts,
event feeds, artifacts, recovery and caches. Dependency exports may expose an
authorized minimal contract and receipt without granting access to the
provider's private history. Query filtering is not an authorization boundary.

The default context contains the selected work, its relevant goals, session
checkpoint, applicable hard rules and required dependency proofs. Unrelated
active workstreams do not contribute their histories. Project-wide hard rules
and administrative barriers still apply.

## Concurrent operations

Ordinary writes validate their actual read set: authority, work and contract
versions, applicable policy, claim/fence, dependency bindings and resource
generations. Unrelated events can advance the project audit cursor without
invalidating an unchanged work. Repeated request IDs with the same intent return
the original receipt; changed intent conflicts. Unknown effects are inspected
before another dispatch.

Same-work ownership and overlapping resources remain exclusive. Source writers
use reviewed patches, source fingerprints and recovery journals
(`awr-source` / `awr-runtime` source-concurrency helpers). Stale whole-file
installs that no longer match the reviewed fingerprint are refused. Supported
sharded multi-source updates form a coherent candidate and activate under a
recovery journal; unsupported adapters reject the operation. External edits and
half-writes remain recoverable without overwriting foreign bytes. SQLite remains
a single-writer database.

Project freeze, permission revocation and restore remain barriers. A restore
changes the coordinator epoch. Lease expiry does not prove process termination;
unknown executions retain resource protection. Worktrees separate local edits
but do not isolate a shared database, deployment destination or integration ref.
File confidentiality against an untrusted process requires a verified host
sandbox or OS boundary, not just AWR metadata.

## Cross-workstream dependencies

Edges connect specific works and required deliveries, not whole workstreams.
For example, `interface` in one stream may precede `sdk` in a second stream,
which precedes `integration` back in the first stream. The stream-level arrows
may return to the first stream while the task graph remains acyclic.

Required dependencies bind work identity, contract, artifact digest, acceptance
receipt, source/environment, adoption policy and export authorization. A source
status of completed is not verified delivery. Fixed accepted versions survive
unrelated upstream updates; current-contract dependencies revalidate when their
actual contract or delivery changes. Revocation is distinct from a newer version.

Check the complete required graph atomically, including concurrent graph edits.
Recheck relevant dependency proofs at preparation, dispatch and completion.
Only affected consumers need reevaluation; preserve historical receipts and
already observed external effects. References do not implicitly become hard
dependencies. Cross-project or cross-organization dependencies are outside this
version of the contract.

The Team source bundle can already represent and validate the acyclic
`interface → sdk → integration` example across workstream ownership. This is
source graph validation only. Delivery receipts and adoption credentials are persisted (WS-030). The runtime
and Team graph APIs enforce acyclic cross-stream task DAGs with explainable hard
cycle paths, atomic concurrent edge mutations, all-necessary-deps readiness, and
shared outcome references (WS-031). Selective invalidation and prepare/dispatch/
complete boundary revalidation are enforced with WS-030 adoption policies and
scoped planning changes for newly discovered dependencies (WS-032).

## Accounting and compatibility

Progress uses an explicit versioned required-work set, not the number of tasks
returned by a goal filter. Planning, implementation, acceptance, merge and
release are separate facts. Shared work is counted once.

WS-040 exposes this as a runtime façade over the pure core contract accountant:
`account_approved_scope` freezes the denominator from an attested versioned
contract, keeps the five delivery stages independent, and labels Goal query
hits as `is_not_contract_completion_rate`. Unique ownership, shared-outcome
references and ownership transfers refuse double-counting or silent history
rewrites; a transfer requires a new contract revision and digest while the
historical ledger remains readable under the prior identity.

Usage receipts identify the execution, work, owning stream at the time, provider,
model, pricing basis and observation coverage. Compression and request billing
must not be charged twice. Unknown usage is not zero. Shared costs require an
explicit allocation rule; allocations sum to the original charge. Parallel
wall time is distinct from summed execution time.

WS-041 persists and queries these as real usage + time accounting: deduped
receipts bind to execution / task / occurrence-time mainline; actual cost,
API-equivalent estimate, unknown, and coverage stay separate columns;
cumulative provider counters convert to incremental deltas; corrections and
cross-stream allocations are append-only and auditable. An allocation conserves
the corrected cost, and a second allocation of the same receipt is refused.
Replaying a request key with a different body conflicts instead of keeping the
first body under a success result. Measured time/usage
with coverage is handed to WS-043 as historical observation only —
cumulative-duration fields must never be presented as estimated remaining time.

WS-043 builds calibrated expected acceptance time and stage checkpoints on top
of that handoff: append-only forecast records (target, generated_at, task-graph
version, strategy, sample/method versions, intervals, assumptions, unknowns);
critical-path scheduling under real concurrency and available executors without
summing parallel durations or waiting on unrelated mainlines; separate
effective-execution, dependency/human-wait, and calendar-window components with
evidence-backed queue exclusions; cold-start provisional/unestimable labels and
calibrated intervals only after frozen sample+holdout gates (coverage/width/
error/missing-rate reported; LLM narrative is never a precise promise); and
reestimates on dependency/rework/executor/capacity changes that preserve
before/after reasons while isolating historical samples from acceptance data.

Legacy projects retain one compatible default scope and existing identities.
Existing project, work and session identifiers remain opaque strings, including
Team identifiers that are not ULIDs. Tenant adapters must obtain catalog and
permission records within the authenticated tenant before applying domain rules.
Legacy project-revision checks remain available. A client that cannot represent
scope must not silently write across newly isolated streams. Schema upgrades,
scope enablement and source migration require explicit, recoverable transitions.
Old dependency and ownership checks remain until replacement protocols pass
compatibility checks.

Navigation nodes (`awr nav`, MCP `awr_workstream` with `action: "nav"`) and `work graph`
nodes carry `last_event_at`: the newest event time of that work in milliseconds over every
branch and session, or null when the work has no event. It dates activity without paging
through the event history; it is neither a progress nor a completion signal, and it does
not enter the graph fingerprint.

## Verification boundary

### Team source projection in the development branch

PostgreSQL schema 10 retains `scope_id='main'` as the existing execution branch
dimension. Workstream identity is separate. Upgrading schema creates a disabled
mode for existing projects and leaves historical sessions/events unattributed;
it does not enable workstreams or infer historical ownership.

The trusted `SourceStore` accepts one `workstreams.json` with codec
`awr-team-workstreams-v1`, containing a complete catalog and an array of
`{workstream_id, contract}` entries. Nested contracts keep their closed V1 codec
and hashes. `contract.json` and `workstreams.json` cannot coexist. The source
package keeps its existing file-count and byte limits. Required dependencies in
this bundle refer to exact work IDs in the same complete bundle; missing works,
duplicate dependencies and cycles are rejected. Nonempty `graph.json` remains
unsupported and is never silently dropped.

The reviewed candidate must be activated explicitly with
`SourceStore::activate_workstreams`. Its result separates `projection_hash` from
the per-work `contract_hashes`. The legacy `activate` API retains the meaning of
its singular `contract_hash` and refuses the new codec. Full manifest bytes,
candidate digest, parser version, source epoch and reviewer are checked again
before activation. Catalog, contracts, ownership, edges, mode and source pointer
commit atomically. Rollback preserves the prior projection.

This stage permits enablement only without existing session history or live
claims/nonterminal executions. Bounded owner-only session/inactive-claim/event attribution is available via
the schema-owner history-migration CLI; reviewed ownership movement remains
unimplemented for Team. Bounded CHECK-safe execution attribution is available via
the schema-owner execution-attribution CLI. Updates retain scope IDs, keys and
ownership; authority changes require increased versions, and retained scopes
must be archived instead of removed. Existing work keys cannot change through
the new codec. A source update also refuses live claims and nonterminal
executions until a narrower activation protocol is available. Downgrade to a
legacy source cannot silently disable isolation.

Legacy Team query, execution and import/restore entrypoints take a shared
admission lock before reading or writing project state. Scope activation takes that separate row exclusively
before the project lock. An already admitted legacy operation finishes before
enablement is rechecked; later unscoped entrypoints return unsupported. Ordinary
legacy source activation still permits repeatable-read snapshots and concurrent
progress. Historical data is never reassigned just to unblock enablement.

`SourceStore` remains a trusted coordinator API, not a client authorization
boundary. Source bundles cannot carry grants and activation grants no reader or
writer permissions. The [Team HTTP service](team-workstream-service.md)
checks live credentials, actor/membership and grants transactionally through a
shared command-domain authorization gate (admission write grant, then
active-stream or attest/reconcile effect checks after idempotent replay). Session
creation, checkpoints, closure, claims, execution intents/admission, result
reporting and authorized reconciliation are supported through HTTP and MCP.
Capabilities advertise `scope_id=main` historical semantics, refuse unsupported
operations, and state that local file access is not a server ACL. The
[reference runner](team-reference-runner.md) performs bounded local file
writes with explicit executor authority. Old-epoch reconciliation requires an
explicit operator review and preserves original attribution. Owner-only read-only recovery inspection is available via
`awr-server access recovery-inspect` for enabled projects. Explicit unattributed
history migration (`history-preview` / `history-apply`) can attribute sessions,
inactive claims and work-bound events from current ownership; it refuses
executions and active claims and does not forge identity or completion receipts.
Owner-only enabled-project logical backup manifests, verified fencing restore,
and a bounded rebuild-from-manifest slice (missing work_items id+external_key plus
ownership when empty/fencing-quiet) are available via `awr-server access backup-*`
(physical basebackup remains external; completion receipts are never rewritten;
divergent ownership overwrite is refused). Owner-only active-claim release/quarantine/attribute-and-release and unattributed
nonterminal execution quarantine-cancel are available via `awr-server access quarantine-*`
(never forges `executor_client_id`). Owner-only explicit execution attribution with a
reviewed `executor_client_id` (CHECK-safe: session+claim present; must match session
client) is available via `awr-server access execution-attribution-*`. Remaining gaps
include full logical rebuild (catalogs, contracts, snapshot ownership, receipts,
grants), attribution of executions lacking session/claim (would require inventing
CHECK fields), and real-client acceptance. Legacy import/restore APIs continue to refuse
enabled projects. The shared personal MCP read boundary described elsewhere does not
provide Team access.

### Source projection in the development branch

`yaml-workstream-ledger-v1` is an explicit, read-only source adapter for one
complete primary ledger. The synthetic [ledger fixture](../../../tests/fixtures/workstreams/ledger.yaml)
shows its `workstreams.version`, strict `definitions`, and one `workstream` key
per work. Definitions retain stable IDs, goal references and acceptance-contract
references. Contract references are identifiers, not evidence of acceptance.
Unknown versions, duplicate scopes, missing ownership and unresolved goal
references reject the candidate. Existing ledger adapters cannot silently consume
this declaration. Multi-source ownership activation is not supported yet.

The source fingerprint, catalog and complete ownership set commit together with
the ordinary ledger projection. Failed imports retain the previous projection
as stale; stale or retired authority cannot be read as current scope data.
Titles can change without changing scope identity. Authority changes require a
new authority version; established ownership changes require a separate runtime
migration. Retained scopes must be archived rather than removed from the source.

SQLite schema 5 gives legacy works a stable single-scope mapping without
renumbering works or changing sessions, claims, checkpoints, evidence or events.
The existing private-memory migration preview does not update the original
database. Applying the schema migration is transactional; a failed migration
can be retried after correcting its cause. Older binaries refuse a schema they
cannot represent. Source projection support alone does not claim runtime
isolation, dependency enforcement or a native-client acceptance result.

The synthetic acceptance matrix is
[`acceptance-matrix.json`](../../../tests/fixtures/workstreams/acceptance-matrix.json).
It defines required counterexamples, not executed results. Domain tests,
SQLite/PG integration tests, native-client checks and complete business
acceptance remain distinct. Report actual evidence and missing coverage rather
than inferring availability or performance from this specification.

## Session attribution and reviewed movement

Schema 6 captures workstream and ownership revision when each session starts.
Claims and checkpoints retain that immutable session attribution. Starting a
work-bound session derives its scope from the work; a conflicting explicit scope
is rejected. Workless sessions need an explicit scope or a client/conversation
default when several scopes exist. Changing that default never rebinds existing
sessions, and a resumed session inherits its predecessor's scope.

Execution ownership is exclusive per work across work branches. Different works
can retain independent claims. Handoff and resume validate the current scope and
ownership generation before transferring execution rights. Paused or unavailable
authority still permits recovery checkpoints, release, session closure and an
unassigned handoff; it does not permit a new execution transfer.

The trusted Store API `commit_source_projection_with_moves` imports a reviewed
source candidate together with an exact move set. `workstream_ownership` provides
the current binding and revision for that review. The caller first invalidates
the edited source and supplies its current source/project revisions. An ordinary
reindex cannot silently move established work. Each move checks the previous
scope and ownership revision, requires an active destination, and rejects active
or otherwise unresolved sessions, effective claims, unfinished checkpoint saves
and nonterminal execution records. An external success report alone does not
establish a supervised terminal outcome. Candidate ownership, source fingerprint
and the move receipt commit atomically; failed imports retain the previous
projection marked stale.

Movement leaves historical sessions, checkpoints and claims in their original
scope. New sessions capture the new ownership generation; automatic recovery does
not import checkpoints from a previous generation, and old sessions cannot resume
execution after movement. This storage API does not itself edit files, authenticate
clients or implement cross-scope delivery adoption. Transport navigation and scoped
permission enforcement remain separate implementation stages.

Schema migration refuses simultaneous effective legacy claims for the same work
across branches. Resolve them through the previous runtime before retrying; the
migration never chooses a winner or deletes claims. An ambiguous historical
workless session retains an unknown scope instead of guessing. Preview and failed
migration leave the original schema and records unchanged.

## Scoped read snapshots

`Store::read_workstream` accepts current trusted `WorkstreamAccess` plus work,
session, explicit scope or conversation selectors. It resolves those selectors
against a coherent private-memory snapshot. Multiple authorized scopes without a
selector are rejected. A selector never grants permission. The returned
`WorkstreamRead` has no raw Store handle or mutation methods.

Catalog counts and pages, event feeds, artifact/evidence metadata, checkpoint
recovery and search use one derived visibility set. Filtering happens before
limits and counts. Search builds its FTS corpus from visible objects only, so
another scope's documents cannot affect BM25 scores or truncation. Scope-bound
cursors include the project, subject and authority version; they do not confer
access to an otherwise invisible object. The original database and its search
cache remain unchanged.

Goals use explicit workstream goal references. Plans and decisions need explicit
local or shared references; mixed or unknown private scope is withheld. Existing
rule scope expresses applicability, not confidentiality: the project rule source
remains shared policy, including hard and unknown rules. Whole-source catalogs
require the project administration interface. A dependency edge alone does not
authorize disclosure of the other workstream's private objects.

Session events keep immutable attribution. Work-only events use ownership at
their recorded revision. Runtime evidence follows its creation event; artifacts
and late artifact registrations follow their originating event. Moving work does
not move historical events, checkpoints, evidence or artifacts into the new
reader's scope. Recovery candidates are filtered before selecting the newest two.

`Runtime::read_artifact_in_workstream` and
`Runtime::read_evidence_report_in_workstream` authorize metadata before filesystem
access, retain path/size/digest checks, and recheck the binding before returning
content. Reading a report does not promote its evidence level.

The typed context readers use that same visibility set for goals, rules, current
work, accepted decisions and evidence associations. `dependency_closure` walks
only visible active tasks. A missing or inaccessible target produces an opaque
`unavailable_dependencies` entry containing the declaring task and an edge
reference; it reveals neither the target's identity/status nor its descendants.
Optional references are excluded when requesting required dependencies. Visible
cycles remain explicit, and an unavailable required dependency makes required
context incomplete. `awr_context::related_work_in_workstream` assembles these
scoped facts without access to the underlying Store. This is not cross-stream
versioned delivery adoption or execution admission.

Current-context session, checkpoint, execution and runtime-evidence readers also
check ownership history. Moving a task away and back does not make earlier
ownership generations current again. Historical receipts remain readable in
their original scope; ordinary work progress does not change ownership. Active
session selection applies scope before testing ambiguity. Legacy single-scope
dependency and related-fact responses keep their previous shape.

These are frozen read APIs, not reusable execution grants. Integrations must load
current authenticated policy for each request and revalidate authority at action
boundaries. Authenticated CLI/HTTP/MCP/Team integration remains a separate stage.
This implementation does not establish complete workstream isolation or business
acceptance.

## Scoped context and semantic identity

The default compiler detects explicit source enablement; a legacy catalog with
one compatible scope keeps its existing behavior and serialized shape. In an
enabled project, work and session selectors determine the scope. Ambiguous
selection is rejected. Without an explicit session/branch, context uses `main`;
another workstream's global branch selection cannot redirect it. An explicit
session supplies its own branch. Branch reads require visibility in that scope.

L1 includes the selected work, workstream-referenced goals, shared applicable
hard rules, visible required dependencies and scoped decisions/evidence. Missing
or inaccessible required dependencies remain opaque gaps, even when their
source status says completed. Unknown hard-rule text remains required. Inactive
workstreams are incomplete for continued execution. Required facts are never
truncated to meet a budget; insufficient budgets return `BudgetExceeded`.

L0 bootstrap uses the same work/session and ownership boundary and retains
unknown hard obligations. It remains orientation, and always requires L1 before
execution. Standalone completeness uses a coherent refreshed snapshot and the
same dependency visibility. An unprovable source refresh fails closed with a
generic diagnostic instead of exposing another stream's source errors.

Context delta narrows source changes to the selected facts and required
dependency declarations before folding, counting or limiting. Removed dependency
edges remain opaque change references. Work and dependency changes from earlier
ownership generations are excluded. Runtime counts and important events use
the selected work, exact branch and current ownership generation. Unattributed
project events and other work's events cannot crowd out the selected work.

L1's `awr.workstream_chunks.v1` policy adds `workstream_identity` with scope,
authority version, ownership revision and reader binding. L0 uses the separate
`awr.workstream_bootstrap.v1` hash domain and exposes the same identity. Rendered
text and semantic hashes exclude global project/source audit cursors and
whole-file fingerprints. Selected fact revisions, required policy, source
identity/configuration, reader, scope and ownership remain bound. An unrelated
task or goal edit in the same source file can advance audit metadata without
changing the selected work's rendered text or hash.

The complete JSON audit envelope intentionally still contains observed source
versions/fingerprints and project revision. It is not byte-stable across unrelated
updates, and semantic hashes do not replace mutation revision checks. Cached text
is never permission to execute: current authorization and action preconditions
must still be checked. Existing MCP reads retain their explicit reindex
requirement when source files differ from the stored projection.





## Named agent host adapters and subtask parallelism (WS-024)

Execution adapters negotiate capabilities (`start`, `status_read`,
`stop_confirmation`, `reconnect_resume`, `result_forensics`) separately from AWR
admission, cancel and session end. L0 manual/`ExternalExecutionReport` remains
first-class. Built-in named clients: `codex_cli` (auto-startable) and
`claude_code` (not auto-startable). Unsupported capabilities return a human
continuation path.

Independent child tasks under a parent keep task identity, own claims and
resource bounds; dependencies order starts while independent work may run in
parallel under an explicit concurrency cap and user pause. Parent rollup
references child outcomes without copying artifacts. Parent session exit does
not auto-complete or release unknown children. Reconnect/retry queries the
original execution before any new start.

See [named agent host](../integrations/named-agent-host.md) and
`tests/fixtures/workstreams/named-agent-host/awr-workstream-isolation-v1.json`.

## Team publish preparation (AWR-TMCP-020)

First-round Team publish preparation maps a server-held YAML workstream ledger
and its referenced Markdown/JSON acceptance specs into the existing
`workstreams.json` contract candidate. The mapping lives in `awr-source`
(`publish_prep`) and the Team coordinator consumes the resulting package through
the existing ingest → approve → activate path in `awr-team-pg`.

### Supported inputs and hard rejects

- Supported ledger adapter: `yaml-workstream-ledger-v1` (`.yaml` / `.yml` only).
- Referenced specs: Markdown (`.md` / `.markdown`) and JSON (`.json`) only.
- Required work fields include stable id, title, workstream key, and a non-empty
  `acceptance` list. Missing fields hard-reject; unsupported formats hard-reject.
- Top-level ledger collections such as `roles`, `members`, `grants`, or
  `permissions` are rejected so publish preparation cannot invent Team role or
  membership relationships from source text.

### Sole source location

The Team project binds exactly one authoritative source location:

- a server-controlled directory (absolute path), or
- a private management repository URL (`git://`, `https://`, or `ssh://`) with a
  pinned revision.

The binding is recorded as `source_binding.json` inside the publish package and
stored with the candidate snapshot. Developers consume the activated Team
contract through the service; they do not need author-laptop files or write
access to the ledger directory. Existing work identities (`work_id` /
`external_key`) and original source versions are preserved across preview and
ingest.

### Preview before ingest

`prepare_publish_from_server_directory` (and the ledger-bytes variant) return a
preview of identity, dependency, acceptance, and source diffs against an
optional previously activated baseline. First publish passes no baseline and
lists every work identity as added. Callers must review the preview before
ingest.

### Ingest / approve / activate boundaries

- First publish uses the existing coordinator semantics: ingest creates a
  candidate, approve records an independent review of the candidate digest, and
  activate installs the immutable source + contract + graph digests.
- Candidate and activated states remain separate. Approving a source candidate
  does **not** grant project membership or member action permissions.
- Source status strings, historical human `done`, and old test materials keep
  source meaning only. Projection install never forges PG completion receipts
  from those fields. Migrations that already carry Team history continue to use
  the existing reject and recovery boundaries; history is not discarded for
  trials.

Fixture coverage lives under `tests/fixtures/team-mcp/publish-prep/`.

The synthetic [context fixture](../../../tests/fixtures/workstreams/context.yaml)
and [manifest](../../../tests/fixtures/workstreams/context.toml) exercise the native
CLI and MCP stdio compilation paths. These are protocol/fixture checks, not
complete business acceptance or authenticated multi-client isolation.

## Parallel / handoff business acceptance (WS-051)

See [workstream-parallel-handoff-biz.md](workstream-parallel-handoff-biz.md).
