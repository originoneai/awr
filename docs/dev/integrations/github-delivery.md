# Optional GitHub delivery observations

This adapter translates bounded, explicitly mapped GitHub HTTPS queries into the
same `awr-delivery` facts used by the [local Git adapter](local-git-delivery.md).
The project, candidate, source contract, authenticated connector and AWR review
remain authoritative. GitHub is optional; an external approval cannot become
an AWR business approval.

## Configure and observe

[`adapter.toml`](../../../examples/github-delivery/adapter.toml) maps one tenant,
project, workstream and work item to an opaque resource, numeric repository ID,
owner/name, API origin and target branch. An optional same-repository PR number
adds a change-request observation. Fork PRs and SHA-256 repositories are not
supported by this first adapter. Omit `pull_number` for a branch-only workflow.

Map each required AWR check to its exact GitHub check name **and numeric producer
App ID**. A successful check with the same name from another application does not
satisfy that mapping. At most 28 producer mappings fit the bounded inbox batch.
Unsupported required checks remain explicit gaps. Legacy commit statuses are
not treated as supported check runs.

The trusted operator loads `GitHubAdapter::open(config, credential)` with an
optional, separately supplied zeroizing credential. The config and reports do
not contain that credential. Opening the library adapter validates configuration
without reading GitHub. `from_transport` is a trusted library extension used by
synthetic tests, not an HTTP/MCP configuration or caller input.

Register a scoped `github` connector using the existing delivery configuration
flow and its actual observer actor/client identity. `reconcile` checks current
selection, source read set, connector version and resource before reserving an
authenticated inspection. It submits observations to the durable inbox; it does
not bypass source publication, grant approval, execute a merge or finalize work.
Keep the request ID and read set when recovering a missing receipt. Reusing the
same inspection returns its original digest-bound report after restart; changed
bindings or corrupt proof are rejected. A fresh request obtains a new observation.

## Stable current-fact polling

Service-owned polling can call `GitHubAdapter::reconcile_current(store, credential)`
without choosing an inspection or carrying cached authority. Each call obtains
the actual authenticated schedule, current candidate, source read set and enabled
connector version before a fresh provider query.

The source manifest, each mapped required check, PR state and target inclusion
are compared independently. Equal outcome alone is insufficient: a new check-run
ID changes that verification. A target or PR change preserves unrelated manifest
and check fact IDs, reports and verification references. Repeated stable polls,
including after reconstruction, publish no observation, notification or new
inspection; unchanged approval references remain usable. If verification
references change, an already prepared integration must revalidate its approval
and eligibility before executing. Full rework and final-acceptance policy are
separate from this observation mechanism.

Equality requires a current, applied, mapped receipt with the actual fact ID,
candidate, selection and source binding. The adapter reads the immutable report
through its SHA-256, verifies the inspection index, reconstructs its records and
compares every exposed summary field. Only the store's recording timestamp is
normalized. Missing, corrupt or inconsistent proof requires new proof; it never
means unchanged. A truncated inventory omitting an expected slot refuses before
the provider query because absence cannot be established.

The preliminary query grants no authority and publishes nothing. Changed slots
use a deterministic request bound to the current mapping, read set, predecessor
facts and complete observation semantics, then reserve and reobserve before
ingestion. Concurrent observers converge on the same durable observation. Only
a store-proven expired observation lease may renew, with a bounded retry; that
renewal cannot dispatch a repository effect. Stable queries recheck current
admission after the provider read, including credential revocation. Results are
observations at read time, without execution, acceptance or source-writeback
authority.

## What the report proves

- Exact immutable source commit; regular tree/blob paths from `git-blob:` artifact
  locators; hashes and byte lengths of actual decoded bytes against the manifest.
  Symlinks, submodules, oversized blobs and unsupported locators cannot pass.
- Complete, bounded check-run pagination on that commit, with unique run IDs,
  producer binding and a fresh reobservation of each matched run. Missing,
  conflicting, skipped, neutral or unfamiliar results are unknown. Changed runs
  cannot carry a passing observation from the preliminary query.
