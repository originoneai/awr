# Local Git delivery

The optional `awr_server::delivery_adapter::LocalGitAdapter` observes an
operator-configured bare repository using the same `awr-delivery` protocol and
authenticated delivery inbox as other providers. It has no GitHub dependency.
The library entry points also power an optional Team server worker. It observes
current candidates and, when explicitly enabled, processes approved integration
intents. Final task acceptance remains separate. The opt-in integrator performs
authorized fast-forward effects as described below. There is no new anonymous
endpoint or caller-selected repository.

## Configuration and scope

See [the synthetic configuration](../../../examples/local-git-delivery/adapter.toml).
All paths must be absolute and already exist. The repository must be bare; the
private report directory must be outside it. Configure the executable, opaque
resource identity, tenant, project, workstream, work, connector and full target
reference explicitly. Configuration is strict and versioned. No credential
belongs in that file: a service principal supplies its credential privately.

Git 2.46 or newer is required. Startup probes `show-ref --exists` so missing
references remain distinct from damaged references and repository read errors.
An unsupported probe or damaged configured target refuses admission; a later
query error cannot become a successful pending observation.

The verified repository stays the fixed process directory. Git receives
`--git-dir=.` so Windows verbatim canonical paths never need argument rewriting.

The adapter uses fixed Git subcommands with validated arguments, clears ambient
Git/credential environment values, disables replacement objects, grafts, hooks, prompts and
lazy fetches, and refuses promisor/partial-clone repositories. Command time,
total inspection time, stdout/stderr and blob sizes are bounded. Git stderr and
file contents are never returned. Inspection performs no repository mutation,
working-tree checkout, hook, network request or shell command.

## Exact observations

A candidate must match the configured project/work/resource/target and contain
an exact Git SHA-1 or SHA-256 commit. Artifact locators use
`git-blob:relative/path`; traversal, symlinks and Git submodules are unsupported.
Ordinary blob bytes must match every manifest SHA-256 and byte length. Files are
read from the immutable commit, independently of a local working tree.

The `local_git.manifest` verification checks only those bytes. It does not run
unit tests or reinterpret a caller-provided test log as trusted verification.
Other required checks remain explicitly unsupported in the report and absent
from observed verification facts. A missing/unavailable revision, unsupported
artifact or exceeded content budget stays unknown; a measured mismatch fails.

Integration is observed as applied only with a usable complete-content witness,
matching manifest bytes and a stable target reference. The witness distinguishes:

- `exact_revision`: the actual target is the same immutable Git commit as the
  selected source. Missing-target creation retains this ordinary behavior.
- `matching_complete_snapshots`: the actual commits differ, their **whole** Git
  trees match, and the declared exact target base is an ancestor of both commits.
  The source need not be an ancestor of a rewritten result. Fixed bounded reads
  resolve SHA-1/SHA-256 tree objects and check their complete reachable object
  graph, including files and modes outside the manifest. No missing object is
  fetched. The final target read occurs after these queries and artifact reads.
- `unavailable`: no usable proof was obtained. Finite reasons distinguish missing
  observations/history, changed content/base, unstable targets and unsupported
  formats. A matching selected subset or a caller flag cannot establish equality.

This supports read-only recognition of squash/rebase results under the unchanged
declared base. A different base, file omission, unrelated content or mode change,
missing nonmanifest object, or target race cannot confirm the old candidate.
Exact whole-snapshot equality is deliberately conservative: legitimate changes
that produce a different tree need a new candidate, verification and review.
`graph_contains_source` still reports actual ancestry; it is not rewritten to
true when complete trees establish equality.

A missing or not-yet-containing target remains pending; uncertain application
remains unconfirmed. Observing an operator's push or rewrite does not establish
that AWR requested or approved it; observation-only `request_id` remains absent.
The observer performs no squash, rebase, merge, push or reference mutation.

Capabilities advertise observation and polling, with integration requests,
change requests and notifications disabled. These flags never grant permission.

## Optional Team server worker

### Source-first delivery without a pull request

The server can connect independently identified members, an authoritative YAML
ledger and a bare Git repository without loading a GitHub or pull-request adapter.
Publish the physical ledger through the existing source preparation, approval and
activation interfaces **before** execution and business review. Its work contract
must explicitly require the intended checks. For the byte-verification flow:

```yaml
completion_policy: caller_managed_execution_and_agent_review
execution_settlement:
  mode: independent_workspace_v1
  workspace_id: member-owned-workspace
verification_requirements: [local_git.manifest]
```

The executor submits its execution result and exact artifact as evidence. An
observation-only Git worker verifies the selected commit's actual manifest bytes;
it creates no integration intent or business approval. The independent reviewer
reads the bound evidence through `artifact.content` and records the business
decision. Source bootstrap approval and this later artifact review are separate.

