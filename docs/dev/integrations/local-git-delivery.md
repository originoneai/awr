# Local Git delivery observations

The optional `awr_server::delivery_adapter::LocalGitAdapter` observes an
operator-configured bare repository using the same `awr-delivery` protocol and
authenticated delivery inbox as other providers. It has no GitHub dependency.
This first adapter boundary is a library entry point; server configuration,
background polling, authorized integration and final task acceptance are separate
capabilities. There is no new anonymous endpoint or caller-selected repository.

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

The adapter uses fixed Git subcommands with validated arguments, clears ambient
Git/credential environment values, disables replacement objects, prompts and
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

## Validation boundary

`local_git_delivery` exercises independent bare SHA-1/SHA-256 repositories,
actual fixture pushes, content drift, unavailable objects, unsupported paths,
bounded inspection, report recovery and concurrent retries.
`local_git_delivery_pg` additionally exercises system connector provenance,
current facts, durable replay, revocation and out-of-order generations in an
isolated PostgreSQL fixture. These are mechanism regressions, not native team
business acceptance, human approval, deployment or an authorized merge test.