- Pinned target revision and ancestry, plus matching bytes in the target tree.
  Merely seeing `merged: true` on a PR does not prove integration. A target or PR
  that changes during inspection yields uncertainty; repository identity and
  PR/target observations are checked again before publishing the report.

Finite immutable reports contain mapped identities, revisions, digests, outcomes
and observation time. Provider response bodies, check output, commit messages,
repository contents, credentials and provider error messages are excluded.
`report_bytes` resolves proof by SHA-256 and rejects altered bytes. Facts express
what was observed at that time; later polling and source publication remain
separate operations.

## Transport and limits

The production transport uses verified HTTPS, the pinned API origin (including
the optional Enterprise `/api/v3` path), the `2022-11-28` REST API version and GET
only. It disables redirects and environment proxies. URLs and pagination are
constructed locally; response `Link`, blob download URLs and PR URLs are never
followed. Credentials cannot redirect to another host.

Request and inspection deadlines, request count, response and cumulative response
bytes, blob and total blob bytes, and check count are bounded. Blocking HTTPS
runs outside async service threads. A dropped inspection future cannot publish
a late report; its read-only blocking job retains its concurrency permit until
its bounded work exits. Provider/authentication/rate-limit/transport failures do
not manufacture passing or rejected terminal facts.

## Current delivery boundary

The library adapter and optional Team service worker use explicit authenticated
reconciliation. Signed wake-up notifications and synchronization timing
measurements remain separate work. No webhook body is an authoritative fact.

The observer advertises `integration_requests: false`. Repository effects require
the separate opt-in integrator described below; observations never grant authority.

Synthetic API conformance, controlled real HTTPS transport, isolated PostgreSQL
mechanism regressions and complete natural-client business acceptance are
different verification scopes. Passing the former does not imply the latter.

## Guarded integration and original-result recovery

[`integration.toml`](../../../examples/github-delivery/integration.toml) wraps the
same explicit mapping with `enabled = true`. `GitHubIntegrator::open` holds a
separately supplied zeroizing credential. Opening it performs no network request
and grants no AWR approval or execution authority. A configured Git endpoint is
derived from `github.com` for the public API or the same Enterprise HTTPS origin.
There is no caller-selected URL, checkout, Git subprocess or credential helper.

The first supported operation is an **existing SHA-1 target with an exact old
revision, updated by fast-forward to an existing source commit**. Fresh API reads
must prove pinned repository identity, push permission, an unarchived repository,
the exact unprotected branch, a successful empty effective-rules response, source
ancestry, manifest bytes and all required checks. Missing/unsupported policy,
protected targets, missing targets, squash/rebase and unsupported formats refuse
before the effect. Operator credentials should be limited to this repository.
There is no deliberate administrator or protection bypass.

GitHub REST PR merge `sha` guards only the PR head. Git reference PATCH has no
expected-old target precondition. The integrator instead uses HTTPS Git smart
`receive-pack`, requiring advertised `atomic` and `report-status`, and sending the
exact old revision, approved new revision and named target. The provider validates
the old ID and applies its live authorization/policy. The standard empty pack
contains no uploaded source objects; the source already exists in that repository.
TLS verification, redirect/proxy refusal, time and byte bounds also apply to GET
advertisement and the single POST. Error bodies are never read into a receipt.

`execute` obtains the noncloneable neutral integration permit from the actual
authenticated store. Before any possibly executable preparation it durably saves
the original request/candidate/read-set/connector/eligibility marker. It rechecks
current AWR source, candidate, evidence, checks, review and authority before the
single POST. An HTTP success or `ok` acknowledgement is **not** delivery proof.
Fresh reads must prove original source inclusion and exact target artifact bytes.

A lost permit response, timeout, cancellation, malformed acknowledgement or
restart cannot issue another POST. `query` observes the original request using
current authenticated admission, without replacing it with a newer candidate or
requiring that a newer PR head/check still describe the old operation. `reconcile`
uses the existing neutral inspection/inbox/confirmation path for that original
result. Stable target state alone does not resolve an uncertain launched effect;
it remains unknown unless actual inclusion or a definitive no-launch/rejection
record establishes the result. Private immutable markers and digest-checked
reports must be retained across service restarts.