The supervisor obtains current contract, connector, selection, evidence and
decision versions through `work.prepare`, `delivery.neutral.inspect` and
`review.inspect`, then issues `delivery.integration.prepare`. If its reply is
lost, query `delivery.neutral.outcome` using the same preparation request key.
That recovers the original intent; it does not request another integration.

With both optional worker groups configured, the Git worker performs the one
authorized fast-forward and re-observes the actual target. The source worker
writes bounded `delivery_sync` references into the exact physical work record,
then confirms its fingerprint and reindex. The original contract, task status,
unrelated work and operator comments remain intact. A source conflict leaves the
operator's bytes untouched and reports synchronization as pending even if the
repository integration was confirmed. After explicit source-conflict resolution
or recovery of a landed write's missing receipt, persisted workers resume the
original operation without repeating Git effects or rewriting unchanged bytes.

`source_synchronized` establishes that these references match the observed source.
It is not task completion. Domain acceptance, any completion receipt and the
source reference to that receipt remain separate steps. This flow does not turn
`local_git.manifest` into a unit-test result or a native Agent business acceptance.

Set `AWR_TEAM_LOCAL_GIT_WORKER_CONFIG` to a separate strict TOML file based on
[the worker example](../../../examples/local-git-delivery/worker.toml), then start
the existing `awr-server serve --config /absolute/path/service.toml` entry point.
The service project key must already map to the repository's tenant/project.
An unset variable or empty worker list reads no credential, repository or PG
state for this group. It creates no principal, grant, connector or task.

Each entry requires an explicit worker ID, a **credential environment-variable
name**, a fixed `LocalGitConfig` under `repository`, and `integration_enabled`.
Provision the credential privately through existing access management. The
principal must be the enabled `local_git` / `adapter_observation` connector's
actual system actor/client and have current read and observation-submit scope.
When integration is enabled it also needs current `delivery.finalize` scope.
A readable schedule alone does not establish either mutation permission.

