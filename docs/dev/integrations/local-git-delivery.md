# Local Git delivery

The optional `awr_server::delivery_adapter::LocalGitAdapter` observes an
operator-configured bare repository using the same `awr-delivery` protocol and
authenticated delivery inbox as other providers. It has no GitHub dependency.
This first adapter boundary is a library entry point; server configuration,
background polling and final task acceptance are separate capabilities. A separate
opt-in library integrator performs authorized fast-forward effects as described
below. There is no new anonymous endpoint or caller-selected repository.

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

Integration is observed as applied only when the source commit is an ancestor
of the observed target commit, every manifest artifact still matches in that
target, and the target reference remains unchanged across the inspection.
Matching trees without ancestry, squash/rebase inclusion, replaced artifacts
and uncertain queries do not prove integration. A missing or not-yet-containing
target remains pending. Observing a manually performed push does not prove that
AWR requested or approved it; `request_id` remains absent.

Capabilities advertise observation and polling, with integration requests,
change requests and notifications disabled. These flags never grant permission.

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
target that includes the original source and exact manifest bytes. A recorded
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
request-bound neutral observation, and confirms that effect. Its read set and
connector version describe **current admission**. Source, selection or ownership
changes preserve the old candidate's historical binding; they cannot promote it
to acceptance of the new work version. The same poll ID recovers the durable
receipt after restart. Confirmation, task acceptance and authoritative source
publication remain separate; this entry point never completes a task.

## Reports, retry and current facts

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
no caller identity, path, approval or read set. Server lifecycle configuration
remains separate. Explicit retries of a published poll still use `reconcile`.

Every poll probes the actual bounded Git state. It compares source verification
and target integration separately with current facts from that connector and
SHA-verified immutable reports. Report timestamps and inspection IDs are excluded
from comparison. Source revisions, measured artifact hashes/lengths, outcomes,
unsupported checks, target contents/ancestry and stability remain significant.
When the preliminary probe matches current proof, it publishes no report,
inspection, fact or notification. Restart
uses the same persisted proof and actual Git probe, without an in-memory cache.

Target-only changes publish only integration observations and preserve the
original source verification run and fact. A missing, corrupt, stale or mismatched
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
human approval or deployment credit. Service lifecycle and end-to-end native
collaboration acceptance remain separate delivery work.