`GitHubIntegrator::reconcile_original_current(store, credential, integration_id)`
provides scheduled recovery for that original intent. It obtains fresh current
mapping and identity admission, and compares unknown results against the actual
digest-verified report, index and complete original confirmation envelope. Stable
unknown observations retain their confirmation fact ID after a fresh query;
changed or missing proof requires an admitted original reobservation. The query
ignores newer PR heads/checks and never issues another POST. Only a proven
expired observation lease may renew.

Confirmed/rejected receipts describe historical settled effects and retain their
original candidate and proof. Scheduled original recovery does not rewrite them
when the target later changes. Use current-fact polling to detect subsequent
target drift; historical confirmation is not a claim that today's target is
unchanged. Neither query finalizes work or asserts that authoritative source
writeback succeeded.

Integration confirmation, authoritative source writeback and final task acceptance
remain separate. Neither the library nor its worker establishes signed webhook
delivery, squash/rebase finalization or complete native-client business acceptance.

## Optional Team service worker

Set `AWR_TEAM_GITHUB_WORKER_CONFIG` to a private TOML file based on
[`worker.toml`](../../../examples/github-delivery/worker.toml). It declares the
project key, worker ID, explicit repository mapping, AWR credential environment
name, separate provider credential environment name and mandatory
`integration_enabled` mode. Raw credential values, unknown fields and transport
injection are refused. Use an existing private report directory and retain it
through restart. This configuration creates no identity, grant or approval.

Each configured principal needs current scoped AWR observation authority and a
matching enabled `github` connector. Effect mode additionally requires existing
integration/finalization authority. Provider access is authenticated separately.
The worker never infers AWR authority from repository credentials or a GitHub
approval. `integration_enabled = false` only observes current facts; it does not
lease or dispatch even an approved prepared intent. Enable effects explicitly
to execute **already prepared** authorized integrations and recover their original
results. The worker does not plan work, create approval or finalize acceptance.

The Team service validates every optional source, local Git and GitHub group
before credential lookup, then completes all sealed admissions before spawning
any group, under one 30-second startup deadline. A denied later group cannot
leave an earlier source publisher or repository worker running. Duplicate
observer connector scopes are refused across provider groups. An absent/empty
group touches no worker credentials, provider or PG connection. Existing
`DeliveryWorkers::start` and source-only callers retain their entry points;
`start_with_github` adds the explicit third group. Trusted Rust transport
extensions still undergo configuration and PG admission; HTTP/MCP callers cannot
select them.

Each loop obtains a fresh authenticated schedule. Current-fact observation uses
`reconcile_current`; original integration recovery uses
`reconcile_original_current`. Missing/stale current selection does not hide
dispatched original requests. Live leases are observed rather than stolen;
prepared or proven expired pre-dispatch intents can be leased through the neutral
store. Dispatched/unknown originals are only queried. Settled history remains
immutable, and no uncertainty permits another POST. Approval/check/source changes
are rechecked by the actual store and effect preflight.

Worker count, page size, pages/jobs per poll and operation/lease limits are bounded.
Every page is authenticated; a partially visited page never advances past
unvisited rows. Provider rate limits, revoked credentials, mapping changes and
store failures use finite redacted failure codes and bounded exponential backoff.
Monitors expose loop counters and state, never credentials or provider bodies;
these counters are not delivery receipts.

Drop, explicit stop and HTTP shutdown share cancellation. Blocking HTTPS jobs
retain their permits until their bounded work exits, even if the async future
is dropped. Startup resolves credentials once into zeroizing memory; explicit
restart is the rotation boundary. A revoked credential cannot keep observing or
execute from cached authority. Restart recovers durable original intents and
digest-bound reports without persisting secret values. Signed notification,
physical GitHub-to-source completion timing and complete natural-client business
acceptance need their own evidence.