`integration_enabled = false` observes the current selection; it never prepares
or executes an intent and does not settle original dispatches. With `true`, the
worker additionally traverses the original-intent queue. A supervisor must first
prepare each intent through the existing
[version-bound admission](delivery-integration.md#supervisor-workflow-over-http-or-mcp).
Configuration and successful observation cannot substitute for actual evidence
and independent review. Worker operations are not new public HTTP/MCP commands.

The group supports at most 16 workers with unique IDs and unique
project/work/connector scopes. Separate processes can compete for the same
scope under PG fencing. Each worker polls at 100–60,000 ms with a 5–300 second
dispatch lease, a 100–60,000 ms operation timeout shorter than that lease,
and capped backoff between its poll interval and 300,000 ms. Repository
inspection timeout cannot exceed operation timeout. Pages contain 1–32 intents;
each poll visits at most 1–16 pages and 1–64 intents. Page size shrinks to the
remaining job budget so no returned row is skipped by advancing its cursor.
Terminal history cannot permanently hide the queue tail; retired cursors reset
to a fresh authenticated scan. Missing/stale current selection does not hide
historical dispatched requests on the same configured resource.

The Git group and optional
[source-publication group](delivery-synchronization.md#optional-server-worker-lifecycle)
share one store/pool and an all-or-none admission boundary. Both complete their
current identity/scope and fixed-resource checks before either spawns any task,
within a 30-second overall startup limit. Invalid Git admission cannot start an
otherwise valid source worker. An absent source group leaves source publication
pending; Git confirmation alone never sets `source_synchronized`.

Prepared and expired **undispatched** requests may acquire a fresh fenced lease
and the domain's first sealed dispatch capability. Live leases are left alone.
Dispatched or unknown requests are only queried through their original identity;
they never receive another Git effect. Confirmation/rejection history remains
terminal. Every domain action rechecks current authority, and explicit credential
rotation requires a service restart. Reopening the service keeps persisted
target guards, reports and request identity; unknown never means safe to retry.

Ctrl-C, Unix SIGTERM, HTTP termination, stop and runtime drop cancel both groups.
Joining workers has a five-second ceiling. A cancellation/timeout or missing
reply retains durable uncertainty even if a physical Git effect already landed.
Restart probes the original request and real target before recording its result.
`LocalGitWorkerRuntime::monitor()` returns finite state/failure codes and
saturating counters per configured worker. State-change logs contain configured
project/worker IDs and finite codes only, without raw errors, repository contents,
private paths, credentials or environment-variable names. These observations
are not approval, completion or delivery receipts.

## Opt-in fast-forward integration

`LocalGitIntegrator::open(LocalGitIntegrationConfig { enabled: true,
repository: config })` enables mechanical integration support using the same
explicit `LocalGitConfig`. `LocalGitAdapter` remains read-only. The integrator
advertises integration support; the configured service principal still needs
actual scoped delivery permission and an approved
[durable integration intent](delivery-integration.md). Disabled configuration
refuses to open the integrator.

`execute(store, credential, DispatchDeliveryIntegration)` validates the original
configured candidate and calls the store's live dispatch. Only its first sealed
`DeliveryIntegrationPermit` can launch a command. `execute_permit` is also
available to a trusted in-process worker that already holds that unique permit.
Serialized requests, reports and receipts cannot substitute for it.

Before launch, the integrator verifies the actual bare repository/object format,
literal regular blob bytes, exact target precondition and fast-forward ancestry.
The total byte budget must cover both source and target verification. It then
rechecks the original issuer, worker credential, dispatch lease, review, required
checks and candidate eligibility through the store. A single fixed
`update-ref --no-deref <configured-ref> <exact-source> <expected-old>` uses Git's
compare-and-swap. Missing targets use an all-zero old revision. Hooks, grafts,
replacement objects, lazy fetches and ambient credential settings are disabled;
reference fsync is explicitly enabled. No shell, network push/helper, checkout,
merge, squash, rebase or GitHub operation is part of this adapter.

Before preflight/launch, a complete immutable attempt marker is published and
synced. Only its creator can launch, and the unique permit is consumed. Command
results are recorded separately using finite diagnostics without stderr or
credential values. A dispatch replay, missing response, timeout, cancelled future
or reconstructed store never launches another command for that intent.

`query(store, credential, integration_id, inspection_id)` observes the original
request without obtaining a lease or permit. Application requires a stable actual
target with a usable complete-content witness and exact manifest bytes. A recorded
pre-command rejection can establish no effect. A failed/launched command, missing
result or unchanged target remains **unknown** unless application can be proven;
absence of a change is not permission to retry. Unknown results retain the store's
target guard. Operator-owned repository/report configuration must stay available
for recovery; changed or conflicting markers/configuration fail explicitly.

Integration reports are immutable and addressed by SHA-256 through the integrator's
`report_bytes` method and `awr-local-git-integration:` locator. The same inspection
ID reuses its original report; a new ID requests a new observation. Missing or
corrupt archived reports fail exact replay. A new inspection may recover actual
repository facts without recreating the missing history or repeating the effect.

`reconcile(store, credential, LocalGitIntegrationPollRequest)` reserves a current
system connector inspection for the immutable original dispatch, ingests its
request-bound neutral observation and its independent content-proof record in
the **same** reserved inbox batch, and confirms that effect. The observation is
the first record, preserving the original confirmation fact identity. A changed
Git result can settle only the original request under its original connector
version with matching candidate/result/report provenance. Unavailable proof
keeps the target guard; it never authorizes redispatch. Its read set and
connector version describe **current admission**. Source, selection or ownership
changes preserve the old candidate's historical binding; they cannot promote it
to acceptance of the new work version. The same poll ID recovers the durable
receipt after restart. Confirmation, task acceptance and authoritative source
publication remain separate; this entry point never completes a task.

## Reports, retry and current facts

New reports include an optional typed `content_witness`. Missing legacy fields
remain absent and emit no invented proof record. Existing version-one archives
and their exact bytes remain readable without backfilling or overwriting them;
a fresh inspection is required to observe new proof. Integration reports retain
an explicit unavailable witness even if a bounded repository observation fails.
An archive's presence never establishes current connector authority.

Reports contain bound revision IDs, measured hashes/lengths, finite outcomes and
the actual observation time. Complete files are published immutably, indexed by
inspection ID and addressed by SHA-256. `report_bytes(digest)` reads the exact
bounded report and checks its digest. The `awr-local-git-report:` locator is
resolved through this configured adapter API; it is not a public download URL.
Keep the private spool for as long as any evidence references it.

Retrying the same inspection ID returns the original report. Use a new ID for a
new repository observation. Concurrent retries retain the same complete winner;
a changed candidate or configuration cannot reuse its index. Missing, corrupt
or conflicting reports fail explicitly rather than being silently regenerated.

`reconcile(store, credential, LocalGitPollRequest)` reads the actual selected
candidate, reserves an inspection against current PG authority, observes it,
then ingests a bounded neutral verification/integration batch. The connector
must be explicitly mapped to an authenticated system actor with
`adapter_observation` provenance and the necessary existing scoped permissions.
A caller-declared connector cannot promote these records to trusted observations.

The poll request ID produces stable reserve/ingest/event IDs. After a missing
response, the existing `delivery.neutral.outcome` query can recover the ingest
receipt; repeating the same bound poll also reuses its durable observation.
Newer PG inspection generations supersede late results. Credential revocation,
changed authority, source, candidate, connector or ownership remain subject to
the existing store checks. A report on disk is not proof of successful ingest.

Observed facts, source-publication confirmation, review, execution authority and
task completion remain separate. The existing source publisher consumes the
resulting durable intents when configured; without its confirmation, source
synchronization is pending. No observation establishes independent approval,
terminates an execution or marks a task complete.

## Stable scheduled observations

`reconcile_current(store, credential)` is a service-owned polling entry point.
It derives admission from the authenticated schedule and requires the configured
`local_git` system connector, resource and current selected candidate. It accepts
no caller identity, path, approval or read set. The optional server worker uses
this entry point. Explicit retries of a published poll still use `reconcile`.

Every poll probes the actual bounded Git state. It compares source verification,
target integration and complete-content proof as three independent slots with
current facts from that connector and
SHA-verified immutable reports. Report timestamps and inspection IDs are excluded
from comparison. Source revisions, measured artifact hashes/lengths, outcomes,
unsupported checks, target contents/ancestry and stability remain significant.
When the preliminary probe matches current proof, it publishes no report,
inspection, fact or notification. Restart
uses the same persisted proof and actual Git probe, without an in-memory cache.

Target-only changes publish changed integration/content slots and preserve the
original source verification run and fact. A new unavailable content observation
replaces its old usable proof without inheriting it. Upgrading an otherwise
unchanged legacy batch can publish only the new proof slot. A missing, corrupt, stale or mismatched
proof requires a fresh reserved observation. A truncated snapshot that omits a
needed slot is an explicit bounded-response error, not proof of absence.
Changed observations share deterministic reservation identities based on actual
admission, semantic probe and predecessor facts, then reobserve after reservation
and ingest only changed slots through the existing domain. If an abandoned
observation lease is confirmed expired by PG, one fresh reservation may recover;
this does not renew, repeat or authorize a Git integration effect.

The reservation's `inspection_lease` describes its live status at read time,
outside the immutable original receipt. It cannot substitute for ingest's own
authority and lease checks. If state changes between the preliminary and reserved
observations, the result can be `unchanged` with `read_only: false`: a reservation
and report exist, but no new fact was published.

The result identifies `changed_slots` and an optional real ingest receipt. An
`unchanged` result is an observation at read time; it grants no acceptance,
execution authority or source synchronization. Credential/mapping revocation
and selection changes remain checked on every poll and domain write.

`LocalGitIntegrator::reconcile_original_current(store, credential, integration_id)`
separately queries the authenticated original dispatch. Its confirmation snapshot
contains the exact fact envelope, observation receipt and inspection/connector
binding. The worker verifies immutable report bytes and the complete original
record and every original content-proof envelope, accounting for the
server-owned recording time separately from Git's
observation time. It then compares a fresh bounded Git probe, excluding only
report timestamps and inspection IDs. Attempt/result state, candidate, measured
contents, complete-content witness and integration outcome remain significant.
Missing or mismatched original proof cannot become an unchanged equality result.

An unchanged unknown query publishes no inspection, fact or confirmation, even
after reconstruction. Missing/corrupt proof requires a fresh reserved observation
without overwriting its archive. If PG confirms an abandoned observation lease
expired, at most one fresh observation reservation is attempted per call. This
never renews an execution permit. A superseded fact can settle only its original
historical intent and cannot accept a current replacement candidate.

## Validation boundary

`local_git_delivery` exercises independent bare SHA-1/SHA-256 repositories,
actual fixture pushes, content drift, unavailable objects, unsupported paths,
bounded inspection, report recovery and concurrent retries.
`local_git_delivery_pg` additionally exercises system connector provenance,
current facts, durable replay, revocation and out-of-order generations in an
isolated PostgreSQL fixture. Scheduled polling also covers unchanged/restarted
observers, target application/rollback with preserved source verification,
concurrent publication, damaged proof, expired abandoned observations and live
revocation/stale selection. These are mechanism regressions, not native team
business acceptance, human approval, deployment or an authorized merge test.

`local_git_integration` checks opt-in configuration and the unchanged read-only
observer. `local_git_integration_pg` uses actual independent Git repositories and
real PG execution/evidence/review records to exercise CAS, both object formats,
conflicts, revoked authority, historical recovery, cancellation, timeout and
durable replay. These are isolated mechanism tests, with no native business,
human approval or deployment credit. Confirmation snapshot and reservation replay
checks preserve exact historical data while reporting current lease liveness.

`local_git_worker` covers opt-in/empty configuration, observation-only behavior,
normal integration, competition, small job budgets, revoked/rotated credentials,
missing selections, cancellation/restart, stable unknown queries, damaged proof
and abandoned observation recovery. `delivery_worker` covers combined admission
failure without source effects. Physical source-first closure and end-to-end
native collaboration acceptance remain separate delivery work.
